//! Which Prometheus-compatible servers each cluster has: Prometheus, Thanos Query, OpenShift's
//! thanos-querier, VictoriaMetrics. The Prometheus sidebar row shows while a cluster has at
//! least one, and the view lets the user pick between several.
//!
//! A server that wants a username and password (HTTP basic auth, a `401` with a `Basic`
//! challenge) is listed locked. The API server strips credentials from service-proxy requests,
//! so a signed-in server is read through a temporary loopback port-forward (a [`Reach`]). The
//! Authorization header is kept in the keychain while the server takes it; once rejected, it's
//! deleted and the view asks again.
//!
//! Only discovery and sign-in live here. Data (targets, rules, query results) is read by the open
//! view itself, so nothing is polled for a view nobody has open.
//!
//! [`PrometheusCore`] is a plain state struct on a [`Host`]. What it reads from the app (the
//! connected clusters, the metrics service's client, settings) comes in as snapshots
//! ([`Discovery`], [`ClusterConn`]); what it asks of the app is a [`PrometheusEffect`].

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kubyl_base::ClusterId;
use kubyl_base::host::{Flow, Host, HostExt as _, Service, TaskHandle};
use kubyl_metrics_core::basic_auth::{self, SavedBasic};
use kubyl_metrics_core::prometheus::{PromClient, PromError, Target};
use kubyl_metrics_core::transport::{ExternalTls, Transport};
use kubyl_portforward_core::reach::{Reach, ReachPort, ReachRequest, Reached};
use secrecy::SecretString;

use crate::servers::*;

/// How long the loopback forward to a signed-in server may take to listen.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(20);

/// How often the app is asked for the connections to look at.
const TICK: Duration = Duration::from_secs(2);

/// Keychain entries of a server's credentials are `prometheus-auth:<cluster id>/<Instance::id>`.
const AUTH_PREFIX: &str = "prometheus-auth";

/// Where the credentials of signed-in servers are kept. Calls block: the service runs them on a
/// blocking thread.
pub trait Credentials: Send + Sync + 'static {
    fn get(&self, key: &str) -> Result<Option<SecretString>, String>;
    fn set(&self, key: &str, secret: &SecretString) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The OS keychain (or whichever store `kubyl_kube_core::auth::store` was given).
pub struct Keychain;

impl Credentials for Keychain {
    fn get(&self, key: &str) -> Result<Option<SecretString>, String> {
        kubyl_kube_core::auth::store::get(key)
    }

    fn set(&self, key: &str, secret: &SecretString) -> Result<(), String> {
        kubyl_kube_core::auth::store::set(key, secret)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        kubyl_kube_core::auth::store::delete(key)
    }
}

/// What the service asks the app to do.
#[derive(Debug, PartialEq, Eq)]
pub enum PrometheusEffect {
    /// The servers of a cluster changed (the sidebar row may appear or go).
    Changed,
    /// Look at the connected clusters now: call [`PrometheusCore::tick`] with their connections.
    Tick,
}

/// What the service reads of a connected cluster to find its servers.
#[derive(Clone)]
pub struct Discovery {
    pub conn: ClusterConn,
    /// The cluster's `"prometheus"` override in the metrics settings turns Prometheus off.
    pub disabled: bool,
    /// What the metrics service knows (settings overrides, external URLs, OpenShift Routes).
    pub metrics: MetricsInfo,
}

/// The metrics service's view of a cluster.
#[derive(Clone, Default)]
pub struct MetricsInfo {
    /// Its Prometheus client, taken as the first instance.
    pub client: Option<PromClient>,
    /// It hasn't finished detecting its source: wait a little for it.
    pub pending: bool,
}

/// How to reach a cluster, for signing in and for the keychain entries of its servers.
#[derive(Clone)]
pub struct ClusterConn {
    /// `None`: the cluster isn't connected.
    pub client: Option<kube::Client>,
    /// `ConnectionManager::settings_keys` (the cluster's id when nothing knows better).
    pub settings_keys: Vec<String>,
}

/// The connected clusters, in the order the app lists them. `tick` takes `None` when the app has
/// no connections to ask.
pub type Conns = Vec<(ClusterId, Discovery)>;

struct ClusterState {
    phase: Phase,
    instances: Vec<Instance>,
    generation: u64,
    since: Instant,
    discovered_at: Option<Instant>,
    discovering: bool,
    /// The connection seen by the last tick, for signing in after a discovery.
    conn: Option<ClusterConn>,
    /// The loopback forwards of signed-in servers, by `Instance::id`.
    forwards: HashMap<String, Forward>,
}

/// A signed-in server's forward. Dropping it stops the forward.
struct Forward {
    /// Tells the watcher of this forward from the watcher of one it replaced.
    token: u64,
    _reached: Reached,
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
            conn: None,
            forwards: HashMap::new(),
        }
    }

    fn instance_mut(&mut self, id: &str) -> Option<&mut Instance> {
        self.instances.iter_mut().find(|i| i.id == id)
    }
}

/// How signing in to a locked server ended.
#[derive(Debug)]
enum Unlock {
    /// The keychain has nothing for it.
    NoCredentials,
    /// The Service was re-created since the credentials were saved.
    Replaced,
    /// The server said no to the username and password.
    Rejected,
    Failed(String),
}

/// A server that took the credentials, with the forward it is read through.
struct Signed {
    client: PromClient,
    reached: Reached,
}

pub struct PrometheusCore {
    clusters: HashMap<ClusterId, ClusterState>,
    generation: u64,
    /// Numbers sign-ins (`Instance::session`).
    sessions: u64,
    /// Numbers forwards (`Forward::token`).
    forwards: u64,
    reach: Arc<dyn Reach>,
    credentials: Arc<dyn Credentials>,
    _timers: Vec<TaskHandle>,
}

impl Service for PrometheusCore {
    type Event = Infallible;
    type Effect = PrometheusEffect;
}

impl PrometheusCore {
    pub fn new(reach: Arc<dyn Reach>, credentials: Arc<dyn Credentials>) -> Self {
        Self {
            clusters: HashMap::new(),
            generation: 0,
            sessions: 0,
            forwards: 0,
            reach,
            credentials,
            _timers: Vec::new(),
        }
    }

    /// Starts the discovery loop: [`PrometheusEffect::Tick`] right away and then every two
    /// seconds.
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        self._timers = vec![
            host.after(Duration::ZERO, |_, host| {
                host.effect(PrometheusEffect::Tick)
            }),
            host.every(TICK, |_, host| {
                host.effect(PrometheusEffect::Tick);
                Flow::Continue
            }),
        ];
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
    pub fn redetect(
        &mut self,
        cluster: &ClusterId,
        conns: Option<&Conns>,
        host: &mut dyn Host<Self>,
    ) {
        self.reset(cluster);
        self.tick(conns, host);
        host.effect(PrometheusEffect::Changed);
        host.notify();
    }

    fn reset(&mut self, cluster: &ClusterId) {
        self.generation += 1;
        // The old state's forwards stop with it.
        self.clusters
            .insert(cluster.clone(), ClusterState::new(self.generation));
    }

    /// Forgets a cluster and stops its forwards.
    fn remove(&mut self, cluster: &ClusterId) -> bool {
        self.clusters.remove(cluster).is_some()
    }

    /// A cluster connected or disconnected.
    pub fn connection_changed(
        &mut self,
        cluster: &ClusterId,
        connected: bool,
        host: &mut dyn Host<Self>,
    ) {
        // A reconnect may reach another cluster (kubeconfig edited): start over.
        if !connected && self.remove(cluster) {
            self.generation += 1;
            host.effect(PrometheusEffect::Changed);
            host.notify();
        }
    }

    /// What the app discovered about a cluster changed.
    pub fn discovery_changed(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        // Prometheus may have been installed.
        if matches!(self.phase(cluster), Phase::NoSource(_)) {
            self.reset(cluster);
            host.notify();
        }
    }

    /// A cluster got another id.
    pub fn rekeyed(&mut self, from: &ClusterId, to: &ClusterId, host: &mut dyn Host<Self>) {
        if self.remove(from) {
            self.reset(to);
            host.effect(PrometheusEffect::Changed);
            host.notify();
        }
    }

    /// Looks at the connected clusters: forgets the gone ones and (re)discovers where due.
    pub fn tick(&mut self, conns: Option<&Conns>, host: &mut dyn Host<Self>) {
        let Some(connected) = conns else {
            return;
        };
        let gone: Vec<ClusterId> = self
            .clusters
            .keys()
            .filter(|id| !connected.iter().any(|(c, _)| &c == id))
            .cloned()
            .collect();
        if !gone.is_empty() {
            for id in gone {
                self.remove(&id);
            }
            self.generation += 1;
            host.effect(PrometheusEffect::Changed);
            host.notify();
        }
        for (cluster, input) in connected {
            let generation = self.generation;
            let state = self
                .clusters
                .entry(cluster.clone())
                .or_insert_with(|| ClusterState::new(generation));
            state.conn = Some(input.conn.clone());
            if input.disabled {
                if state.phase != Phase::Disabled {
                    state.phase = Phase::Disabled;
                    host.effect(PrometheusEffect::Changed);
                    host.notify();
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
                self.discover(cluster, input, host);
            }
        }
    }

    fn discover(&mut self, cluster: &ClusterId, input: &Discovery, host: &mut dyn Host<Self>) {
        let Some(client) = input.conn.client.clone() else {
            return;
        };
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        // The metrics service knows settings overrides, external URLs and OpenShift Routes:
        // wait a little for its detection, then take its client as the first instance.
        if input.metrics.pending && state.since.elapsed() < METRICS_WAIT {
            state.phase = Phase::Discovering;
            return;
        }
        state.discovering = true;
        state.phase = Phase::Discovering;
        let generation = state.generation;
        let metrics_client = input.metrics.client.clone();
        let cluster = cluster.clone();
        host.spawn(
            async move {
                // A stalled API server must not leave the cluster "discovering" forever.
                tokio::time::timeout(DISCOVERY_TIMEOUT, find(client, metrics_client))
                    .await
                    .unwrap_or_default()
            },
            move |this, instances, host| {
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
                let conn = state.conn.clone();
                // Signs in with what the keychain has, if anything.
                for id in locked {
                    let conn = conn.clone().unwrap_or(ClusterConn {
                        client: None,
                        settings_keys: vec![cluster.to_string()],
                    });
                    this.unlock(&cluster, &id, None, conn, host);
                }
                host.effect(PrometheusEffect::Changed);
                host.notify();
            },
        )
        .detach();
        host.notify();
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
        conn: ClusterConn,
        host: &mut dyn Host<Self>,
    ) {
        let header = kubyl_metrics_core::prometheus::basic_authorization(username, password);
        self.unlock(cluster, id, Some(header), conn, host);
    }

    /// Signs in again with the credentials in the keychain.
    pub fn reconnect(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        conn: ClusterConn,
        host: &mut dyn Host<Self>,
    ) {
        self.unlock(cluster, id, None, conn, host);
    }

    /// A read of a signed-in server was refused: its credentials no longer work. They're
    /// deleted, and the view asks again. `session` is the sign-in the read was made with: a
    /// late answer to older credentials changes nothing.
    pub fn rejected(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        session: u64,
        conn: &ClusterConn,
        host: &mut dyn Host<Self>,
    ) {
        let keys = basic_auth::keys(AUTH_PREFIX, &conn.settings_keys, id);
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
        state.forwards.remove(id);
        self.delete_secrets(keys, host);
        host.notify();
    }

    /// The user (or the cluster's disconnect) ended a signed-in server's forward.
    fn forward_stopped(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        token: u64,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        // A forward that was replaced or dropped by us isn't a stop by the user.
        if state.forwards.get(id).is_none_or(|f| f.token != token) {
            return;
        }
        state.forwards.remove(id);
        if let Some(instance) = state.instance_mut(id) {
            instance.access = Access::Locked {
                saved: true,
                problem: Some("The port-forward was stopped.".into()),
            };
        }
        host.notify();
    }

    /// Connects to a locked server through a loopback forward with `header` (`None`: the one in
    /// the keychain, if the Service is still the one it was given to), and keeps a header the
    /// user typed once the server takes it.
    fn unlock(
        &mut self,
        cluster: &ClusterId,
        id: &str,
        header: Option<SecretString>,
        conn: ClusterConn,
        host: &mut dyn Host<Self>,
    ) {
        let keys = basic_auth::keys(AUTH_PREFIX, &conn.settings_keys, id);
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
        let Some(client) = conn.client else {
            instance.access = Access::Locked {
                saved: was_saved,
                problem: Some("The cluster isn't connected.".into()),
            };
            host.notify();
            return;
        };
        let target = instance.client.target().clone();
        instance.access = Access::Unlocking;
        host.notify();
        let typed = header.is_some();
        let (reach, credentials) = (self.reach.clone(), self.credentials.clone());
        let work = {
            let (cluster, id) = (cluster.clone(), id.to_string());
            async move {
                let result = sign_in_to(
                    &*reach,
                    &credentials,
                    &cluster,
                    &id,
                    &target,
                    client,
                    header,
                    &keys,
                )
                .await;
                // Keep what the user typed only once it worked; forget saved credentials the
                // server refused or that belong to a replaced Service. (A mistyped password
                // leaves saved ones alone.)
                match &result {
                    Ok((_, header, uid)) if typed => {
                        let saved = SavedBasic {
                            header: header.clone(),
                            service_uid: uid.clone(),
                        };
                        if let Some(key) = keys.first()
                            && let Err(err) =
                                write_secret(&credentials, key.clone(), saved.to_secret()).await
                        {
                            tracing::warn!("Prometheus credentials not stored: {err}");
                        }
                    }
                    Err(Unlock::Rejected) if !typed => delete_keys(credentials.clone(), keys).await,
                    Err(Unlock::Replaced) => delete_keys(credentials.clone(), keys).await,
                    _ => {}
                }
                result.map(|(signed, _, _)| signed)
            }
        };
        let (cluster, id) = (cluster.clone(), id.to_string());
        host.spawn(work, move |this, result, host| {
            this.sessions += 1;
            let session = this.sessions;
            this.forwards += 1;
            let token = this.forwards;
            let state = this
                .clusters
                .get_mut(&cluster)
                .filter(|s| s.generation == generation);
            let instance = state.and_then(|state| {
                let instance = state.instances.iter_mut().find(|i| i.id == id)?;
                Some((instance, &mut state.forwards))
            });
            // Without the server a successful sign-in is dropped, and with it its forward.
            let Some((instance, forwards)) = instance else {
                return;
            };
            instance.access = match result {
                Ok(Signed {
                    client,
                    mut reached,
                }) => {
                    instance.client = client;
                    instance.session = session;
                    let stopped = reached.take_stopped();
                    let (watch_cluster, watch_id) = (cluster.clone(), id.clone());
                    // Replacing an older forward of the server stops it.
                    forwards.insert(
                        id.clone(),
                        Forward {
                            token,
                            _reached: reached,
                        },
                    );
                    host.spawn(stopped, move |this, (), host| {
                        this.forward_stopped(&watch_cluster, &watch_id, token, host)
                    })
                    .detach();
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
            host.notify();
        })
        .detach();
    }

    fn delete_secrets(&self, keys: Vec<String>, host: &mut dyn Host<Self>) {
        let credentials = self.credentials.clone();
        host.spawn(delete_keys(credentials, keys), |_, (), _| {})
            .detach();
    }
}

/// Reads the saved credentials (or takes `header`), opens a loopback forward to the Service and
/// checks that the server takes the header there.
#[allow(clippy::too_many_arguments)]
async fn sign_in_to(
    reach: &dyn Reach,
    credentials: &Arc<dyn Credentials>,
    cluster: &ClusterId,
    id: &str,
    target: &Target,
    client: kube::Client,
    header: Option<SecretString>,
    keys: &[String],
) -> Result<(Signed, SecretString, String), Unlock> {
    let uid = service_uid(&client, target).await?;
    let header = match header {
        Some(header) => header,
        None => {
            let saved = read_saved(credentials, keys)
                .await
                .ok_or(Unlock::NoCredentials)?;
            // Never to a Service that replaced the one the user signed in to.
            if saved.service_uid != uid {
                return Err(Unlock::Replaced);
            }
            saved.header
        }
    };
    let signed = connect(reach, cluster, id, target, client, header.clone()).await?;
    Ok((signed, header, uid))
}

/// The UID of the Service behind `target`.
async fn service_uid(client: &kube::Client, target: &Target) -> Result<String, Unlock> {
    let Target::Service {
        namespace, service, ..
    } = target
    else {
        return Err(Unlock::Failed("Only a Service can be signed in to.".into()));
    };
    basic_auth::service_uid(client, namespace, service)
        .await
        .map_err(Unlock::Failed)
}

/// Opens a loopback forward to `target` and checks that the server takes `header` there.
async fn connect(
    reach: &dyn Reach,
    cluster: &ClusterId,
    id: &str,
    target: &Target,
    client: kube::Client,
    header: SecretString,
) -> Result<Signed, Unlock> {
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
    let reached = reach
        .reach(ReachRequest {
            client,
            cluster: cluster.clone(),
            namespace: namespace.clone(),
            service: service.clone(),
            port: ReachPort::Number(number),
            title: format!("Prometheus · svc/{service}"),
            https: false,
            timeout: FORWARD_TIMEOUT,
            reconnect_grace: None,
        })
        .await
        .map_err(|err| Unlock::Failed(reach_problem(err)))?;
    // TLS ends in the pod, behind the API server's tunnel: its certificate is for the Service's
    // name, not 127.0.0.1.
    let https = scheme == "https";
    let url = format!(
        "{}://127.0.0.1:{}{}",
        if https { "https" } else { "http" },
        reached.local_port,
        kubyl_metrics_core::transport::normalize_prefix(path)
    );
    let tls = ExternalTls {
        insecure: https,
        ..Default::default()
    };
    let probed = async {
        let transport = Transport::external(&url, Some(&header), &tls)?;
        let prom = PromClient::from_transport(transport, target.clone());
        prom.probe(PROBE_TIMEOUT).await.map(|()| prom)
    }
    .await;
    // A server that doesn't take the header drops `reached`, and the forward with it.
    match probed {
        Ok(client) => Ok(Signed { client, reached }),
        Err(PromError::Http(401, _)) => Err(Unlock::Rejected),
        Err(err) => Err(Unlock::Failed(err.to_string())),
    }
}

/// The reason a forward didn't open, as the user reads it.
fn reach_problem(err: String) -> String {
    match err.as_str() {
        "the port-forward didn't start (needs create pods/portforward)" => {
            "The port-forward didn't start (needs create pods/portforward).".into()
        }
        "the port-forward stopped" => "The port-forward stopped.".into(),
        _ => err,
    }
}

/// The first readable entry of `keys`.
async fn read_saved(credentials: &Arc<dyn Credentials>, keys: &[String]) -> Option<SavedBasic> {
    let (credentials, keys) = (credentials.clone(), keys.to_vec());
    tokio::task::spawn_blocking(move || {
        keys.iter().find_map(|key| {
            credentials
                .get(key)
                .inspect_err(|e| tracing::warn!("keychain: {e}"))
                .ok()
                .flatten()
                .and_then(|secret| SavedBasic::parse(&secret))
        })
    })
    .await
    .ok()
    .flatten()
}

async fn write_secret(
    credentials: &Arc<dyn Credentials>,
    key: String,
    secret: SecretString,
) -> Result<(), String> {
    let credentials = credentials.clone();
    tokio::task::spawn_blocking(move || credentials.set(&key, &secret))
        .await
        .map_err(|e| e.to_string())?
}

async fn delete_keys(credentials: Arc<dyn Credentials>, keys: Vec<String>) {
    tokio::task::spawn_blocking(move || {
        for key in keys {
            credentials.delete(&key).ok();
        }
    })
    .await
    .ok();
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::net::SocketAddr;
    use std::sync::Mutex;

    use futures::future::BoxFuture;
    use kubyl_base::host::TestHost;
    use kubyl_portforward_core::reach::FakeReached;
    use secrecy::ExposeSecret as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    const PROM_OK: &str = r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#;

    struct Request {
        path: String,
        authorization: Option<String>,
    }

    /// A tiny HTTP server: `answer` gets the path and the Authorization header and returns the
    /// status and body.
    fn serve(
        runtime: &tokio::runtime::Runtime,
        answer: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static,
    ) -> SocketAddr {
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let address = listener.local_addr().unwrap();
        let answer = Arc::new(answer);
        runtime.spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let answer = answer.clone();
                tokio::spawn(async move {
                    let mut buffer = vec![0; 8192];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let text = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let path = text
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .split('?')
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    let authorization = text.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then(|| value.trim().to_string())
                    });
                    let (status, body) = answer(&Request {
                        path,
                        authorization,
                    });
                    let challenge = if status == 401 {
                        "www-authenticate: Basic realm=\"prom\"\r\n"
                    } else {
                        ""
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X\r\n{challenge}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(response.as_bytes()).await.ok();
                });
            }
        });
        address
    }

    #[derive(Default)]
    struct FakeCredentials(Mutex<HashMap<String, String>>);

    impl FakeCredentials {
        fn put(&self, key: &str, secret: SecretString) {
            self.0
                .lock()
                .unwrap()
                .insert(key.into(), secret.expose_secret().to_string());
        }

        fn has(&self, key: &str) -> bool {
            self.0.lock().unwrap().contains_key(key)
        }
    }

    impl Credentials for FakeCredentials {
        fn get(&self, key: &str) -> Result<Option<SecretString>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(key)
                .map(|s| SecretString::from(s.clone())))
        }

        fn set(&self, key: &str, secret: &SecretString) -> Result<(), String> {
            self.put(key, secret.clone());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// Answers each request with the next canned result: a port to reach or a problem.
    #[derive(Default)]
    struct FakeReach {
        answers: Mutex<VecDeque<Result<u16, String>>>,
        requests: Mutex<Vec<ReachRequest>>,
        forwards: Mutex<Vec<FakeReached>>,
    }

    impl FakeReach {
        fn answer(&self, answer: Result<u16, String>) {
            self.answers.lock().unwrap().push_back(answer);
        }

        fn end(&self, index: usize) {
            self.forwards.lock().unwrap()[index].end();
        }

        fn released(&self, index: usize) -> bool {
            self.forwards.lock().unwrap()[index].released()
        }
    }

    impl Reach for FakeReach {
        fn reach(&self, request: ReachRequest) -> BoxFuture<'static, Result<Reached, String>> {
            self.requests.lock().unwrap().push(request);
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("no answer".into()));
            let reached = answer.map(|port| {
                let (reached, fake) = Reached::fake(port);
                self.forwards.lock().unwrap().push(fake);
                reached
            });
            Box::pin(async move { reached })
        }
    }

    struct Fixture {
        /// Runs the fake servers and the clients talking to them.
        _runtime: tokio::runtime::Runtime,
        client: kube::Client,
        reach: Arc<FakeReach>,
        credentials: Arc<FakeCredentials>,
        core: PrometheusCore,
        host: TestHost<PrometheusCore>,
        cluster: ClusterId,
        /// The Prometheus behind the forward: takes `admin:s3cr3t`.
        prom: SocketAddr,
    }

    const ID: &str = "monitoring/prometheus-k8s";

    impl Fixture {
        fn new() -> Self {
            Self::with_api(|request| match request.path.as_str() {
                "/api/v1/namespaces/monitoring/services/prometheus-k8s" => {
                    (200, r#"{"metadata":{"uid":"uid-1"}}"#.into())
                }
                _ => (200, r#"{"items":[]}"#.into()),
            })
        }

        fn with_api(api: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> Self {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let api = serve(&runtime, api);
            let prom = serve(&runtime, |request| {
                if request.authorization.as_deref() == Some("Basic YWRtaW46czNjcjN0") {
                    (200, PROM_OK.into())
                } else {
                    (401, "Unauthorized".into())
                }
            });
            let client = {
                let _guard = runtime.enter();
                kube::Client::try_from(kube::Config::new(format!("http://{api}").parse().unwrap()))
                    .unwrap()
            };
            let reach = Arc::new(FakeReach::default());
            let credentials = Arc::new(FakeCredentials::default());
            Self {
                _runtime: runtime,
                client,
                core: PrometheusCore::new(reach.clone(), credentials.clone()),
                reach,
                credentials,
                host: TestHost::new(),
                cluster: ClusterId::new("c"),
                prom,
            }
        }

        fn conn(&self) -> ClusterConn {
            ClusterConn {
                client: Some(self.client.clone()),
                settings_keys: vec!["c".into()],
            }
        }

        fn discovery(&self, disabled: bool, metrics: MetricsInfo) -> Conns {
            vec![(
                self.cluster.clone(),
                Discovery {
                    conn: self.conn(),
                    disabled,
                    metrics,
                },
            )]
        }

        fn tick(&mut self, conns: &Conns) {
            self.core.tick(Some(conns), &mut self.host);
        }

        /// A discovered cluster with a locked Prometheus Service.
        fn with_locked_server(mut self) -> Self {
            let target = Target::service("monitoring", "prometheus-k8s", "9090");
            let mut state = ClusterState::new(self.core.generation);
            state.phase = Phase::Ready;
            state.instances = vec![Instance::locked(PromClient::new(
                self.client.clone(),
                target,
            ))];
            self.core.clusters.insert(self.cluster.clone(), state);
            self
        }

        fn access(&self) -> Access {
            self.core.instances(&self.cluster)[0].access.clone()
        }

        fn sign_in(&mut self, password: &str) {
            let conn = self.conn();
            self.core.sign_in(
                &self.cluster,
                ID,
                "admin",
                &SecretString::from(password.to_string()),
                conn,
                &mut self.host,
            );
        }

        fn run_until_settled(&mut self) {
            let cluster = self.cluster.clone();
            self.host.run_until(&mut self.core, |core, _| {
                !matches!(core.instances(&cluster)[0].access, Access::Unlocking)
            });
        }

        /// Runs the callbacks of everything that finished by now (the watchers of forwards
        /// never end, so the host is never idle).
        fn pump(&mut self) {
            let pumped = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = pumped.clone();
            let _timer = self.host.spawn(
                async { tokio::time::sleep(Duration::from_millis(300)).await },
                move |_, (), _| flag.store(true, std::sync::atomic::Ordering::SeqCst),
            );
            self.host.run_until(&mut self.core, |_, _| {
                pumped.load(std::sync::atomic::Ordering::SeqCst)
            });
        }

        fn metrics_client(&self) -> PromClient {
            let _guard = self._runtime.enter();
            PromClient::from_transport(
                Transport::external(
                    &format!("http://{}", self.prom),
                    None,
                    &ExternalTls::default(),
                )
                .unwrap(),
                Target::Url {
                    url: format!("http://{}", self.prom),
                    insecure: false,
                },
            )
        }
    }

    fn locked(saved: bool, problem: Option<&str>) -> Access {
        Access::Locked {
            saved,
            problem: problem.map(Into::into),
        }
    }

    #[test]
    fn an_empty_cluster_has_no_source_and_a_metrics_client_is_the_first_instance() {
        let mut f = Fixture::new();
        let conns = f.discovery(false, MetricsInfo::default());
        f.tick(&conns);
        assert_eq!(f.core.phase(&f.cluster), Phase::Discovering);
        assert_eq!(f.core.has_instances(&f.cluster), None);
        f.host.run_until_idle(&mut f.core);
        assert!(matches!(f.core.phase(&f.cluster), Phase::NoSource(_)));
        assert_eq!(f.core.has_instances(&f.cluster), Some(false));
        assert!(f.host.effects.contains(&PrometheusEffect::Changed));

        // Redetecting forgets that and looks again, now with the metrics service's client.
        let metrics = MetricsInfo {
            client: Some(f.metrics_client()),
            pending: false,
        };
        let conns = f.discovery(false, metrics);
        let cluster = f.cluster.clone();
        f.core.redetect(&cluster, Some(&conns), &mut f.host);
        assert_eq!(f.core.phase(&cluster), Phase::Discovering);
        f.host.run_until_idle(&mut f.core);
        assert_eq!(f.core.phase(&cluster), Phase::Ready);
        assert_eq!(f.core.has_instances(&cluster), Some(true));
        let instances = f.core.instances(&cluster);
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].access, Access::Open);
    }

    #[test]
    fn disabled_clusters_are_not_looked_at_until_enabled() {
        let mut f = Fixture::new();
        let conns = f.discovery(true, MetricsInfo::default());
        f.tick(&conns);
        assert_eq!(f.core.phase(&f.cluster), Phase::Disabled);
        assert_eq!(f.core.has_instances(&f.cluster), Some(false));
        f.host.run_until_idle(&mut f.core);
        assert_eq!(f.core.phase(&f.cluster), Phase::Disabled);

        let conns = f.discovery(false, MetricsInfo::default());
        f.tick(&conns);
        assert_eq!(f.core.phase(&f.cluster), Phase::Discovering);
        f.host.run_until_idle(&mut f.core);
        assert!(matches!(f.core.phase(&f.cluster), Phase::NoSource(_)));
    }

    #[test]
    fn discovery_waits_for_the_metrics_service() {
        let mut f = Fixture::new();
        let pending = MetricsInfo {
            client: None,
            pending: true,
        };
        let conns = f.discovery(false, pending);
        f.tick(&conns);
        // Nothing was started: the metrics service gets METRICS_WAIT to finish detecting.
        f.host.run_until_idle(&mut f.core);
        assert_eq!(f.core.phase(&f.cluster), Phase::Discovering);
        assert!(f.core.instances(&f.cluster).is_empty());
    }

    #[test]
    fn a_disconnected_cluster_is_forgotten_and_late_answers_are_ignored() {
        let mut f = Fixture::new();
        let conns = f.discovery(false, MetricsInfo::default());
        f.tick(&conns);
        let cluster = f.cluster.clone();
        f.core.connection_changed(&cluster, false, &mut f.host);
        assert_eq!(f.core.phase(&cluster), Phase::Unknown);
        f.host.run_until_idle(&mut f.core);
        assert_eq!(f.core.phase(&cluster), Phase::Unknown);

        // Gone from the connections: same.
        f.tick(&conns);
        f.tick(&Vec::new());
        f.host.run_until_idle(&mut f.core);
        assert_eq!(f.core.phase(&cluster), Phase::Unknown);
        // No connection manager: nothing happens.
        f.core.tick(None, &mut f.host);
    }

    #[test]
    fn rekeyed_clusters_start_over_under_the_new_id() {
        let mut f = Fixture::new().with_locked_server();
        let (from, to) = (f.cluster.clone(), ClusterId::new("d"));
        f.core.rekeyed(&from, &to, &mut f.host);
        assert_eq!(f.core.phase(&from), Phase::Unknown);
        assert_eq!(f.core.phase(&to), Phase::Unknown);
        assert!(f.core.clusters.contains_key(&to));
        // Unknown clusters stay as they are.
        f.core.rekeyed(&from, &ClusterId::new("e"), &mut f.host);
        assert!(!f.core.clusters.contains_key(&ClusterId::new("e")));
    }

    #[test]
    fn signing_in_keeps_the_credentials_and_reads_through_a_forward() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Ok(f.prom.port()));
        f.sign_in("s3cr3t");
        assert_eq!(f.access(), Access::Unlocking);
        f.run_until_settled();
        assert_eq!(f.access(), Access::SignedIn);
        let instance = &f.core.instances(&f.cluster)[0];
        assert_eq!(instance.session, 1);
        assert!(instance.is_readable());
        // The client reads through the loopback forward from now on.
        assert!(matches!(instance.client.target(), Target::Service { .. }));
        let request = f.reach.requests.lock().unwrap()[0].clone();
        assert_eq!(request.namespace, "monitoring");
        assert_eq!(request.service, "prometheus-k8s");
        assert_eq!(request.port, ReachPort::Number(9090));
        assert_eq!(request.title, "Prometheus · svc/prometheus-k8s");
        assert_eq!(request.timeout, FORWARD_TIMEOUT);
        assert!(
            f.credentials
                .has("prometheus-auth:c/monitoring/prometheus-k8s")
        );
        assert!(!f.reach.released(0));
    }

    #[test]
    fn a_rejected_password_is_not_kept_and_stops_the_forward() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Ok(f.prom.port()));
        f.sign_in("wrong");
        f.run_until_settled();
        assert_eq!(
            f.access(),
            locked(
                false,
                Some("The server rejected this username and password.")
            )
        );
        assert!(
            !f.credentials
                .has("prometheus-auth:c/monitoring/prometheus-k8s")
        );
        assert!(f.reach.released(0));
    }

    #[test]
    fn the_user_stopping_the_forward_locks_the_server_again() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Ok(f.prom.port()));
        f.sign_in("s3cr3t");
        f.run_until_settled();
        assert_eq!(f.access(), Access::SignedIn);

        f.reach.end(0);
        let cluster = f.cluster.clone();
        f.host.run_until(&mut f.core, |core, _| {
            core.instances(&cluster)[0].access != Access::SignedIn
        });
        assert_eq!(
            f.access(),
            locked(true, Some("The port-forward was stopped."))
        );
        // The keychain keeps the credentials for "reconnect".
        assert!(
            f.credentials
                .has("prometheus-auth:c/monitoring/prometheus-k8s")
        );

        f.reach.answer(Ok(f.prom.port()));
        let conn = f.conn();
        f.core.reconnect(&cluster, ID, conn, &mut f.host);
        assert_eq!(f.access(), Access::Unlocking);
        f.run_until_settled();
        assert_eq!(f.access(), Access::SignedIn);
        assert_eq!(f.core.instances(&cluster)[0].session, 2);
    }

    #[test]
    fn a_locked_server_found_by_discovery_signs_in_with_the_saved_credentials() {
        let mut f = Fixture::with_api(|request| match request.path.as_str() {
            "/api/v1/services" => (
                200,
                r#"{"items":[{"metadata":{"name":"prometheus-k8s","namespace":"monitoring"},
                    "spec":{"ports":[{"name":"web","port":9090}]}}]}"#
                    .into(),
            ),
            "/api/v1/namespaces/monitoring/services/prometheus-k8s" => {
                (200, r#"{"metadata":{"uid":"uid-1"}}"#.into())
            }
            // The Service behind the API server's proxy wants a password.
            path if path.contains("/proxy/") => (401, "Unauthorized".into()),
            _ => (404, "{}".into()),
        });
        let saved = SavedBasic {
            header: SecretString::from("Basic YWRtaW46czNjcjN0".to_string()),
            service_uid: "uid-1".into(),
        };
        f.credentials.put(
            "prometheus-auth:c/monitoring/prometheus-k8s",
            saved.to_secret(),
        );
        f.reach.answer(Ok(f.prom.port()));
        let conns = f.discovery(false, MetricsInfo::default());
        f.tick(&conns);
        let cluster = f.cluster.clone();
        f.host.run_until(&mut f.core, |core, _| {
            core.instances(&cluster)
                .first()
                .is_some_and(|i| i.access == Access::SignedIn)
        });
        assert_eq!(f.core.phase(&cluster), Phase::Ready);
        assert_eq!(f.core.instances(&cluster)[0].id, ID);
    }

    #[test]
    fn credentials_of_a_replaced_service_are_deleted() {
        let mut f = Fixture::new().with_locked_server();
        let key = "prometheus-auth:c/monitoring/prometheus-k8s";
        let saved = SavedBasic {
            header: SecretString::from("Basic YWRtaW46czNjcjN0".to_string()),
            service_uid: "another-uid".into(),
        };
        f.credentials.put(key, saved.to_secret());
        let (cluster, conn) = (f.cluster.clone(), f.conn());
        f.core.reconnect(&cluster, ID, conn, &mut f.host);
        f.run_until_settled();
        assert_eq!(
            f.access(),
            locked(
                false,
                Some(
                    "The Service was re-created since you signed in. Check that it's still your \
                     Prometheus, then sign in again."
                )
            )
        );
        assert!(!f.credentials.has(key));
        assert!(f.reach.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn without_saved_credentials_the_server_stays_locked() {
        let mut f = Fixture::new().with_locked_server();
        let (cluster, conn) = (f.cluster.clone(), f.conn());
        f.core.reconnect(&cluster, ID, conn, &mut f.host);
        f.run_until_settled();
        assert_eq!(f.access(), locked(false, None));
        assert!(f.reach.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn forwards_that_fail_say_why() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Err("port-forward: forbidden".into()));
        f.sign_in("s3cr3t");
        f.run_until_settled();
        assert_eq!(f.access(), locked(false, Some("port-forward: forbidden")));

        f.reach.answer(Err(
            "the port-forward didn't start (needs create pods/portforward)".into(),
        ));
        f.sign_in("s3cr3t");
        f.run_until_settled();
        assert_eq!(
            f.access(),
            locked(
                false,
                Some("The port-forward didn't start (needs create pods/portforward).")
            )
        );
        assert!(
            !f.credentials
                .has("prometheus-auth:c/monitoring/prometheus-k8s")
        );
    }

    #[test]
    fn a_disconnected_cluster_cannot_be_signed_in_to() {
        let mut f = Fixture::new().with_locked_server();
        let mut conn = f.conn();
        conn.client = None;
        let cluster = f.cluster.clone();
        f.core.reconnect(&cluster, ID, conn, &mut f.host);
        assert_eq!(
            f.access(),
            locked(false, Some("The cluster isn't connected."))
        );
        f.host.run_until_idle(&mut f.core);
    }

    #[test]
    fn a_sign_in_that_finishes_after_the_cluster_went_stops_its_forward() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Ok(f.prom.port()));
        f.sign_in("s3cr3t");
        let cluster = f.cluster.clone();
        f.core.connection_changed(&cluster, false, &mut f.host);
        f.host.run_until_idle(&mut f.core);
        assert_eq!(f.core.phase(&cluster), Phase::Unknown);
        assert!(f.reach.released(0));
    }

    #[test]
    fn a_refused_read_locks_the_server_and_forgets_the_credentials() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Ok(f.prom.port()));
        f.sign_in("s3cr3t");
        f.run_until_settled();
        let key = "prometheus-auth:c/monitoring/prometheus-k8s";
        assert!(f.credentials.has(key));
        let (cluster, conn) = (f.cluster.clone(), f.conn());

        // An answer to older credentials changes nothing.
        f.core.rejected(&cluster, ID, 99, &conn, &mut f.host);
        assert_eq!(f.access(), Access::SignedIn);

        f.core.rejected(&cluster, ID, 1, &conn, &mut f.host);
        assert_eq!(
            f.access(),
            locked(
                false,
                Some("The server no longer takes the saved username and password.")
            )
        );
        assert!(f.reach.released(0));
        // The keychain is cleaned in the background.
        f.pump();
        assert!(!f.credentials.has(key));
    }

    #[test]
    fn a_new_sign_in_replaces_the_forward_of_the_old_one() {
        let mut f = Fixture::new().with_locked_server();
        f.reach.answer(Ok(f.prom.port()));
        f.sign_in("s3cr3t");
        f.run_until_settled();
        // Signed in servers can sign in again (the view shows "reconnect" only when locked, but
        // the service doesn't mind).
        f.reach.answer(Ok(f.prom.port()));
        let (cluster, conn) = (f.cluster.clone(), f.conn());
        f.core.reconnect(&cluster, ID, conn, &mut f.host);
        f.run_until_settled();
        assert_eq!(f.access(), Access::SignedIn);
        assert!(f.reach.released(0));
        assert!(!f.reach.released(1));
        // Ending the old one is no stop by the user.
        f.reach.end(0);
        f.pump();
        assert_eq!(f.access(), Access::SignedIn);
    }
}
