//! [`UpdatesCore`]: the update state of every cluster the app looks at, without the UI. Per
//! cluster it detects the provider, reads it while a view (or another crate) wants it (every
//! 15 s, every 5 s while an update runs), runs pre-flight checks and the writes that outlive a
//! dialog. It runs on any [`Host`]; what it needs from the app (connections, Helm and
//! Prometheus state) comes in as snapshots and [`UpdatesEffect`]s.
//!
//! Cluster facts come from the connection manager's core ([`ClusterConn::from_manager`]).

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kubyl_base::ClusterId;
use kubyl_base::host::{Flow, Host, HostExt as _, Service, TaskHandle};
use kubyl_base::notice::Notice;
use kubyl_kube_core::cluster_info::Distribution;
use kubyl_kube_core::discovery::Discovery;
use kubyl_kube_core::manager::ManagerCore;

use crate::capi;
use crate::check::{self, Check};
use crate::detect::{self, Detected, Facts};
use crate::fallback::ReadOnly;
use crate::model::{Note, Plan, ProviderKind, Status};
use crate::preflight::{self, Inputs};
use crate::provider::{ProviderError, UpdateProvider};
use crate::providers;
use crate::settings::{ProviderSetting, UpdatesSettings};

/// Read interval while nothing runs.
pub const IDLE: Duration = Duration::from_secs(15);
/// Read interval while an update runs.
pub const UPDATING: Duration = Duration::from_secs(5);
/// How long a cluster stays after the last lease went.
pub const KEEP: Duration = Duration::from_secs(60);
/// How long a pre-flight run waits for the Helm releases to list.
pub const HELM_WAIT: Duration = Duration::from_secs(15);

/// Poll interval of a pre-flight run waiting for the Helm releases and Prometheus detection.
pub const PREFLIGHT_POLL: Duration = Duration::from_millis(300);

/// A read's outcome.
#[derive(Clone, Debug)]
pub enum ReadState {
    Loading,
    Ready(Arc<Status>),
    Failed(ProviderError),
}

/// What a view shows for a cluster.
#[derive(Clone, Debug)]
pub enum UpdateState {
    NotConnected,
    /// Discovery hasn't finished: the provider isn't known yet.
    Detecting,
    Known {
        detected: Detected,
        read: ReadState,
        /// The last good read while a newer one failed.
        last: Option<Arc<Status>>,
        fetched_at: Option<Instant>,
    },
}

/// A pre-flight run for one target.
#[derive(Clone, Debug)]
pub struct Preflight {
    pub target: String,
    /// The version it ran against (a new version runs it again).
    pub current: String,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub checks: Vec<Check>,
}

impl Preflight {
    pub fn running(&self) -> bool {
        self.finished.is_none()
    }
}

/// The facts detection looks at, if the cluster is connected and discovered.
pub fn facts(manager: &ManagerCore, cluster: &ClusterId) -> Option<Facts> {
    let connection = manager.cluster(cluster)?;
    let discovery = connection.discovery.as_ref()?;
    let context = manager.context(cluster);
    let exec_command = context.and_then(|c| match &c.auth {
        kubyl_kube_core::auth::AuthMethod::Exec(exec) => Some(exec.command.clone()),
        _ => None,
    });
    Some(
        Facts {
            git_version: connection
                .info
                .as_ref()
                .map(|i| i.version.clone())
                .unwrap_or_default(),
            server: context.and_then(|c| c.server.clone()).unwrap_or_default(),
            names: context
                .map(|c| format!("{} {}", c.context, c.cluster))
                .unwrap_or_default(),
            exec_command,
            ..Facts::default()
        }
        .with_discovery(discovery),
    )
}

/// What the updates service needs to know about a cluster's connection, copied out of the
/// connection manager by the app (a snapshot: the service never reads the manager itself).
#[derive(Clone)]
pub struct ClusterConn {
    /// The user marked the cluster read-only (holds while it is disconnected).
    pub read_only: bool,
    /// Present while the cluster is connected and discovered.
    pub live: Option<Live>,
}

/// A connected, discovered cluster.
#[derive(Clone)]
pub struct Live {
    pub client: kube::Client,
    pub facts: Facts,
    pub display_name: String,
    pub distribution: Option<Distribution>,
    pub capi: capi::CapiVersions,
    /// Where a cloud provider finds the cluster, with the cluster's settings overrides.
    pub cloud: Option<providers::CloudContext>,
    /// The provider picked in settings (`updates.clusters.<cluster>.provider`).
    pub provider_override: Option<ProviderSetting>,
}

/// Connection snapshots by cluster.
pub type Conns = HashMap<ClusterId, ClusterConn>;

impl ClusterConn {
    /// Copies the facts about `cluster` out of the manager.
    pub fn from_manager(
        manager: &ManagerCore,
        cluster: &ClusterId,
        settings: &UpdatesSettings,
    ) -> Self {
        let read_only = manager.caps(cluster).read_only;
        let live = (|| {
            let client = manager.client(cluster)?;
            let facts = facts(manager, cluster)?;
            let overrides = settings.for_keys(&manager.settings_keys(cluster));
            Some(Live {
                client,
                facts,
                display_name: manager.display_name(cluster),
                distribution: manager
                    .cluster(cluster)
                    .and_then(|c| c.info.as_ref())
                    .map(|i| i.distribution),
                capi: capi_versions(manager.discovery(cluster).as_deref()),
                provider_override: overrides.provider,
                cloud: manager
                    .context(cluster)
                    .map(|context| providers::CloudContext {
                        display_name: manager.display_name(cluster),
                        kubeconfig: context.file.clone(),
                        context: context.context.clone(),
                        user: context.user.clone(),
                        cluster_entry: context.cluster.clone(),
                        server: context.server.clone().unwrap_or_default(),
                        settings: overrides,
                    }),
            })
        })();
        Self { read_only, live }
    }
}

/// Decides which provider a cluster gets: the settings override, else detection.
pub fn detect_provider(live: &Live) -> Detected {
    match live.provider_override {
        Some(setting) => Detected {
            kind: setting.kind(),
            reason: "picked in settings (updates.clusters.<cluster>.provider)".into(),
        },
        None => detect::detect(&live.facts),
    }
}

/// Builds the provider of a detected kind.
pub fn build(live: &Live, detected: &Detected) -> Arc<dyn UpdateProvider> {
    let facts = &live.facts;
    let client = live.client.clone();
    let read_only = |label: &str, reason: String, notes: Vec<Note>| -> Arc<dyn UpdateProvider> {
        Arc::new(ReadOnly::new(
            client.clone(),
            detected.kind,
            label,
            reason,
            notes,
        ))
    };
    match detected.kind {
        ProviderKind::OpenShift => Arc::new(crate::openshift::OpenShift::new(client.clone())),
        ProviderKind::K3s | ProviderKind::Rke2 => {
            if facts.suc_plans {
                Arc::new(crate::suc::Suc::new(client.clone(), detected.kind))
            } else {
                read_only(
                    detected.kind.label(),
                    "Kubyl updates k3s and RKE2 through system-upgrade-controller's Plans, which this cluster doesn't have.".into(),
                    vec![Note {
                        warning: false,
                        title: "system-upgrade-controller isn't installed".into(),
                        text: "Install it to update this cluster from Kubyl with Plans.".into(),
                        command: None,
                        url: Some(if detected.kind == ProviderKind::K3s {
                            "https://docs.k3s.io/upgrades/automated".into()
                        } else {
                            "https://docs.rke2.io/upgrades/automated_upgrade".into()
                        }),
                    }],
                )
            }
        }
        ProviderKind::ClusterApi => Arc::new(crate::capi::ClusterApi::new(
            client.clone(),
            live.capi.clone(),
        )),
        kind if kind.is_cloud() => {
            if let Some(provider) = cloud(kind, live.cloud.as_ref()) {
                provider
            } else {
                let feature = providers::feature_of(kind).unwrap_or_default();
                read_only(
                    kind.label(),
                    format!("This build doesn't include the {} provider.", kind.label()),
                    vec![Note {
                        warning: true,
                        title: format!("This build doesn't include the {} provider", kind.label()),
                        text: format!(
                            "Kubyl was built without the {feature} feature: the version, nodes and pre-flight checks still work, updates don't."
                        ),
                        command: None,
                        url: None,
                    }],
                )
            }
        }
        _ => read_only(
            detect::self_managed_label(live.distribution, facts),
            "Kubyl doesn't know how this cluster was installed, so it can't update it. Update it with the tool that installed it.".into(),
            Vec::new(),
        ),
    }
}

/// The Cluster API versions the cluster serves (preferred).
pub fn capi_versions(discovery: Option<&Discovery>) -> capi::CapiVersions {
    let version = |group: &str, resource: &str| {
        discovery.and_then(|d| {
            d.preferred()
                .find(|r| r.gvr.group == group && r.gvr.resource == resource)
                .map(|r| r.gvr.version.clone())
        })
    };
    let defaults = capi::CapiVersions::default();
    capi::CapiVersions {
        cluster: version("cluster.x-k8s.io", "clusters").unwrap_or(defaults.cluster),
        control_plane: version("controlplane.cluster.x-k8s.io", "kubeadmcontrolplanes"),
    }
}

/// The cloud provider, when this build has it.
#[allow(unused_variables)]
pub fn cloud(
    kind: ProviderKind,
    context: Option<&providers::CloudContext>,
) -> Option<Arc<dyn UpdateProvider>> {
    if !providers::cloud_built(kind) {
        return None;
    }
    let context = context?.clone();
    match kind {
        #[cfg(feature = "updates-eks")]
        ProviderKind::Eks => Some(providers::eks::provider(context)),
        #[cfg(feature = "updates-gke")]
        ProviderKind::Gke => Some(providers::gke::provider(context)),
        #[cfg(feature = "updates-aks")]
        ProviderKind::Aks => Some(providers::aks::provider(context)),
        _ => {
            let _ = context;
            None
        }
    }
}

/// A cloud read that failed for want of credentials or the cloud API: the read-only view
/// takes over with a note.
pub fn cloud_unavailable(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::Credentials { .. }
            | ProviderError::Unavailable(_)
            | ProviderError::Forbidden { .. }
    )
}

pub type NoteFn = Box<dyn Fn(&ProviderError) -> Note + Send>;

/// For cloud providers: the read-only provider to fall back to and the note it gets.
pub fn fallback_for(
    client: Option<kube::Client>,
    provider: &Arc<dyn UpdateProvider>,
) -> Option<(ReadOnly, NoteFn)> {
    let kind = provider.kind();
    if !kind.is_cloud() {
        return None;
    }
    let client = client?;
    let label = kind.label();
    let fallback = ReadOnly::new(
        client,
        kind,
        label,
        format!("The {label} API isn't available."),
        Vec::new(),
    );
    let note: NoteFn = Box::new(move |err: &ProviderError| {
        let (title, command) = match err {
            ProviderError::Credentials { command, .. } => (
                format!("{label} credentials aren't available"),
                command.clone(),
            ),
            ProviderError::Forbidden { .. } => (format!("The {label} API denied access"), None),
            _ => (format!("The {label} API isn't available"), None),
        };
        Note {
            warning: true,
            title,
            text: format!("{err} Kubernetes-side checks still run."),
            command,
            url: None,
        }
    });
    Some((fallback, note))
}

/// Keeps a cluster's reads going while held (views hold one).
#[derive(Clone)]
pub struct UpdatesLease(#[allow(dead_code)] Arc<()>);

/// What the app does for the service. Each variant that names a method asks the app to call it
/// right away with fresh snapshots (the service itself never reads the app's state).
#[derive(Debug)]
pub enum UpdatesEffect {
    /// Call [`UpdatesCore::read`] for the cluster.
    Read(ClusterId),
    /// Look at the app's Helm and Prometheus state and call [`UpdatesCore::preflight_probe`]:
    /// [`Probe::Go`] once the Helm releases listed and Prometheus detection finished (or
    /// [`UpdatesCore::preflight_overdue`] says to stop waiting), else [`Probe::Waiting`].
    ProbePreflight {
        run: u64,
        cluster: ClusterId,
        kind: ProviderKind,
        target: String,
    },
    /// Keep the cluster's Helm releases watched (pre-flight reads them).
    WatchHelm(ClusterId),
    /// Nobody looks at the cluster any more: drop the Helm watch.
    ReleaseHelm(ClusterId),
}

/// The app's answer to [`UpdatesEffect::ProbePreflight`].
pub enum Probe {
    /// Not ready yet; ask again.
    Waiting,
    /// Run the checks with these inputs (`None`: the cluster went away meanwhile).
    Go(Option<Box<Inputs>>),
}

struct ClusterUpdates {
    detected: Option<Detected>,
    /// The facts the provider was built from (a change rebuilds it).
    facts: Option<Facts>,
    provider: Option<Arc<dyn UpdateProvider>>,
    read: ReadState,
    last: Option<Arc<Status>>,
    fetched_at: Option<Instant>,
    reading: bool,
    /// Bumped when the provider is rebuilt; older reads are dropped.
    generation: u64,
    lease: Arc<()>,
    wanted_until: Instant,
    preflight: HashMap<String, Preflight>,
    busy: Option<String>,
    /// Which write `busy` belongs to (the state moves when the cluster is rekeyed, so a write
    /// finds it again by this and not by the id it started under).
    busy_token: u64,
    /// Reads, pre-flight runs, the poll loop, each under the token its callback reports with.
    tasks: Vec<(u64, TaskHandle)>,
    /// The poll loop runs (it ends by itself).
    polling: bool,
}

impl ClusterUpdates {
    fn new(lease: Arc<()>) -> Self {
        Self {
            detected: None,
            facts: None,
            provider: None,
            read: ReadState::Loading,
            last: None,
            fetched_at: None,
            reading: false,
            generation: 0,
            lease,
            wanted_until: Instant::now() + KEEP,
            preflight: HashMap::new(),
            busy: None,
            busy_token: 0,
            tasks: Vec::new(),
            polling: false,
        }
    }

    fn status(&self) -> Option<&Arc<Status>> {
        match &self.read {
            ReadState::Ready(status) => Some(status),
            _ => self.last.as_ref(),
        }
    }
}

/// A pre-flight run that waits for the app's state or for its checks.
struct Run {
    cluster: ClusterId,
    kind: ProviderKind,
    target: String,
    current: String,
    provider: Arc<dyn UpdateProvider>,
    status: Arc<Status>,
    deadline: Instant,
    /// Still waiting for the Helm releases and Prometheus detection.
    waiting: bool,
}

/// The app-wide update state. See the module docs.
pub struct UpdatesCore {
    clusters: HashMap<ClusterId, ClusterUpdates>,
    /// Poll on timers (off in tests).
    poll: bool,
    /// The last write token handed out.
    writes: u64,
    /// The last task token and run id handed out.
    tokens: u64,
    /// Tasks whose last callback ran; their handles are dropped on the next prune.
    done: HashSet<u64>,
    /// The task whose callback runs now: its handle must not drop itself.
    executing: Option<u64>,
    runs: HashMap<u64, Run>,
}

impl Service for UpdatesCore {
    type Event = Infallible;
    type Effect = UpdatesEffect;
}

impl UpdatesCore {
    /// `poll`: read on timers (off in tests).
    pub fn new(poll: bool) -> Self {
        Self {
            clusters: HashMap::new(),
            poll,
            writes: 0,
            tokens: 0,
            done: HashSet::new(),
            executing: None,
            runs: HashMap::new(),
        }
    }

    /// The clusters being tracked (the app builds their connection snapshots).
    pub fn tracked(&self) -> Vec<ClusterId> {
        self.clusters.keys().cloned().collect()
    }

    pub fn tracks(&self, cluster: &ClusterId) -> bool {
        self.clusters.contains_key(cluster)
    }

    fn token(&mut self) -> u64 {
        self.tokens += 1;
        self.tokens
    }

    /// Keeps `handle` alive with the cluster's state, dropping those of finished tasks.
    fn track(&mut self, cluster: &ClusterId, token: u64, handle: TaskHandle) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let (done, executing) = (&mut self.done, self.executing);
        state.tasks.retain(|(t, _)| {
            let finished = done.contains(t) && Some(*t) != executing;
            if finished {
                done.remove(t);
            }
            !finished
        });
        state.tasks.push((token, handle));
    }

    /// Called first thing in the callback that ends task `token`.
    fn task_ended(&mut self, token: u64) {
        self.done.insert(token);
        self.executing = Some(token);
    }

    /// Keeps `cluster`'s reads going while the returned lease lives.
    pub fn watch(
        &mut self,
        cluster: &ClusterId,
        conns: &Conns,
        host: &mut dyn Host<Self>,
    ) -> UpdatesLease {
        self.ensure(cluster, conns, host);
        UpdatesLease(self.clusters[cluster].lease.clone())
    }

    /// A connection's state or discovery changed.
    pub fn connection_changed(
        &mut self,
        cluster: &ClusterId,
        conns: &Conns,
        host: &mut dyn Host<Self>,
    ) {
        if self.clusters.contains_key(cluster) {
            self.detect(cluster, conns, host);
        }
        host.notify();
    }

    /// A cluster's id changed (`conns` holds the new one).
    pub fn rekeyed(
        &mut self,
        from: &ClusterId,
        to: &ClusterId,
        conns: &Conns,
        host: &mut dyn Host<Self>,
    ) {
        if let Some(mut state) = self.clusters.remove(from) {
            // The provider holds the connection's client, which moved with it.
            state.facts = None;
            self.clusters.insert(to.clone(), state);
            self.detect(to, conns, host);
        }
        host.notify();
    }

    /// A provider override or cloud settings changed: build the providers again (`conns` holds
    /// every tracked cluster).
    pub fn settings_changed(&mut self, conns: &Conns, host: &mut dyn Host<Self>) {
        for id in self.tracked() {
            if let Some(state) = self.clusters.get_mut(&id) {
                state.facts = None;
            }
            self.detect(&id, conns, host);
        }
    }

    /// Starts tracking `cluster` (detection, reads, the poll loop) and marks it wanted.
    fn ensure(&mut self, cluster: &ClusterId, conns: &Conns, host: &mut dyn Host<Self>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.wanted_until = Instant::now() + KEEP;
            if !state.polling && self.poll {
                self.start_poll(cluster, host);
            }
            return;
        }
        self.clusters
            .insert(cluster.clone(), ClusterUpdates::new(Arc::new(())));
        self.detect(cluster, conns, host);
        if self.poll {
            self.start_poll(cluster, host);
        }
    }

    fn start_poll(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.polling = true;
        }
        self.schedule_poll(cluster.clone(), Duration::ZERO, host);
    }

    /// One round of the poll loop after `delay`: read, then schedule the next. Each round is its
    /// own timer, so none drops the handle of the round that runs it.
    fn schedule_poll(&mut self, id: ClusterId, delay: Duration, host: &mut dyn Host<Self>) {
        let token = self.token();
        let round_id = id.clone();
        let handle = host.after(delay, move |this, host| {
            this.task_ended(token);
            let id = round_id;
            let Some(state) = this.clusters.get_mut(&id) else {
                return;
            };
            let wanted = Arc::strong_count(&state.lease) > 1 || Instant::now() < state.wanted_until;
            if !wanted {
                // Nobody looks: stop, and drop the Helm watches.
                state.polling = false;
                host.effect(UpdatesEffect::ReleaseHelm(id));
                return;
            }
            let updating = state.status().is_some_and(|s| s.updating());
            host.effect(UpdatesEffect::Read(id.clone()));
            this.schedule_poll(id, if updating { UPDATING } else { IDLE }, host);
        });
        self.track(&id, token, handle);
    }

    /// Detects (again) and rebuilds the provider when its inputs changed.
    fn detect(&mut self, cluster: &ClusterId, conns: &Conns, host: &mut dyn Host<Self>) {
        let live = conns.get(cluster).and_then(|c| c.live.as_ref());
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(live) = live else {
            // Disconnected or not discovered yet: nothing to read.
            state.detected = None;
            state.facts = None;
            state.provider = None;
            state.read = ReadState::Loading;
            state.generation += 1;
            host.notify();
            return;
        };
        if state.facts.as_ref() == Some(&live.facts) && state.provider.is_some() {
            return;
        }
        let detected = detect_provider(live);
        let provider = build(live, &detected);
        let changed = state.detected.as_ref().map(|d| d.kind) != Some(detected.kind);
        state.detected = Some(detected);
        state.facts = Some(live.facts.clone());
        state.provider = Some(provider);
        state.generation += 1;
        state.reading = false;
        if changed {
            state.read = ReadState::Loading;
            state.last = None;
            state.preflight.clear();
        }
        self.read(cluster, conns, host);
    }

    /// Reads the provider now (unless a read runs).
    pub fn read(&mut self, cluster: &ClusterId, conns: &Conns, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(provider) = state.provider.clone() else {
            return;
        };
        if state.reading {
            return;
        }
        state.reading = true;
        let generation = state.generation;
        let client = conns
            .get(cluster)
            .and_then(|c| c.live.as_ref())
            .map(|l| l.client.clone());
        let fallback = fallback_for(client, &provider);
        let token = self.token();
        let id = cluster.clone();
        let handle = host.spawn(
            async move {
                match provider.read().await {
                    Ok(status) => Ok(status),
                    // A cloud without credentials still shows the Kubernetes side.
                    Err(err) if fallback.is_some() && cloud_unavailable(&err) => {
                        let (fallback, note) = fallback.expect("checked");
                        match fallback.read().await {
                            Ok(mut status) => {
                                status.notes.insert(0, note(&err));
                                Ok(status)
                            }
                            Err(_) => Err(err),
                        }
                    }
                    Err(err) => Err(err),
                }
            },
            {
                let id = id.clone();
                move |this, result, host| {
                    this.task_ended(token);
                    let Some(state) = this.clusters.get_mut(&id) else {
                        return;
                    };
                    if state.generation != generation {
                        return;
                    }
                    state.reading = false;
                    state.fetched_at = Some(Instant::now());
                    state.read = match result {
                        Ok(status) => {
                            let status = Arc::new(status);
                            state.last = Some(status.clone());
                            ReadState::Ready(status)
                        }
                        Err(err) => ReadState::Failed(err),
                    };
                    host.notify();
                }
            },
        );
        self.track(&id, token, handle);
    }

    /// What a view shows for a connected `cluster`.
    pub fn known(&self, cluster: &ClusterId) -> UpdateState {
        let Some(state) = self.clusters.get(cluster) else {
            return UpdateState::Detecting;
        };
        let Some(detected) = state.detected.clone() else {
            return UpdateState::Detecting;
        };
        UpdateState::Known {
            detected,
            read: state.read.clone(),
            last: state.last.clone(),
            fetched_at: state.fetched_at,
        }
    }

    /// The last good status of `cluster` (for other crates: badges, the title bar).
    pub fn status(&self, cluster: &ClusterId) -> Option<Arc<Status>> {
        self.clusters.get(cluster)?.status().cloned()
    }

    pub fn detected(&self, cluster: &ClusterId) -> Option<&Detected> {
        self.clusters.get(cluster)?.detected.as_ref()
    }

    pub fn provider(&self, cluster: &ClusterId) -> Option<Arc<dyn UpdateProvider>> {
        self.clusters.get(cluster)?.provider.clone()
    }

    pub fn is_reading(&self, cluster: &ClusterId) -> bool {
        self.clusters.get(cluster).is_some_and(|s| s.reading)
    }

    /// The write in flight, if any.
    pub fn busy(&self, cluster: &ClusterId) -> Option<&str> {
        self.clusters.get(cluster)?.busy.as_deref()
    }

    /// The pre-flight run of `target`, if one ran against the current version.
    pub fn preflight(&self, cluster: &ClusterId, target: &str) -> Option<&Preflight> {
        let state = self.clusters.get(cluster)?;
        let current = state.status()?.current.version.clone();
        state
            .preflight
            .get(target)
            .filter(|run| run.current == current)
    }

    /// Runs the pre-flight checks for `target` (again).
    ///
    /// Waits (polling the app through [`UpdatesEffect::ProbePreflight`]) until the Helm releases
    /// listed and Prometheus detection finished, or [`HELM_WAIT`] passed, so the checks don't
    /// pass on nothing.
    pub fn run_preflight(&mut self, cluster: &ClusterId, target: &str, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let (Some(provider), Some(status), Some(detected)) = (
            state.provider.clone(),
            state.status().cloned(),
            state.detected.clone(),
        ) else {
            return;
        };
        host.effect(UpdatesEffect::WatchHelm(cluster.clone()));
        let current = status.current.version.clone();
        state.preflight.insert(
            target.to_string(),
            Preflight {
                target: target.to_string(),
                current: current.clone(),
                started: Instant::now(),
                finished: None,
                checks: Vec::new(),
            },
        );
        host.notify();
        let run = self.token();
        self.runs.insert(
            run,
            Run {
                cluster: cluster.clone(),
                kind: detected.kind,
                target: target.to_string(),
                current,
                provider,
                status,
                deadline: Instant::now() + HELM_WAIT,
                waiting: true,
            },
        );
        // A first look right away, then every `PREFLIGHT_POLL`.
        let first = self.token();
        let handle = host.after(Duration::ZERO, move |this, host| {
            this.task_ended(first);
            this.ask_probe(run, host);
        });
        self.track(cluster, first, handle);
        let ticker = self.token();
        let handle = host.every(PREFLIGHT_POLL, move |this, host| {
            if !this.runs.get(&run).is_some_and(|r| r.waiting) {
                this.task_ended(ticker);
                return Flow::Stop;
            }
            this.ask_probe(run, host);
            Flow::Continue
        });
        self.track(cluster, ticker, handle);
    }

    fn ask_probe(&mut self, run: u64, host: &mut dyn Host<Self>) {
        if let Some(r) = self.runs.get(&run).filter(|r| r.waiting) {
            host.effect(UpdatesEffect::ProbePreflight {
                run,
                cluster: r.cluster.clone(),
                kind: r.kind,
                target: r.target.clone(),
            });
        }
    }

    /// Whether the run has waited long enough: the app then answers [`Probe::Go`] even if its
    /// state isn't ready.
    pub fn preflight_overdue(&self, run: u64) -> bool {
        self.runs
            .get(&run)
            .is_some_and(|r| Instant::now() > r.deadline)
    }

    /// The app's answer to [`UpdatesEffect::ProbePreflight`].
    pub fn preflight_probe(&mut self, run: u64, probe: Probe, host: &mut dyn Host<Self>) {
        let Some(r) = self.runs.get_mut(&run).filter(|r| r.waiting) else {
            return;
        };
        let Probe::Go(inputs) = probe else {
            return;
        };
        r.waiting = false;
        let Some(inputs) = inputs else {
            // The cluster went away while waiting: finish the run, don't leave it "running"
            // forever.
            let checks = vec![Check::new(
                "inputs",
                "Pre-flight checks",
                check::CheckStatus::Unknown,
                "The cluster disconnected before the checks could run. Re-run them.",
            )];
            self.finish_preflight(run, checks, host);
            return;
        };
        let extras = r.provider.preflight_extras(&r.status, &r.target);
        let cluster = r.cluster.clone();
        let token = self.token();
        let handle = host.spawn(
            async move {
                let (mut checks, extras) = futures::join!(preflight::run(*inputs), extras);
                checks.extend(extras);
                checks
            },
            move |this, checks, host| {
                this.task_ended(token);
                this.finish_preflight(run, checks, host);
            },
        );
        self.track(&cluster, token, handle);
    }

    fn finish_preflight(&mut self, run: u64, checks: Vec<Check>, host: &mut dyn Host<Self>) {
        let Some(r) = self.runs.remove(&run) else {
            return;
        };
        if let Some(found) = self
            .clusters
            .get_mut(&r.cluster)
            .and_then(|s| s.preflight.get_mut(&r.target))
            .filter(|found| found.current == r.current)
        {
            found.checks = checks;
            found.finished = Some(Instant::now());
        }
        host.notify();
    }

    /// Runs a confirmed plan once. Refuses on read-only clusters; never retries.
    pub fn start(
        &mut self,
        cluster: &ClusterId,
        plan: Plan,
        conns: &Conns,
        host: &mut dyn Host<Self>,
    ) {
        if conns.get(cluster).is_some_and(|c| c.read_only) {
            host.toast(Notice::error("This cluster is read-only in Kubyl."));
            return;
        }
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(provider) = state.provider.clone() else {
            return;
        };
        if state.busy.is_some() {
            return;
        }
        state.busy = Some(plan.title.clone());
        self.writes += 1;
        let write = self.writes;
        state.busy_token = write;
        host.notify();
        let token = self.token();
        let handle = host.spawn(
            async move { provider.start(&plan).await },
            move |this, result, host| {
                this.task_ended(token);
                // The cluster may have been rekeyed meanwhile: the id it started under is then
                // the old one.
                let current = finish_write(&mut this.clusters, write);
                match result {
                    Ok(message) => host.toast(Notice::info(message)),
                    Err(err) => host.toast(Notice::error(err.to_string())),
                }
                if let Some(current) = current {
                    host.effect(UpdatesEffect::Read(current));
                }
                host.notify();
            },
        );
        self.track(cluster, token, handle);
    }

    /// Puts a status in place without a provider (tests of views).
    pub fn seed(&mut self, cluster: &ClusterId, detected: Detected, status: Status) {
        let mut state = ClusterUpdates::new(Arc::new(()));
        state.detected = Some(detected);
        let status = Arc::new(status);
        state.last = Some(status.clone());
        state.read = ReadState::Ready(status);
        self.clusters.insert(cluster.clone(), state);
    }

    /// Sets a target's pre-flight run: running (`checks` None) or finished (tests of views).
    pub fn seed_preflight(
        &mut self,
        cluster: &ClusterId,
        target: &str,
        checks: Option<Vec<Check>>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(current) = state.status().map(|s| s.current.version.clone()) else {
            return;
        };
        state.preflight.insert(
            target.to_string(),
            Preflight {
                target: target.to_string(),
                current,
                started: Instant::now(),
                finished: checks.is_some().then(Instant::now),
                checks: checks.unwrap_or_default(),
            },
        );
    }

    /// Sets a cluster's provider without detection (tests).
    pub fn seed_provider(&mut self, cluster: &ClusterId, provider: Arc<dyn UpdateProvider>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.provider = Some(provider);
        }
    }
}

/// Clears the busy mark of write `token` wherever its cluster is now, and returns that id.
fn finish_write(
    clusters: &mut HashMap<ClusterId, ClusterUpdates>,
    token: u64,
) -> Option<ClusterId> {
    let (id, state) = clusters
        .iter_mut()
        .find(|(_, state)| state.busy_token == token)?;
    state.busy = None;
    state.busy_token = 0;
    Some(id.clone())
}

#[cfg(test)]
mod tests {
    use kubyl_base::host::TestHost;

    use super::*;
    use crate::provider::ProviderError;

    /// A provider whose reads and writes answer from memory.
    struct Fixed {
        version: &'static str,
        started: Result<&'static str, &'static str>,
    }

    impl UpdateProvider for Fixed {
        fn kind(&self) -> ProviderKind {
            ProviderKind::OpenShift
        }

        fn read(&self) -> crate::provider::ProviderFuture<Result<Status, ProviderError>> {
            let mut status = Status::default();
            status.current.version = self.version.into();
            Box::pin(async move { Ok(status) })
        }

        fn start(
            &self,
            _plan: &Plan,
        ) -> crate::provider::ProviderFuture<Result<String, ProviderError>> {
            let result = self
                .started
                .map(str::to_string)
                .map_err(|err| ProviderError::Other(err.to_string()));
            Box::pin(async move { result })
        }
    }

    fn plan(title: &str) -> Plan {
        Plan {
            scope: crate::model::Scope::ControlPlane,
            title: title.into(),
            from: "4.17.8".into(),
            to: "4.17.12".into(),
            kind_label: "z-stream".into(),
            changes: Vec::new(),
            risks: Vec::new(),
            notes: Vec::new(),
            irreversible: true,
            request: serde_json::Value::Null,
        }
    }

    fn detected() -> Detected {
        Detected {
            kind: ProviderKind::OpenShift,
            reason: "test".into(),
        }
    }

    fn seeded(core: &mut UpdatesCore, cluster: &ClusterId, provider: Fixed) {
        let mut status = Status::default();
        status.current.version = "4.17.8".into();
        core.seed(cluster, detected(), status);
        core.seed_provider(cluster, Arc::new(provider));
    }

    #[test]
    fn a_read_replaces_the_status_and_ignores_a_second_one_in_flight() {
        let cluster = ClusterId::new("c");
        let mut host = TestHost::new();
        let mut core = UpdatesCore::new(false);
        let fixed = Fixed {
            version: "4.17.12",
            started: Ok("started"),
        };
        seeded(&mut core, &cluster, fixed);

        core.read(&cluster, &Conns::new(), &mut host);
        assert!(core.is_reading(&cluster));
        // A read runs: this one does nothing.
        core.read(&cluster, &Conns::new(), &mut host);
        host.run_until_idle(&mut core);

        assert!(!core.is_reading(&cluster));
        assert_eq!(core.status(&cluster).unwrap().current.version, "4.17.12");
        assert!(host.notified > 0);
    }

    #[test]
    fn a_write_marks_the_cluster_busy_tells_the_result_and_reads_again() {
        let cluster = ClusterId::new("c");
        let mut host = TestHost::new();
        let mut core = UpdatesCore::new(false);
        let fixed = Fixed {
            version: "4.17.8",
            started: Ok("Update started"),
        };
        seeded(&mut core, &cluster, fixed);
        let plan = plan("Update c to 4.17.12");

        core.start(&cluster, plan.clone(), &Conns::new(), &mut host);
        assert_eq!(core.busy(&cluster), Some("Update c to 4.17.12"));
        // Only one write at a time.
        core.start(&cluster, plan, &Conns::new(), &mut host);
        host.run_until_idle(&mut core);

        assert_eq!(core.busy(&cluster), None);
        assert_eq!(host.notices.len(), 1);
        assert_eq!(host.notices[0].message, "Update started");
        assert!(matches!(
            host.effects.as_slice(),
            [UpdatesEffect::Read(read)] if *read == cluster
        ));
    }

    #[test]
    fn a_read_only_cluster_refuses_the_write() {
        let cluster = ClusterId::new("c");
        let mut host = TestHost::new();
        let mut core = UpdatesCore::new(false);
        let fixed = Fixed {
            version: "4.17.8",
            started: Ok("never"),
        };
        seeded(&mut core, &cluster, fixed);
        let conns = Conns::from([(
            cluster.clone(),
            ClusterConn {
                read_only: true,
                live: None,
            },
        )]);

        core.start(&cluster, plan("Update"), &conns, &mut host);

        assert_eq!(core.busy(&cluster), None);
        assert_eq!(
            host.notices[0].message,
            "This cluster is read-only in Kubyl."
        );
    }

    #[test]
    fn pre_flight_asks_the_app_until_it_answers_and_finishes_without_inputs() {
        let cluster = ClusterId::new("c");
        let mut host = TestHost::new();
        let mut core = UpdatesCore::new(false);
        let fixed = Fixed {
            version: "4.17.8",
            started: Ok("never"),
        };
        seeded(&mut core, &cluster, fixed);

        core.run_preflight(&cluster, "4.17.12", &mut host);
        assert!(core.preflight(&cluster, "4.17.12").unwrap().running());
        assert!(matches!(
            host.effects.first(),
            Some(UpdatesEffect::WatchHelm(watched)) if *watched == cluster
        ));

        // The app isn't ready yet: the service asks again after a while.
        host.run_until(&mut core, |_, host| {
            host.effects
                .iter()
                .any(|e| matches!(e, UpdatesEffect::ProbePreflight { .. }))
        });
        let run = host
            .effects
            .iter()
            .find_map(|e| match e {
                UpdatesEffect::ProbePreflight { run, .. } => Some(*run),
                _ => None,
            })
            .unwrap();
        core.preflight_probe(run, Probe::Waiting, &mut host);
        assert!(core.preflight(&cluster, "4.17.12").unwrap().running());

        // The cluster went away meanwhile.
        core.preflight_probe(run, Probe::Go(None), &mut host);
        host.run_until_idle(&mut core);
        let finished = core.preflight(&cluster, "4.17.12").unwrap();
        assert!(!finished.running());
        assert_eq!(finished.checks.len(), 1);
        assert_eq!(finished.checks[0].status, check::CheckStatus::Unknown);
    }

    /// A write that finishes after its cluster was rekeyed clears `busy` on the new id.
    #[test]
    fn a_write_finishing_after_a_rekey_clears_busy() {
        let mut state = ClusterUpdates::new(Arc::new(()));
        state.busy = Some("Update".into());
        state.busy_token = 7;
        let mut clusters = HashMap::new();
        // `Rekeyed` moved the state from the old id to the new one.
        clusters.insert(ClusterId::new("new-id"), state);
        clusters.insert(ClusterId::new("other"), ClusterUpdates::new(Arc::new(())));
        assert_eq!(
            finish_write(&mut clusters, 7),
            Some(ClusterId::new("new-id"))
        );
        assert!(clusters[&ClusterId::new("new-id")].busy.is_none());
        // Nothing left to clear.
        assert_eq!(finish_write(&mut clusters, 7), None);
    }
}
