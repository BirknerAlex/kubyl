//! Which Prometheus-compatible servers each cluster has: Prometheus, Thanos Query, OpenShift's
//! thanos-querier, VictoriaMetrics. The Prometheus sidebar row shows while a cluster has at
//! least one, and the view lets the user pick between several.
//!
//! A server that wants a username and password (HTTP basic auth, a `401` with a `Basic`
//! challenge) is listed locked. The API server strips credentials from service-proxy requests,
//! so a signed-in server is read through a temporary loopback port-forward. The Authorization
//! header is kept in the keychain while the server takes it; once rejected, it's deleted and the
//! view asks again.
//!
//! Only discovery and sign-in live here. Data (targets, rules, query results) is read by the open view
//! itself, so nothing is polled for a view nobody has open.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, Global, SharedString, Subscription, Task,
};
use kubyl_core::{ClusterId, Gvr, ResourceRef, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_metrics::MetricsService;
use kubyl_metrics::basic_auth::{self, SavedBasic};
use kubyl_metrics::discover::{self, Candidate};
use kubyl_metrics::prometheus::{PromClient, PromError, Target, basic_authorization};
use kubyl_metrics::settings::MetricsSettings;
use kubyl_metrics::transport::{ExternalTls, Transport};
use kubyl_portforward::manager::{
    Ephemeral, ForwardId, ForwardSpec, ForwardState, PortForwardManager,
};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};
use kubyl_settings::Settings;
use secrecy::SecretString;
use serde_json::Value;

/// How long a cluster without any Prometheus waits before it is searched again.
const REDISCOVER: Duration = Duration::from_secs(300);
/// How long discovery waits for the metrics service's own detection (it knows about overrides,
/// external URLs and OpenShift Routes).
const METRICS_WAIT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// Services probed per cluster at most.
const MAX_PROBES: usize = 8;
/// Listing Services and probing them together may take this long.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the loopback forward to a signed-in server may take to listen.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(20);

/// Keychain entries of a server's credentials are `prometheus-auth:<cluster id>/<Instance::id>`.
const AUTH_PREFIX: &str = "prometheus-auth";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Prometheus,
    Thanos,
    VictoriaMetrics,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Prometheus => "Prometheus",
            Kind::Thanos => "Thanos Query",
            Kind::VictoriaMetrics => "VictoriaMetrics",
        }
    }

    fn of(target: &Target) -> Self {
        let (name, is_service) = match target {
            Target::Service { service, .. } | Target::Route { service, .. } => {
                (service.to_lowercase(), true)
            }
            Target::Url { url, .. } => (url.to_lowercase(), false),
        };
        // Thanos' other components (ruler, store) have no query UI either, but only a querier
        // answers the query API.
        if name.contains("thanos") && (name.contains("query") || name.contains("querier")) {
            Kind::Thanos
        } else if name.contains("victoria") || (is_service && name.starts_with("vm")) {
            Kind::VictoriaMetrics
        } else {
            Kind::Prometheus
        }
    }
}

/// Whether a server can be read, or wants a username and password first.
#[derive(Clone, Debug, PartialEq)]
pub enum Access {
    /// Answers without credentials.
    Open,
    /// HTTP basic auth and no working credentials. `saved`: the keychain has some (the
    /// forward was stopped, or the server didn't answer), so reconnecting may work. `problem`
    /// says why the last attempt failed.
    Locked {
        saved: bool,
        problem: Option<SharedString>,
    },
    /// Signing in.
    Unlocking,
    /// Signed in with credentials from the keychain, through a loopback forward.
    SignedIn,
}

/// A server the view can talk to.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Stable within a cluster: `monitoring/prometheus-operated`.
    pub id: String,
    pub kind: Kind,
    pub client: PromClient,
    pub access: Access,
    /// Which sign-in the client belongs to: a refused read only locks the server again when
    /// it was made with the current credentials.
    pub session: u64,
}

impl Instance {
    pub(crate) fn new(client: PromClient) -> Self {
        let target = client.target();
        Self {
            id: key(target),
            kind: Kind::of(target),
            client,
            access: Access::Open,
            session: 0,
        }
    }

    /// A server that asks for a username and password.
    pub(crate) fn locked(client: PromClient) -> Self {
        Self {
            access: Access::Locked {
                saved: false,
                problem: None,
            },
            ..Self::new(client)
        }
    }

    /// Whether the tabs can read it.
    pub fn is_readable(&self) -> bool {
        matches!(self.access, Access::Open | Access::SignedIn)
    }

    /// `monitoring/prometheus-k8s`.
    pub fn label(&self) -> String {
        self.client.target().label()
    }

    /// The Service behind it (for the web UI), when it is one.
    pub fn service(&self) -> Option<(&str, &str, &str, &str)> {
        match self.client.target() {
            Target::Service {
                namespace,
                service,
                port,
                path,
                ..
            } => Some((namespace, service, port, path)),
            _ => None,
        }
    }
}

/// One server, however it is reached (service proxy or Route).
fn key(target: &Target) -> String {
    match target {
        Target::Service {
            namespace, service, ..
        }
        | Target::Route {
            namespace, service, ..
        } => format!("{namespace}/{service}"),
        Target::Url { url, .. } => url.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Unknown,
    Discovering,
    Ready,
    /// Nothing answered. The text says why, for the empty state.
    NoSource(SharedString),
    Disabled,
}

struct ClusterState {
    phase: Phase,
    instances: Vec<Instance>,
    generation: u64,
    since: Instant,
    discovered_at: Option<Instant>,
    discovering: bool,
    /// The loopback forwards of signed-in servers, by `Instance::id`.
    forwards: HashMap<String, ForwardId>,
}

impl ClusterState {
    fn new(generation: u64) -> Self {
        Self {
            phase: Phase::Unknown,
            instances: Vec::new(),
            generation,
            since: Instant::now(),
            discovered_at: None,
            discovering: false,
            forwards: HashMap::new(),
        }
    }

    fn instance_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }
}

/// How signing in to a locked server ended.
enum Unlock {
    /// The keychain has nothing for it.
    NoCredentials,
    /// The Service was re-created since the credentials were saved.
    Replaced,
    /// The server said no to the username and password.
    Rejected,
    Failed(String),
}

pub struct PrometheusService {
    clusters: HashMap<ClusterId, ClusterState>,
    generation: u64,
    /// Numbers sign-ins (`Instance::session`).
    sessions: u64,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

struct GlobalService(Entity<PrometheusService>);

impl Global for GlobalService {}

impl PrometheusService {
    /// Creates the service and makes it global. `tick` starts the discovery loop (off in tests).
    pub fn install(tick: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(|cx| Self::new(tick, cx));
        cx.set_global(GlobalService(service.clone()));
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    fn new(tick: bool, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |this, _, event, cx| {
                this.connection_event(event, cx)
            }));
        }
        let task = if tick {
            cx.spawn(async move |this, cx| {
                loop {
                    if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                }
            })
        } else {
            Task::ready(())
        };
        Self {
            clusters: HashMap::new(),
            generation: 0,
            sessions: 0,
            _tick: task,
            _subscriptions: subscriptions,
        }
    }

    // ----- Reading -----

    pub fn phase(&self, cluster: &ClusterId) -> Phase {
        self.clusters
            .get(cluster)
            .map_or(Phase::Unknown, |c| c.phase.clone())
    }

    pub fn instances(&self, cluster: &ClusterId) -> &[Instance] {
        self.clusters
            .get(cluster)
            .map_or(&[], |c| c.instances.as_slice())
    }

    /// Whether the cluster has a Prometheus (`None` while still looking).
    pub fn has_instances(&self, cluster: &ClusterId) -> Option<bool> {
        match self.clusters.get(cluster)?.phase {
            Phase::Ready => Some(true),
            Phase::NoSource(_) | Phase::Disabled => Some(false),
            Phase::Unknown | Phase::Discovering => None,
        }
    }

    // ----- Control -----

    /// Forgets what was found and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.reset(cluster, cx);
        self.tick(cx);
        crate::changed(cx);
        cx.notify();
    }

    fn reset(&mut self, cluster: &ClusterId, cx: &mut App) {
        self.generation += 1;
        if let Some(old) = self
            .clusters
            .insert(cluster.clone(), ClusterState::new(self.generation))
        {
            stop_forwards(old.forwards.into_values(), cx);
        }
    }

    /// Forgets a cluster and stops its forwards.
    fn remove(&mut self, cluster: &ClusterId, cx: &mut App) -> bool {
        match self.clusters.remove(cluster) {
            Some(old) => {
                stop_forwards(old.forwards.into_values(), cx);
                true
            }
            None => false,
        }
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                // A reconnect may reach another cluster (kubeconfig edited): start over.
                if !connected && self.remove(id, cx) {
                    self.generation += 1;
                    crate::changed(cx);
                    cx.notify();
                }
            }
            ConnectionEvent::DiscoveryChanged(id) => {
                // Prometheus may have been installed.
                if matches!(self.phase(id), Phase::NoSource(_)) {
                    self.reset(id, cx);
                    cx.notify();
                }
            }
            ConnectionEvent::Rekeyed { from, to } if self.remove(from, cx) => {
                self.reset(to, cx);
                crate::changed(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let connected: Vec<(ClusterId, kube::Client, Vec<String>)> = {
            let manager = manager.read(cx);
            manager
                .entries()
                .iter()
                .filter_map(|c| {
                    Some((
                        c.id.clone(),
                        manager.client(&c.id)?,
                        manager.settings_keys(&c.id),
                    ))
                })
                .collect()
        };
        let gone: Vec<ClusterId> = self
            .clusters
            .keys()
            .filter(|id| !connected.iter().any(|(c, _, _)| &c == id))
            .cloned()
            .collect();
        if !gone.is_empty() {
            for id in gone {
                self.remove(&id, cx);
            }
            self.generation += 1;
            crate::changed(cx);
            cx.notify();
        }
        for (cluster, client, keys) in connected {
            let disabled = Settings::get::<MetricsSettings>(cx)
                .prometheus_for_keys(&keys)
                .is_some_and(|p| p.disabled);
            let generation = self.generation;
            let state = self
                .clusters
                .entry(cluster.clone())
                .or_insert_with(|| ClusterState::new(generation));
            if disabled {
                if state.phase != Phase::Disabled {
                    state.phase = Phase::Disabled;
                    crate::changed(cx);
                    cx.notify();
                }
                continue;
            }
            if state.phase == Phase::Disabled {
                state.phase = Phase::Unknown;
            }
            let rediscover = match state.phase {
                Phase::Unknown | Phase::Discovering => true,
                Phase::NoSource(_) => state
                    .discovered_at
                    .is_some_and(|t| t.elapsed() >= REDISCOVER),
                Phase::Ready | Phase::Disabled => false,
            };
            if rediscover && !state.discovering {
                self.discover(&cluster, client, cx);
            }
        }
    }

    fn discover(&mut self, cluster: &ClusterId, client: kube::Client, cx: &mut Context<Self>) {
        // The metrics service knows settings overrides, external URLs and OpenShift Routes:
        // wait a little for its detection, then take its client as the first instance.
        let (metrics_client, metrics_pending) = match MetricsService::global(cx) {
            Some(metrics) => {
                let metrics = metrics.read(cx);
                let source = metrics.source(cluster);
                (
                    metrics.prometheus(cluster),
                    matches!(
                        source,
                        kubyl_metrics::Source::Unknown | kubyl_metrics::Source::Detecting
                    ),
                )
            }
            None => (None, false),
        };
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if metrics_pending && state.since.elapsed() < METRICS_WAIT {
            state.phase = Phase::Discovering;
            return;
        }
        state.discovering = true;
        state.phase = Phase::Discovering;
        let generation = state.generation;
        let task = spawn_kube(cx, async move {
            // A stalled API server must not leave the cluster "discovering" forever.
            tokio::time::timeout(DISCOVERY_TIMEOUT, find(client, metrics_client))
                .await
                .unwrap_or_default()
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let instances = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.clusters.get_mut(&cluster) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.discovering = false;
                state.discovered_at = Some(Instant::now());
                state.phase = if instances.is_empty() {
                    Phase::NoSource(
                        "No Prometheus, Thanos Query or VictoriaMetrics answered in this cluster."
                            .into(),
                    )
                } else {
                    Phase::Ready
                };
                let locked: Vec<String> = instances
                    .iter()
                    .filter(|i| !i.is_readable())
                    .map(|i| i.id.clone())
                    .collect();
                state.instances = instances;
                // Signs in with what the keychain has, if anything.
                for id in locked {
                    this.unlock(&cluster, &id, None, cx);
                }
                crate::changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ----- Signing in -----

    /// Signs in to a locked server with a username and password; they're kept in the keychain
    /// once the server takes them.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        username: &str,
        password: &SecretString,
        cx: &mut Context<Self>,
    ) {
        let header = basic_authorization(username, password);
        self.unlock(cluster, id, Some(header), cx);
    }

    /// Signs in again with the credentials in the keychain.
    pub fn reconnect(&mut self, cluster: &ClusterId, id: &str, cx: &mut Context<Self>) {
        self.unlock(cluster, id, None, cx);
    }

    /// A read of a signed-in server was refused: its credentials no longer work. They're
    /// deleted, and the view asks again. `session` is the sign-in the read was made with: a
    /// late answer to older credentials changes nothing.
    pub fn rejected(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        session: u64,
        cx: &mut Context<Self>,
    ) {
        let keys = auth_keys(cluster, id, cx);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(instance) = state.instance_mut(id) else {
            return;
        };
        if instance.access != Access::SignedIn || instance.session != session {
            return;
        }
        instance.access = Access::Locked {
            saved: false,
            problem: Some("The server no longer takes the saved username and password.".into()),
        };
        stop_forwards(state.forwards.remove(id), cx);
        delete_secrets(keys, cx);
        cx.notify();
    }

    /// The user stopped a signed-in server's forward in Active Sessions.
    fn forward_stopped(&mut self, cluster: &ClusterId, id: &str, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if state.forwards.remove(id).is_none() {
            return;
        }
        if let Some(instance) = state.instance_mut(id) {
            instance.access = Access::Locked {
                saved: true,
                problem: Some("The port-forward was stopped.".into()),
            };
        }
        cx.notify();
    }

    /// Connects to a locked server through a loopback forward with `header` (`None`: the one in
    /// the keychain, if the Service is still the one it was given to), and keeps a header the
    /// user typed once the server takes it.
    fn unlock(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        header: Option<SecretString>,
        cx: &mut Context<Self>,
    ) {
        let client = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster));
        let keys = auth_keys(cluster, id, cx);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let generation = state.generation;
        let Some(instance) = state.instance_mut(id) else {
            return;
        };
        let was_saved = match &instance.access {
            Access::Locked { saved, .. } => *saved,
            Access::Unlocking => return,
            Access::Open | Access::SignedIn => false,
        };
        let Some(client) = client else {
            instance.access = Access::Locked {
                saved: was_saved,
                problem: Some("The cluster isn't connected.".into()),
            };
            cx.notify();
            return;
        };
        let target = instance.client.target().clone();
        instance.access = Access::Unlocking;
        cx.notify();
        let typed = header.is_some();
        let cluster = cluster.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let result = async {
                let uid = service_uid(&client, &target, cx).await?;
                let header = match header {
                    Some(header) => header,
                    None => {
                        let saved = read_saved(cx, &keys).await.ok_or(Unlock::NoCredentials)?;
                        // Never to a Service that replaced the one the user signed in to.
                        if saved.service_uid != uid {
                            return Err(Unlock::Replaced);
                        }
                        saved.header
                    }
                };
                let connected = connect(&cluster, &id, &target, client, header.clone(), cx).await?;
                Ok((connected, header, uid))
            }
            .await;
            // Keep what the user typed only once it worked; forget saved credentials the server
            // refused or that belong to a replaced Service. (A mistyped password leaves saved
            // ones alone.)
            match &result {
                Ok((_, header, uid)) if typed => {
                    let saved = SavedBasic {
                        header: header.clone(),
                        service_uid: uid.clone(),
                    };
                    if let Some(key) = keys.first()
                        && let Err(err) = write_secret(cx, key.clone(), saved.to_secret()).await
                    {
                        tracing::warn!("Prometheus credentials not stored: {err}");
                    }
                }
                Err(Unlock::Rejected) if !typed => cx.update(|cx| delete_secrets(keys, cx)),
                Err(Unlock::Replaced) => cx.update(|cx| delete_secrets(keys, cx)),
                _ => {}
            }
            this.update(cx, |this, cx| {
                this.sessions += 1;
                let session = this.sessions;
                let state = this
                    .clusters
                    .get_mut(&cluster)
                    .filter(|s| s.generation == generation);
                let instance = state.and_then(|state| {
                    let instance = state.instances.iter_mut().find(|i| i.id == id)?;
                    Some((instance, &mut state.forwards))
                });
                let Some((instance, forwards)) = instance else {
                    if let Ok(((_, forward), _, _)) = result {
                        stop_forwards(Some(forward), cx);
                    }
                    return;
                };
                instance.access = match result {
                    Ok(((prom, forward), _, _)) => {
                        instance.client = prom;
                        instance.session = session;
                        stop_forwards(forwards.insert(id.clone(), forward), cx);
                        Access::SignedIn
                    }
                    Err(Unlock::NoCredentials) => Access::Locked {
                        saved: false,
                        problem: None,
                    },
                    Err(Unlock::Replaced) => Access::Locked {
                        saved: false,
                        problem: Some(
                            "The Service was re-created since you signed in. Check that it's \
                             still your Prometheus, then sign in again."
                                .into(),
                        ),
                    },
                    Err(Unlock::Rejected) if typed => Access::Locked {
                        saved: was_saved,
                        problem: Some("The server rejected this username and password.".into()),
                    },
                    Err(Unlock::Rejected) => Access::Locked {
                        saved: false,
                        problem: Some(
                            "The server no longer takes the saved username and password.".into(),
                        ),
                    },
                    Err(Unlock::Failed(err)) => Access::Locked {
                        saved: was_saved || !typed,
                        problem: Some(err.into()),
                    },
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Where a server's credentials may be in the keychain, the one to write first.
fn auth_keys(cluster: &ClusterId, id: &str, cx: &App) -> Vec<String> {
    let ids = ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).settings_keys(cluster))
        .unwrap_or_else(|| vec![cluster.to_string()]);
    basic_auth::keys(AUTH_PREFIX, &ids, id)
}

/// The UID of the Service behind `target`.
async fn service_uid(
    client: &kube::Client,
    target: &Target,
    cx: &mut AsyncApp,
) -> Result<String, Unlock> {
    let Target::Service {
        namespace, service, ..
    } = target
    else {
        return Err(Unlock::Failed("Only a Service can be signed in to.".into()));
    };
    let (client, namespace, service) = (client.clone(), namespace.clone(), service.clone());
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            basic_auth::service_uid(&client, &namespace, &service).await
        })
    });
    task.await.map_err(Unlock::Failed)
}

/// Opens a loopback forward to `target` and checks that the server takes `header` there.
async fn connect(
    cluster: &ClusterId,
    id: &str,
    target: &Target,
    client: kube::Client,
    header: SecretString,
    cx: &mut AsyncApp,
) -> Result<(PromClient, ForwardId), Unlock> {
    let Target::Service {
        namespace,
        service,
        port,
        scheme,
        path,
    } = target
    else {
        return Err(Unlock::Failed("Only a Service can be signed in to.".into()));
    };
    let number: u16 = port
        .parse()
        .map_err(|_| Unlock::Failed(format!("{id} has no port number")))?;
    let on_stop = {
        let (cluster, id) = (cluster.clone(), id.to_string());
        Arc::new(move |cx: &mut App| {
            if let Some(service) = PrometheusService::global(cx) {
                service.update(cx, |this, cx| this.forward_stopped(&cluster, &id, cx));
            }
        })
    };
    let spec = ForwardSpec {
        cluster: cluster.clone(),
        namespace: namespace.clone(),
        kind: ForwardKind::Service {
            service: service.clone(),
        },
        port: RemotePort::Service(Some(number)),
        // Loopback only: the credentials go to this port.
        bind_address: "127.0.0.1".into(),
        local_port: 0,
        target: ResourceRef::object(
            cluster.clone(),
            Gvr::new("", "v1", "services"),
            Some(namespace.clone()),
            service.clone(),
        ),
        remote_port: Some(number),
        http: false,
        https: false,
        open_browser: false,
        ephemeral: Some(Ephemeral {
            title: format!("Prometheus · svc/{service}"),
            buttons: Vec::new(),
            on_stop,
        }),
    };
    let forward = cx.update(|cx| PortForwardManager::start(client, spec, cx));
    let start = Instant::now();
    let local_port = loop {
        let info = cx.update(|cx| PortForwardManager::global(cx).read(cx).info(forward));
        match info {
            Some(info) if info.state == ForwardState::Listening && info.local_port != 0 => {
                break info.local_port;
            }
            Some(info) => {
                if let ForwardState::Failed(err) = info.state {
                    cx.update(|cx| stop_forwards(Some(forward), cx));
                    return Err(Unlock::Failed(format!("port-forward: {err}")));
                }
            }
            None => return Err(Unlock::Failed("The port-forward stopped.".into())),
        }
        if start.elapsed() > FORWARD_TIMEOUT {
            cx.update(|cx| stop_forwards(Some(forward), cx));
            return Err(Unlock::Failed(
                "The port-forward didn't start (needs create pods/portforward).".into(),
            ));
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    };
    // TLS ends in the pod, behind the API server's tunnel: its certificate is for the Service's
    // name, not 127.0.0.1.
    let https = scheme == "https";
    let url = format!(
        "{}://127.0.0.1:{local_port}{}",
        if https { "https" } else { "http" },
        kubyl_metrics::transport::normalize_prefix(path)
    );
    let target = target.clone();
    let probed = cx
        .update(|cx| {
            spawn_kube(cx, async move {
                let tls = ExternalTls {
                    insecure: https,
                    ..Default::default()
                };
                let transport = Transport::external(&url, Some(&header), &tls)?;
                let prom = PromClient::from_transport(transport, target);
                prom.probe(PROBE_TIMEOUT).await.map(|()| prom)
            })
        })
        .await;
    match probed {
        Ok(prom) => Ok((prom, forward)),
        Err(err) => {
            cx.update(|cx| stop_forwards(Some(forward), cx));
            Err(match err {
                PromError::Http(401, _) => Unlock::Rejected,
                err => Unlock::Failed(err.to_string()),
            })
        }
    }
}

fn stop_forwards(forwards: impl IntoIterator<Item = ForwardId>, cx: &mut App) {
    let mut forwards = forwards.into_iter().peekable();
    if forwards.peek().is_none() {
        return;
    }
    PortForwardManager::global(cx).update(cx, |manager, cx| {
        for id in forwards {
            manager.stop(id, cx);
        }
    });
}

/// The first readable entry of `keys`.
async fn read_saved(cx: &mut AsyncApp, keys: &[String]) -> Option<SavedBasic> {
    let keys = keys.to_vec();
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            tokio::task::spawn_blocking(move || {
                keys.iter().find_map(|key| {
                    kubyl_kube::auth::store::get(key)
                        .inspect_err(|e| tracing::warn!("keychain: {e}"))
                        .ok()
                        .flatten()
                        .and_then(|secret| SavedBasic::parse(&secret))
                })
            })
            .await
            .ok()
            .flatten()
        })
    });
    task.await
}

async fn write_secret(cx: &mut AsyncApp, key: String, secret: SecretString) -> Result<(), String> {
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            tokio::task::spawn_blocking(move || kubyl_kube::auth::store::set(&key, &secret))
                .await
                .map_err(|e| e.to_string())?
        })
    });
    task.await
}

fn delete_secrets(keys: Vec<String>, cx: &mut App) {
    spawn_kube(cx, async move {
        tokio::task::spawn_blocking(move || {
            for key in keys {
                kubyl_kube::auth::store::delete(&key).ok();
            }
        })
        .await
        .ok();
    })
    .detach();
}

/// Lists Services, ranks and de-duplicates them, and probes each. The metrics service's client
/// comes first, servers that want a password last.
async fn find(client: kube::Client, metrics_client: Option<PromClient>) -> Vec<Instance> {
    let services = discover::list_services(&client).await;
    let targets = probe_targets(&services, metrics_client.as_ref().map(PromClient::target));
    let probes = targets.into_iter().take(MAX_PROBES).map(|target| {
        let prom = PromClient::new(client.clone(), target);
        async move {
            match prom.probe(PROBE_TIMEOUT).await {
                Ok(()) => Some(Instance::new(prom)),
                // An auth proxy (OpenShift's prometheus-k8s) says 401 too, without a Basic
                // challenge: no password would help there.
                Err(PromError::Http(401, _)) if prom.asks_for_basic_auth().await => {
                    Some(Instance::locked(prom))
                }
                Err(_) => None,
            }
        }
    });
    let mut found: Vec<Instance> = metrics_client.into_iter().map(Instance::new).collect();
    for instance in futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
    {
        if !found.iter().any(|f| f.id == instance.id) {
            found.push(instance);
        }
    }
    // The view shows the first one.
    found.sort_by_key(|i| !i.is_readable());
    found
}

fn selector(service: &Value) -> BTreeMap<String, String> {
    service["spec"]["selector"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// The candidates worth probing, best first. Several Services can front the same pods
/// (`prometheus-operated` and the chart's `…-prometheus`): when one's selector is the same as
/// or a subset of a better ranked Service's in the same namespace, it isn't another instance.
pub fn probe_targets(services: &[Value], preferred: Option<&Target>) -> Vec<Target> {
    let mut ranked: Vec<(Candidate, String, BTreeMap<String, String>)> = services
        .iter()
        .filter_map(|service| {
            let candidate = discover::candidates(std::slice::from_ref(service))
                .into_iter()
                .next()?;
            let namespace = service["metadata"]["namespace"]
                .as_str()
                .unwrap_or("default")
                .to_string();
            Some((candidate, namespace, selector(service)))
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.score
            .cmp(&a.0.score)
            .then_with(|| a.0.target.label().cmp(&b.0.target.label()))
    });
    // The server the metrics service found comes first whatever its rank; Services over its pods
    // (either way round: `prometheus-operated` covers a chart's own Service) aren't another one.
    let preferred_pods = preferred.and_then(|target| {
        let (Target::Service {
            namespace, service, ..
        }
        | Target::Route {
            namespace, service, ..
        }) = target
        else {
            return None;
        };
        let found = services.iter().find(|s| {
            s["metadata"]["namespace"].as_str() == Some(namespace)
                && s["metadata"]["name"].as_str() == Some(service)
        })?;
        Some((namespace.clone(), selector(found)))
    });
    let mut kept: Vec<(Candidate, String, BTreeMap<String, String>)> = Vec::new();
    for (candidate, namespace, selector) in ranked {
        let same_as_preferred = !selector.is_empty()
            && preferred_pods.as_ref().is_some_and(|(ns, other)| {
                *ns == namespace
                    && !other.is_empty()
                    && (selector.iter().all(|(k, v)| other.get(k) == Some(v))
                        || other.iter().all(|(k, v)| selector.get(k) == Some(v)))
            });
        let same_pods = same_as_preferred
            || (!selector.is_empty()
                && kept.iter().any(|(_, ns, other)| {
                    *ns == namespace && selector.iter().all(|(k, v)| other.get(k) == Some(v))
                }));
        if !same_pods {
            kept.push((candidate, namespace, selector));
        }
    }
    kept.into_iter().map(|(c, _, _)| c.target).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc(namespace: &str, name: &str, selector: Value) -> Value {
        json!({"metadata": {"name": name, "namespace": namespace},
               "spec": {"ports": [{"name": "web", "port": 9090}], "selector": selector}})
    }

    fn labels(targets: &[Target]) -> Vec<String> {
        targets.iter().map(Target::label).collect()
    }

    #[test]
    fn services_in_front_of_the_same_pods_are_one_instance() {
        let services = vec![
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                json!({"app.kubernetes.io/name": "prometheus", "operator.prometheus.io/name": "kps"}),
            ),
            svc(
                "monitoring",
                "prometheus-operated",
                json!({"app.kubernetes.io/name": "prometheus"}),
            ),
        ];
        assert_eq!(
            labels(&probe_targets(&services, None)),
            ["monitoring/kube-prometheus-stack-prometheus"]
        );
    }

    #[test]
    fn two_prometheuses_stay_two() {
        let services = vec![
            svc(
                "monitoring",
                "prometheus-a",
                json!({"app.kubernetes.io/name": "prometheus", "operator.prometheus.io/name": "a"}),
            ),
            svc(
                "monitoring",
                "prometheus-b",
                json!({"app.kubernetes.io/name": "prometheus", "operator.prometheus.io/name": "b"}),
            ),
            svc(
                "other",
                "prometheus-server",
                json!({"app.kubernetes.io/name": "prometheus"}),
            ),
        ];
        assert_eq!(probe_targets(&services, None).len(), 3);
    }

    #[test]
    fn thanos_next_to_prometheus_is_another_instance() {
        let services = vec![
            svc(
                "monitoring",
                "prometheus-operated",
                json!({"app": "prometheus"}),
            ),
            svc("monitoring", "thanos-query", json!({"app": "thanos-query"})),
        ];
        let targets = probe_targets(&services, None);
        assert_eq!(targets.len(), 2);
        assert_eq!(Kind::of(&targets[1]), Kind::Thanos);
    }

    #[test]
    fn a_service_over_the_preferred_servers_pods_is_not_another_one() {
        let services = vec![
            svc(
                "monitoring",
                "prometheus-operated",
                json!({"app": "prometheus"}),
            ),
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                json!({"app": "prometheus", "operator.prometheus.io/name": "kps"}),
            ),
            svc("monitoring", "thanos-query", json!({"app": "thanos-query"})),
        ];
        let preferred = Target::service("monitoring", "prometheus-operated", "9090");
        assert_eq!(
            labels(&probe_targets(&services, Some(&preferred))),
            ["monitoring/thanos-query"]
        );
    }

    #[test]
    fn kinds_follow_the_name() {
        let kind = |name: &str| Kind::of(&Target::service("ns", name, "9090"));
        assert_eq!(kind("thanos-querier"), Kind::Thanos);
        assert_eq!(kind("thanos-query-frontend"), Kind::Thanos);
        assert_eq!(kind("thanos-ruler"), Kind::Prometheus);
        assert_eq!(kind("vmselect-cluster"), Kind::VictoriaMetrics);
        assert_eq!(kind("prometheus-k8s"), Kind::Prometheus);
    }
}
