//! The alerts cache: one entry per cluster, filled in the background.
//!
//! Alertmanager has no watch API, so the service polls, but only what someone looks at: a
//! cluster whose alerts a view shows is refreshed every `refresh_interval`; every other
//! connected cluster (sidebar badges, the status bar, notifications) every
//! `background_refresh_interval`, and every view falls back to that pace while no Kubyl window
//! is active. Rules are read every 2 minutes. Parsing and merging run on the Tokio runtime.
//!
//! Per cluster, the sources are found first (see [`crate::discover`] and [`crate::client`]),
//! then alerts and silences are read from every Alertmanager, pending alerts and rules from the
//! Prometheus phase 07 found. Transitions (started firing, resolved) are kept in memory: the
//! first fetch is the baseline, resolved alerts show for a while, and notifications only ever
//! mention alerts that started while Kubyl watched.
//!
//! An Alertmanager behind HTTP basic auth is listed as [`Locked`] until the user signs in. Its
//! Authorization header is kept in the keychain while it works and deleted once it's rejected
//! (the view asks again); it's read through a temporary loopback forward, since the API server
//! strips credentials from service-proxy requests.
//!
//! [`AlertsCore`] is a plain struct on any [`Host`]. What it reads from the app (connected
//! clusters and their clients, the Prometheus metrics found, who may be notified) comes in as an
//! [`Env`] snapshot that the host builds when it carries out [`AlertsEffect::Tick`]; forwards to
//! Services come from a [`Reach`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures::channel::oneshot;
use jiff::Timestamp;
use kubyl_base::host::{Flow, Host, HostExt as _, Service, TaskHandle};
use kubyl_base::{ClusterId, Notice, NotificationLevel, SharedString};
use kubyl_kube_core::auth::BearerToken;
use kubyl_metrics_core::basic_auth::{self, SavedBasic};
use kubyl_metrics_core::prometheus::{PromClient, PromError, basic_authorization};
use kubyl_portforward_core::reach::{Reach, ReachPort, ReachRequest, Reached};
use secrecy::SecretString;
use serde_json::Value;

use crate::cache::{
    self, AFTER_WRITE, DEMAND_TTL, FORWARD_TIMEOUT, FetchInput, FetchOutput, Locked, MAX_FAILURES,
    METRICS_WAIT, NOTIFY_EVERY, NodeMap, Pace, Phase, REDISCOVER, RESOLVED_KEEP, RULES_EVERY,
    Source, read_headers, read_nodes, service_port,
};
use crate::client::{self, AmConn, Credentials, Discovered, Probed, Tried};
use crate::discover::AmTarget;
use crate::matchers::{self, Matcher};
use crate::merge::Heartbeat;
use crate::model::{self, Alert, AlertState, RuleGroup, Severity, Silence};
use crate::settings::{AlertsSettings, ClusterSettings, NotifyClusters};

pub use crate::cache::Counts;

/// Keychain entries of the username and password (a `Basic …` header with the Service's UID)
/// of an in-cluster Alertmanager behind basic auth: `alerts-password:<cluster id>/<ns>/<svc>`,
/// the entry's id first, then its members' (see [`basic_auth::keys`]). `ids` are the cluster's
/// settings keys, when the app knows them.
pub fn password_keys(cluster: &ClusterId, ids: Option<&[String]>, label: &str) -> Vec<String> {
    match ids {
        Some(ids) => basic_auth::keys("alerts-password", ids, label),
        None => basic_auth::keys("alerts-password", &[cluster.to_string()], label),
    }
}

/// How signing in to a locked Alertmanager ended.
#[derive(Debug)]
pub enum Unlock {
    NoCredentials,
    /// The Service was re-created since the credentials were saved.
    Replaced,
    Rejected,
    Failed(String),
}

/// What the service tells its host to do.
#[derive(Debug)]
pub enum AlertsEffect {
    /// Alert data changed: the chrome (sidebar badges and markers) rebuilds.
    Changed,
    /// Run [`AlertsCore::tick`] with a fresh [`Env`].
    Tick,
    /// Alerts started or resolved in a cluster: show this (with a way to open the alerts).
    Notify { cluster: ClusterId, notice: Notice },
}

/// What the service reads from the app, at the time of a tick.
pub struct Env {
    /// Some Kubyl window is active (views fall back to the background pace without one).
    pub window_active: bool,
    /// The connected clusters, in the order the app lists them.
    pub clusters: Vec<ClusterEnv>,
}

/// One connected cluster.
pub struct ClusterEnv {
    pub id: ClusterId,
    pub client: kube::Client,
    /// The keys its settings and keychain entries go by (see `settings_keys` of the connection
    /// manager).
    pub settings_keys: Vec<String>,
    /// Needed only when [`AlertsCore::discovery_due`] says discovery runs now.
    pub discovery: Option<DiscoveryEnv>,
    /// Needed only for clusters in [`AlertsCore::notices_ready`].
    pub notify: Option<NotifyEnv>,
}

/// What discovery reads from the app for one cluster.
#[derive(Default)]
pub struct DiscoveryEnv {
    /// The user's bearer token (not for client-certificate users).
    pub user_token: Option<BearerToken>,
    /// The metrics service's view of the cluster, `None` without one.
    pub metrics: Option<MetricsEnv>,
}

/// What the metrics service knows about a cluster's Prometheus.
#[derive(Clone)]
pub struct MetricsEnv {
    /// The Prometheus rules and pending alerts come from.
    pub prometheus: Option<PromClient>,
    /// The source isn't known yet (not looked for, or looking).
    pub pending: bool,
}

/// What deciding whether to notify about a cluster reads from the app.
#[derive(Default)]
pub struct NotifyEnv {
    pub display_name: Option<String>,
    /// Marked as a production cluster.
    pub production: bool,
    /// Is the active cluster.
    pub active: bool,
    /// A favorite of the user (a saved view doesn't make its cluster one).
    pub favorite: bool,
}

#[derive(Default)]
struct Fetch {
    last: Option<Instant>,
    in_flight: bool,
}

impl Fetch {
    fn due(&self, every: Duration) -> bool {
        !self.in_flight && self.last.is_none_or(|last| last.elapsed() >= every)
    }
}

/// A temporary loopback forward the service keeps while it uses it. Dropping it stops the
/// forward. When the user stops it first, the service hears about it.
struct Forward {
    key: u64,
    // Dropped first: the watch must not hear about the stop we cause.
    _watch: TaskHandle,
    _reached: Reached,
}

/// What stopping a forward on the user's side means.
enum Held {
    /// A trusted Service behind an auth proxy: look again.
    Trusted,
    /// A signed-in Alertmanager: locked again.
    Password(String),
}

/// Everything known about one cluster's alerts.
pub struct ClusterAlerts {
    pub phase: Phase,
    pub sources: Vec<Source>,
    /// What discovery tried, and the steps that found nothing.
    pub tried: Vec<Tried>,
    pub notes: Vec<String>,
    pub discovered_at: Option<Timestamp>,
    /// `Prometheus monitoring/prometheus-operated`, when rules come from there.
    pub rules_source: Option<String>,
    /// Active alerts, merged and sorted (heartbeat and hidden alerts left out).
    pub alerts: Arc<Vec<Alert>>,
    /// Alerts that stopped firing while Kubyl watched, newest first.
    pub resolved: Vec<Alert>,
    pub silences: Arc<Vec<Silence>>,
    pub rules: Arc<Vec<RuleGroup>>,
    pub rules_error: Option<SharedString>,
    pub heartbeat: Option<Heartbeat>,
    /// The last complete read.
    pub checked_at: Option<Timestamp>,
    /// The last read failed (the data is from `checked_at`).
    pub error: Option<SharedString>,
    /// The cluster's `matchers` (a central Alertmanager), added to every silence.
    pub matchers: Vec<Matcher>,
    /// Alertmanagers behind basic auth that aren't signed in.
    pub locked: Vec<Locked>,
    /// Bumped whenever the data changes (views rebuild their rows then).
    pub revision: u64,
    generation: u64,
    fetch: Fetch,
    rules_fetch: Fetch,
    failures: u32,
    discovering: bool,
    since: Instant,
    baseline: bool,
    /// Firing alerts of the last read, by fingerprint (for transitions).
    firing: HashMap<String, Alert>,
    forwards: Vec<Forward>,
    /// The forwards of signed-in Alertmanagers (also in `forwards`), by label.
    password_forwards: HashMap<String, u64>,
    prom: Option<PromClient>,
    prometheus_alertmanagers: Option<usize>,
    nodes: Arc<NodeMap>,
    refetch_at: Option<Instant>,
    /// The client and settings keys of the last tick, for signing in.
    client: Option<kube::Client>,
    settings_keys: Option<Vec<String>>,
}

impl ClusterAlerts {
    /// Whether to read now. Never while a read is in flight (an older read finishing last
    /// would overwrite a newer one): a pending `refetch_at` waits for it.
    fn fetch_wanted(&self, every: Duration) -> bool {
        let refetch = self.refetch_at.is_some_and(|t| t <= Instant::now());
        !self.fetch.in_flight && (refetch || self.fetch.due(every))
    }

    fn new(generation: u64) -> Self {
        Self {
            phase: Phase::Unknown,
            sources: Vec::new(),
            tried: Vec::new(),
            notes: Vec::new(),
            discovered_at: None,
            rules_source: None,
            alerts: Arc::default(),
            resolved: Vec::new(),
            silences: Arc::default(),
            rules: Arc::default(),
            rules_error: None,
            heartbeat: None,
            checked_at: None,
            error: None,
            matchers: Vec::new(),
            locked: Vec::new(),
            revision: 0,
            generation,
            fetch: Fetch::default(),
            rules_fetch: Fetch::default(),
            failures: 0,
            discovering: false,
            since: Instant::now(),
            baseline: false,
            firing: HashMap::new(),
            forwards: Vec::new(),
            password_forwards: HashMap::new(),
            prom: None,
            prometheus_alertmanagers: None,
            nodes: Arc::default(),
            refetch_at: None,
            client: None,
            settings_keys: None,
        }
    }

    pub fn counts(&self) -> Counts {
        Counts::of(&self.alerts)
    }

    /// The Prometheus rules and pending alerts come from.
    pub fn prometheus(&self) -> Option<&PromClient> {
        self.prom.as_ref()
    }

    /// Silences can be read and written (an Alertmanager answers).
    pub fn has_alertmanager(&self) -> bool {
        !self.sources.is_empty()
    }

    pub fn has_rules(&self) -> bool {
        self.prom.is_some()
    }

    /// The Alertmanager a silence goes to: the one named `source`, else the first.
    pub fn source(&self, source: Option<&str>) -> Option<&Source> {
        match source {
            Some(label) => self.sources.iter().find(|s| s.label() == label),
            None => self.sources.first(),
        }
    }

    /// Every receiver name of every Alertmanager.
    pub fn receivers(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .sources
            .iter()
            .flat_map(|s| s.receivers.iter().cloned())
            .chain(self.alerts.iter().flat_map(|a| a.receivers.iter().cloned()))
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Rules that fail to evaluate.
    pub fn failing_rules(&self) -> usize {
        self.rules
            .iter()
            .flat_map(|g| &g.rules)
            .filter(|r| r.failing())
            .count()
    }

    pub fn rule_count(&self) -> usize {
        self.rules.iter().map(|g| g.rules.len()).sum()
    }

    /// Whether discovery should run again, given the settings and how long ago it ran.
    fn rediscover(&self) -> bool {
        match self.phase {
            // Also while waiting for phase 07's Prometheus (nothing in flight yet).
            Phase::Unknown | Phase::Discovering | Phase::Disabled => true,
            Phase::NoSource => self.discovered_at.is_some_and(|t| {
                Timestamp::now().duration_since(t).as_secs() >= REDISCOVER.as_secs() as i64
            }),
            Phase::Ready => self.failures >= MAX_FAILURES,
        }
    }
}

#[derive(Default)]
struct Notices {
    started: Vec<Alert>,
    resolved: Vec<Alert>,
}

/// A write to an Alertmanager. Await it for the outcome; dropping it cancels the write (and
/// the update of the cached lists that follows it).
pub struct Write<T> {
    done: oneshot::Receiver<Result<T, String>>,
    _task: TaskHandle,
}

impl<T> Write<T> {
    fn failed(error: &str) -> Self {
        let (tx, done) = oneshot::channel();
        tx.send(Err(error.to_string())).ok();
        Self {
            done,
            _task: TaskHandle::none(),
        }
    }
}

impl<T> Future for Write<T> {
    type Output = Result<T, String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.done)
            .poll(cx)
            .map(|done| done.unwrap_or_else(|_| Err("the write was cancelled".into())))
    }
}

/// The app-wide alerts cache. See the module docs.
pub struct AlertsCore {
    clusters: HashMap<ClusterId, ClusterAlerts>,
    demand: RefCell<HashMap<ClusterId, Instant>>,
    settings: AlertsSettings,
    generation: u64,
    notices: HashMap<ClusterId, Notices>,
    notified: HashMap<ClusterId, Instant>,
    reach: Arc<dyn Reach>,
    /// Where the saved Authorization headers and Alertmanager logins are kept.
    secrets: kubyl_kube_core::auth::Credentials,
    next_forward: u64,
    ticking: Option<TaskHandle>,
}

impl Service for AlertsCore {
    type Event = Infallible;
    type Effect = AlertsEffect;
}

impl AlertsCore {
    pub fn new(settings: AlertsSettings, reach: Arc<dyn Reach>) -> Self {
        Self {
            clusters: HashMap::new(),
            demand: RefCell::default(),
            settings,
            generation: 0,
            notices: HashMap::new(),
            notified: HashMap::new(),
            reach,
            secrets: kubyl_kube_core::auth::Credentials::default(),
            next_forward: 0,
            ticking: None,
        }
    }

    /// Keeps the saved headers and logins in `secrets` instead of the default handle's entries.
    pub fn with_credentials(mut self, secrets: kubyl_kube_core::auth::Credentials) -> Self {
        self.secrets = secrets;
        self
    }

    /// Starts the refresh loop: [`AlertsEffect::Tick`] once a second.
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        self.ticking = Some(host.every(Duration::from_secs(1), |_, host| {
            host.effect(AlertsEffect::Tick);
            Flow::Continue
        }));
    }

    pub fn settings(&self) -> &AlertsSettings {
        &self.settings
    }

    // ----- Reading -----

    /// The cluster's alerts. `Pace::View` keeps them at the fast pace.
    pub fn cluster(&self, cluster: &ClusterId, pace: Pace) -> Option<&ClusterAlerts> {
        if pace == Pace::View {
            self.demand
                .borrow_mut()
                .insert(cluster.clone(), Instant::now());
        }
        self.clusters.get(cluster)
    }

    /// The phase, `Unknown` for clusters not looked at.
    pub fn phase(&self, cluster: &ClusterId) -> Phase {
        self.clusters
            .get(cluster)
            .map(|c| c.phase.clone())
            .unwrap_or(Phase::Unknown)
    }

    /// Counts of the cluster's alerts, once read.
    pub fn counts(&self, cluster: &ClusterId) -> Option<Counts> {
        let state = self.clusters.get(cluster)?;
        state.checked_at.map(|_| state.counts())
    }

    /// The cluster has an alert source (`None` while unknown).
    pub fn has_source(&self, cluster: &ClusterId) -> Option<bool> {
        match self.clusters.get(cluster)?.phase {
            Phase::Ready => Some(true),
            Phase::NoSource | Phase::Disabled => Some(false),
            Phase::Unknown | Phase::Discovering => None,
        }
    }

    /// Clusters with alert data, for the all-clusters view.
    pub fn clusters(&self) -> impl Iterator<Item = (&ClusterId, &ClusterAlerts)> {
        self.clusters.iter()
    }

    /// Whether the next tick runs discovery for `cluster`: the host fills [`ClusterEnv::discovery`]
    /// for these only.
    pub fn discovery_due(&self, cluster: &ClusterId, settings_keys: &[String]) -> bool {
        if !self.settings.enabled || self.settings.cluster(settings_keys).disabled {
            return false;
        }
        self.clusters
            .get(cluster)
            .is_none_or(|state| !state.discovering && state.rediscover())
    }

    /// The clusters the next tick may notify about: the host fills [`ClusterEnv::notify`] for
    /// these only.
    pub fn notices_ready(&self) -> Vec<ClusterId> {
        self.notices
            .keys()
            .filter(|c| {
                self.notified
                    .get(*c)
                    .is_none_or(|t| t.elapsed() >= NOTIFY_EVERY)
            })
            .cloned()
            .collect()
    }

    // ----- Control -----

    /// The settings changed.
    pub fn settings_changed(&mut self, settings: AlertsSettings, host: &mut dyn Host<Self>) {
        if self.settings != settings {
            self.settings = settings;
            self.reset_all(host);
        }
    }

    /// Forgets the cluster's sources and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        self.reset(cluster, host);
        self.demand
            .borrow_mut()
            .insert(cluster.clone(), Instant::now());
        host.effect(AlertsEffect::Tick);
        host.notify();
    }

    /// Reads the cluster again now (after a write, or "Refresh").
    pub fn refresh_now(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.fetch.last = None;
        }
        host.effect(AlertsEffect::Tick);
    }

    fn reset(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        self.generation += 1;
        // The old state's forwards stop with it.
        drop(
            self.clusters
                .insert(cluster.clone(), ClusterAlerts::new(self.generation)),
        );
        host.effect(AlertsEffect::Changed);
    }

    fn reset_all(&mut self, host: &mut dyn Host<Self>) {
        let clusters: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for cluster in clusters {
            self.reset(&cluster, host);
        }
        host.effect(AlertsEffect::Tick);
        host.notify();
    }

    fn drop_cluster(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        if self.clusters.remove(cluster).is_some() {
            // Results of tasks started for the dropped state must not apply to a reconnect.
            self.generation += 1;
            self.notices.remove(cluster);
            host.effect(AlertsEffect::Changed);
            host.notify();
        }
    }

    // ----- What happens to the connections -----

    /// A cluster connected or disconnected (`connected`: it has a client now).
    pub fn connection_changed(
        &mut self,
        cluster: &ClusterId,
        connected: bool,
        host: &mut dyn Host<Self>,
    ) {
        // A reconnect may reach another cluster (kubeconfig edited): start over.
        if !connected {
            self.drop_cluster(cluster, host);
        }
    }

    /// A cluster's discovery finished or re-ran: Alertmanager or Prometheus may have been
    /// installed.
    pub fn discovery_changed(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        if matches!(self.phase(cluster), Phase::NoSource) {
            self.reset(cluster, host);
            host.notify();
        }
    }

    /// A cluster's id changed without reconnecting.
    pub fn rekeyed(&mut self, from: &ClusterId, to: &ClusterId, host: &mut dyn Host<Self>) {
        // Discovery and reads in flight report under the old id and are dropped: start the new
        // id over (new generation), or `discovering`/`in_flight` stay set.
        if self.clusters.remove(from).is_some() {
            self.reset(to, host);
        }
        let moved = self.demand.borrow_mut().remove(from);
        if let Some(t) = moved {
            self.demand.borrow_mut().insert(to.clone(), t);
        }
        if let Some(n) = self.notices.remove(from) {
            self.notices.insert(to.clone(), n);
        }
        host.notify();
        host.effect(AlertsEffect::Tick);
    }

    /// Production/read-only flags or settings keys may have changed.
    pub fn contexts_changed(&mut self, host: &mut dyn Host<Self>) {
        host.effect(AlertsEffect::Changed);
    }

    // ----- The refresh loop -----

    /// One step of the refresh loop: starts discovery and reads where they're due, and shows
    /// notifications that are ready.
    pub fn tick(&mut self, env: &Env, host: &mut dyn Host<Self>) {
        let connected_ids: HashSet<&ClusterId> = env.clusters.iter().map(|c| &c.id).collect();
        let gone: Vec<ClusterId> = self
            .clusters
            .keys()
            .filter(|id| !connected_ids.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            self.drop_cluster(&id, host);
        }
        self.demand
            .borrow_mut()
            .retain(|_, t| t.elapsed() < DEMAND_TTL * 10);
        for ce in &env.clusters {
            let cluster = &ce.id;
            let cluster_settings = self.settings.cluster(&ce.settings_keys);
            let generation = self.generation;
            let state = self
                .clusters
                .entry(cluster.clone())
                .or_insert_with(|| ClusterAlerts::new(generation));
            state.client = Some(ce.client.clone());
            state.settings_keys = Some(ce.settings_keys.clone());
            if !self.settings.enabled || cluster_settings.disabled {
                if state.phase != Phase::Disabled {
                    state.phase = Phase::Disabled;
                    state.revision += 1;
                    host.effect(AlertsEffect::Changed);
                    host.notify();
                }
                continue;
            }
            if state.phase == Phase::Disabled {
                state.phase = Phase::Unknown;
            }
            let viewed = self
                .demand
                .borrow()
                .get(cluster)
                .is_some_and(|t| t.elapsed() < DEMAND_TTL);
            let every = if viewed && env.window_active {
                self.settings.refresh()
            } else {
                self.settings.background_refresh()
            };
            if state.rediscover() && !state.discovering {
                self.discover(ce, cluster_settings, host);
                continue;
            }
            let Some(state) = self.clusters.get_mut(cluster) else {
                continue;
            };
            if state.phase == Phase::Ready && state.fetch_wanted(every) {
                state.refetch_at = None;
                self.fetch(cluster, host);
            }
        }
        self.flush_notices(env, host);
    }

    fn discover(
        &mut self,
        ce: &ClusterEnv,
        cluster_settings: ClusterSettings,
        host: &mut dyn Host<Self>,
    ) {
        let cluster = &ce.id;
        // Prometheus (phase 07) gives the rules and Prometheus' Alertmanager list: wait a
        // little for its detection.
        let (user_token, prom, metrics_pending) = match &ce.discovery {
            Some(discovery) => match &discovery.metrics {
                Some(metrics) => (
                    discovery.user_token.clone(),
                    metrics.prometheus.clone(),
                    metrics.pending,
                ),
                None => (discovery.user_token.clone(), None, false),
            },
            None => (None, None, false),
        };
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if metrics_pending && state.since.elapsed() < METRICS_WAIT {
            if state.phase == Phase::Unknown {
                state.phase = Phase::Discovering;
                host.notify();
            }
            return;
        }
        state.discovering = true;
        state.phase = Phase::Discovering;
        state.matchers = cluster_settings
            .matchers
            .iter()
            .filter_map(|m| matchers::parse(m).ok())
            .flatten()
            .collect();
        let generation = state.generation;
        // The forwards of the previous discovery stop.
        state.forwards.clear();
        // Keychain entries for external URLs: the entry's id first, then its members' ids.
        // (the URL as in settings, the keychain's key; the API URL a probe compares against)
        let urls: Vec<(String, String)> = cluster_settings
            .alertmanagers
            .iter()
            .filter_map(|a| Some((a.url.clone()?, crate::discover::url_of(a)?)))
            .collect();
        let id_keys: Vec<String> = ce
            .settings_keys
            .iter()
            .filter(|k| k.contains('@'))
            .cloned()
            .collect();
        let settings = self.settings.clone();
        let use_rules = cluster_settings.rules;
        let prom_for_rules = prom.clone().filter(|_| use_rules);
        let reach = self.reach.clone();
        let secrets = self.secrets.clone();
        let client = ce.client.clone();
        let cluster = cluster.clone();
        let work_cluster = cluster.clone();
        host.spawn(
            async move {
                let headers = read_headers(&secrets, &id_keys, &urls).await;
                let credentials = Credentials {
                    user_token,
                    headers,
                };
                let mut found = client::discover(
                    &client,
                    &cluster_settings,
                    settings.discover,
                    prom.as_ref(),
                    &credentials,
                )
                .await;
                let nodes = read_nodes(&client).await;
                // Trusted Services behind an auth proxy without a Route: temporary
                // forwards.
                let mut forwards = Vec::new();
                for target in std::mem::take(&mut found.forwards) {
                    match forward(&*reach, &work_cluster, &client, &target, &credentials).await {
                        Ok((conn, reached)) => {
                            forwards.push(reached);
                            if let Some(tried) =
                                found.tried.iter_mut().find(|t| t.label == target.label())
                            {
                                tried.error = None;
                            }
                            found.connected.push(conn);
                        }
                        Err(err) => {
                            if let Some(tried) =
                                found.tried.iter_mut().find(|t| t.label == target.label())
                            {
                                tried.error = Some(err);
                            }
                        }
                    }
                }
                (found, nodes, forwards)
            },
            move |this, (found, nodes, forwards), host| {
                let current = this
                    .clusters
                    .get(&cluster)
                    .is_some_and(|s| s.generation == generation);
                if !current {
                    // The forwards stop with `forwards`.
                    return;
                }
                this.discovered(&cluster, found, prom_for_rules, nodes, forwards, host);
            },
        )
        .detach();
        host.notify();
    }

    fn discovered(
        &mut self,
        cluster: &ClusterId,
        found: Discovered,
        prom: Option<PromClient>,
        nodes: NodeMap,
        forwards: Vec<Reached>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(generation) = self.clusters.get(cluster).map(|s| s.generation) else {
            return;
        };
        let forwards: Vec<Forward> = forwards
            .into_iter()
            .map(|reached| {
                self.hold(reached, cluster, generation, Held::Trusted, host)
                    .0
            })
            .collect();
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.discovering = false;
        state.discovered_at = Some(Timestamp::now());
        state.failures = 0;
        state.tried = found.tried;
        state.notes = found.notes;
        state.prometheus_alertmanagers = found.prometheus_alertmanagers;
        state.sources = found
            .connected
            .into_iter()
            .map(|conn| Source {
                conn,
                error: None,
                receivers: Vec::new(),
            })
            .collect();
        state.rules_source = prom
            .as_ref()
            .map(|p| format!("Prometheus {}", p.target().label()));
        state.prom = prom;
        state.nodes = Arc::new(nodes);
        let previous = std::mem::replace(&mut state.forwards, forwards);
        state.password_forwards.clear();
        state.locked = found.locked.into_iter().map(Locked::new).collect();
        let locked: Vec<String> = state.locked.iter().map(Locked::label).collect();
        state.phase = if state.sources.is_empty() && state.prom.is_none() {
            Phase::NoSource
        } else {
            Phase::Ready
        };
        state.fetch = Fetch::default();
        state.rules_fetch = Fetch::default();
        state.revision += 1;
        drop(previous);
        tracing::info!(
            cluster = %cluster,
            alertmanagers = state.sources.len(),
            rules = state.prom.is_some(),
            "alert sources"
        );
        host.effect(AlertsEffect::Changed);
        host.notify();
        // Signs in with what the keychain has, if anything.
        for label in locked {
            self.unlock(cluster, &label, None, host);
        }
        host.effect(AlertsEffect::Tick);
    }

    /// Keeps a forward and hears when the user stops it.
    fn hold(
        &mut self,
        mut reached: Reached,
        cluster: &ClusterId,
        generation: u64,
        held: Held,
        host: &mut dyn Host<Self>,
    ) -> (Forward, u64) {
        self.next_forward += 1;
        let key = self.next_forward;
        let stopped = reached.take_stopped();
        let cluster = cluster.clone();
        let watch = host.spawn(stopped, move |this, (), host| {
            this.forward_stopped(&cluster, generation, key, &held, host)
        });
        (
            Forward {
                key,
                _watch: watch,
                _reached: reached,
            },
            key,
        )
    }

    /// A forward ended without us dropping it: the user stopped it in Active Sessions (or its
    /// cluster went away).
    fn forward_stopped(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        key: u64,
        held: &Held,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get(cluster) else {
            return;
        };
        if state.generation != generation || !state.forwards.iter().any(|f| f.key == key) {
            return;
        }
        match held {
            Held::Trusted => self.redetect(cluster, host),
            Held::Password(label) => self.password_forward_stopped(cluster, label, host),
        }
    }

    // ----- Basic auth -----

    /// Signs in to a locked Alertmanager; the credentials are kept in the keychain once it
    /// takes them.
    pub fn sign_in(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        username: &str,
        password: &SecretString,
        host: &mut dyn Host<Self>,
    ) {
        let header = basic_authorization(username, password);
        self.unlock(cluster, label, Some(header), host);
    }

    /// Signs in again with the credentials in the keychain.
    pub fn reconnect(&mut self, cluster: &ClusterId, label: &str, host: &mut dyn Host<Self>) {
        self.unlock(cluster, label, None, host);
    }

    /// Connects to a locked Alertmanager through a loopback forward with `header` (`None`: the
    /// one in the keychain, if the Service is still the one it was given to). A header the user
    /// typed is kept once it works; saved ones the server refused are deleted.
    fn unlock(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        header: Option<SecretString>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let client = state.client.clone();
        let keys = password_keys(cluster, state.settings_keys.as_deref(), label);
        let generation = state.generation;
        let Some(locked) = state.locked.iter_mut().find(|l| l.label() == label) else {
            return;
        };
        if locked.busy {
            return;
        }
        let Some(client) = client else {
            locked.problem = Some("The cluster isn't connected.".into());
            host.notify();
            return;
        };
        let was_saved = locked.saved;
        locked.busy = true;
        let target = locked.target.clone();
        host.notify();
        let typed = header.is_some();
        let reach = self.reach.clone();
        let secrets = self.secrets.clone();
        let work_cluster = cluster.clone();
        let cluster = cluster.clone();
        let label = label.to_string();
        host.spawn(
            async move {
                let result = async {
                    let uid = service_uid(&client, &target).await?;
                    let header = match header {
                        Some(header) => header,
                        None => {
                            let saved = read_saved(&secrets, &keys)
                                .await
                                .ok_or(Unlock::NoCredentials)?;
                            // Never to a Service that replaced the one the user signed in to.
                            if saved.service_uid != uid {
                                return Err(Unlock::Replaced);
                            }
                            saved.header
                        }
                    };
                    let connected =
                        password_forward(&*reach, &work_cluster, &client, &target, &header).await?;
                    Ok((connected, header, uid))
                }
                .await;
                // A mistyped password leaves saved credentials alone.
                match &result {
                    Ok((_, header, uid)) if typed => {
                        let saved = SavedBasic {
                            header: header.clone(),
                            service_uid: uid.clone(),
                        };
                        if let Some(key) = keys.first()
                            && let Err(err) =
                                write_secret(&secrets, key.clone(), saved.to_secret()).await
                        {
                            tracing::warn!("Alertmanager credentials not stored: {err}");
                        }
                    }
                    Err(Unlock::Rejected) if !typed => delete_secrets(secrets, keys),
                    Err(Unlock::Replaced) => delete_secrets(secrets, keys),
                    _ => {}
                }
                result.map(|(connected, _, _)| connected)
            },
            move |this, result, host| {
                let current = this
                    .clusters
                    .get(&cluster)
                    .is_some_and(|s| s.generation == generation);
                if !current {
                    // A forward that was opened stops with `result`.
                    return;
                }
                this.unlocked(&cluster, &label, typed, was_saved, result, host);
            },
        )
        .detach();
    }

    fn unlocked(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        typed: bool,
        was_saved: bool,
        result: Result<(AmConn, Reached), Unlock>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get(cluster) else {
            return;
        };
        let generation = state.generation;
        let Some(index) = state.locked.iter().position(|l| l.label() == label) else {
            // A forward that was opened stops with `result`.
            return;
        };
        match result {
            Ok((conn, reached)) => {
                let (forward, key) = self.hold(
                    reached,
                    cluster,
                    generation,
                    Held::Password(label.to_string()),
                    host,
                );
                let Some(state) = self.clusters.get_mut(cluster) else {
                    return;
                };
                state.locked.remove(index);
                state.forwards.push(forward);
                state.password_forwards.insert(label.to_string(), key);
                state.sources.push(Source {
                    conn,
                    error: None,
                    receivers: Vec::new(),
                });
                if state.phase == Phase::NoSource {
                    state.phase = Phase::Ready;
                }
                if let Some(tried) = state.tried.iter_mut().find(|t| t.label == label) {
                    tried.error = None;
                }
                state.fetch.last = None;
                state.revision += 1;
                host.effect(AlertsEffect::Changed);
                host.notify();
                host.effect(AlertsEffect::Tick);
            }
            Err(err) => {
                let Some(state) = self.clusters.get_mut(cluster) else {
                    return;
                };
                let locked = &mut state.locked[index];
                locked.busy = false;
                (locked.saved, locked.problem) = match err {
                    Unlock::NoCredentials => (false, None),
                    Unlock::Replaced => (
                        false,
                        Some(
                            "The Service was re-created since you signed in. Check that it's \
                             still your Alertmanager, then sign in again."
                                .into(),
                        ),
                    ),
                    Unlock::Rejected if typed => (
                        was_saved,
                        Some("Alertmanager rejected this username and password.".into()),
                    ),
                    Unlock::Rejected => (
                        false,
                        Some(
                            "Alertmanager no longer takes the saved username and password.".into(),
                        ),
                    ),
                    Unlock::Failed(err) => (was_saved || !typed, Some(err.into())),
                };
                state.revision += 1;
                host.notify();
            }
        }
    }

    /// Signed-in Alertmanagers whose reads were refused: their credentials are deleted and
    /// they're locked again. Each comes with the forward port the read went through: a late
    /// answer for credentials already replaced by a new sign-in changes nothing.
    fn rejected(
        &mut self,
        cluster: &ClusterId,
        rejected: &[(String, u16)],
        host: &mut dyn Host<Self>,
    ) {
        for (label, port) in rejected {
            let Some(state) = self.clusters.get_mut(cluster) else {
                return;
            };
            let keys = password_keys(cluster, state.settings_keys.as_deref(), label);
            let Some(index) = state.sources.iter().position(|s| {
                &s.label() == label && s.conn.via == client::Via::Password { local_port: *port }
            }) else {
                continue;
            };
            let source = state.sources.remove(index);
            if let Some(forward) = state.password_forwards.remove(label) {
                state.forwards.retain(|f| f.key != forward);
            }
            delete_secrets_on(self.secrets.clone(), keys, host);
            let mut locked = Locked::new(source.conn.target);
            locked.problem =
                Some("Alertmanager no longer takes the saved username and password.".into());
            state.locked.push(locked);
            state.revision += 1;
        }
    }

    /// The user stopped a signed-in Alertmanager's forward in Active Sessions.
    fn password_forward_stopped(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(forward) = state.password_forwards.remove(label) else {
            return;
        };
        state.forwards.retain(|f| f.key != forward);
        if let Some(index) = state.sources.iter().position(|s| s.label() == label) {
            let source = state.sources.remove(index);
            let mut locked = Locked::new(source.conn.target);
            locked.saved = true;
            locked.problem = Some("The port-forward was stopped.".into());
            state.locked.push(locked);
        }
        state.revision += 1;
        host.effect(AlertsEffect::Changed);
        host.notify();
    }

    fn fetch(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.fetch.in_flight = true;
        let want_rules = state.prom.is_some() && state.rules_fetch.due(RULES_EVERY);
        if want_rules {
            state.rules_fetch.in_flight = true;
        }
        let input = FetchInput {
            conns: state.sources.iter().map(|s| s.conn.clone()).collect(),
            prom: state.prom.clone(),
            want_rules,
            rules: state.rules.clone(),
            matchers: state.matchers.clone(),
            settings: self.settings.clone(),
            nodes: state.nodes.clone(),
            prometheus_alertmanagers: state.prometheus_alertmanagers,
        };
        let generation = state.generation;
        let cluster = cluster.clone();
        host.spawn(cache::fetch(input), move |this, output, host| {
            this.fetched(&cluster, generation, output, host)
        })
        .detach();
    }

    fn fetched(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        output: FetchOutput,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if state.generation != generation {
            return;
        }
        state.fetch.in_flight = false;
        state.fetch.last = Some(Instant::now());
        if !output.rejected.is_empty() {
            self.rejected(cluster, &output.rejected, host);
            // The sources changed under this read: the next one starts over.
            host.effect(AlertsEffect::Changed);
            host.notify();
            return;
        }
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if let Some(rules) = output.rules {
            state.rules_fetch.in_flight = false;
            state.rules_fetch.last = Some(Instant::now());
            match rules {
                Ok(rules) => {
                    state.rules = Arc::new(rules);
                    state.rules_error = None;
                }
                Err(err) => state.rules_error = Some(err.into()),
            }
        }
        for (source, result) in state.sources.iter_mut().zip(&output.sources) {
            match result {
                Ok(receivers) => {
                    source.error = None;
                    if let Some(receivers) = receivers {
                        source.receivers = receivers.clone();
                    }
                }
                Err(err) => source.error = Some(err.clone().into()),
            }
        }
        let Some(merged) = output.merged else {
            state.failures += 1;
            state.error = output.error.map(SharedString::from);
            state.revision += 1;
            host.effect(AlertsEffect::Changed);
            host.notify();
            return;
        };
        state.failures = 0;
        state.error = output.error.map(SharedString::from);
        state.checked_at = Some(output.now);
        state.heartbeat = merged.heartbeat;
        if output.silences_ok {
            state.silences = Arc::new(output.silences);
        }

        // Transitions.
        let now = output.now;
        let firing: HashMap<String, Alert> = merged
            .alerts
            .iter()
            .filter(|a| a.state != AlertState::Pending)
            .map(|a| (a.fingerprint.clone(), a.clone()))
            .collect();
        let mut started = Vec::new();
        let mut resolved = Vec::new();
        if state.baseline {
            for (fingerprint, alert) in &firing {
                if !state.firing.contains_key(fingerprint) {
                    started.push(alert.clone());
                }
            }
            for (fingerprint, alert) in &state.firing {
                if !firing.contains_key(fingerprint) {
                    let mut alert = alert.clone();
                    alert.state = AlertState::Resolved;
                    alert.ends_at = Some(now);
                    resolved.push(alert);
                }
            }
        }
        state.baseline = true;
        state
            .resolved
            .retain(|a| !firing.contains_key(&a.fingerprint));
        state.resolved.retain(|a| {
            a.ends_at
                .is_some_and(|t| now.duration_since(t).as_secs() < RESOLVED_KEEP.as_secs() as i64)
        });
        resolved.sort_by(|a, b| a.name.cmp(&b.name));
        for alert in resolved.iter().rev() {
            state.resolved.insert(0, alert.clone());
        }
        state.firing = firing;
        state.alerts = Arc::new(merged.alerts);
        state.revision += 1;
        if !started.is_empty() || !resolved.is_empty() {
            let started: Vec<Alert> = started
                .into_iter()
                .filter(|a| a.state == AlertState::Firing)
                .collect();
            let notices = self.notices.entry(cluster.clone()).or_default();
            notices.started.extend(started);
            notices.resolved.extend(resolved);
        }
        host.effect(AlertsEffect::Changed);
        host.notify();
    }

    // ----- Notifications -----

    fn notify_cluster(&self, cluster: &ClusterId, env: &Env) -> bool {
        let notify = &self.settings.notify;
        if !notify.enabled {
            return false;
        }
        if notify.clusters == NotifyClusters::All {
            return true;
        }
        let Some(info) = env
            .clusters
            .iter()
            .find(|c| &c.id == cluster)
            .and_then(|c| c.notify.as_ref())
        else {
            return false;
        };
        match notify.clusters {
            NotifyClusters::All => true,
            NotifyClusters::Active => info.active,
            NotifyClusters::Production => info.production,
            NotifyClusters::Favorites => info.favorite,
        }
    }

    fn flush_notices(&mut self, env: &Env, host: &mut dyn Host<Self>) {
        let min = self.settings.min_notify_severity();
        for cluster in self.notices_ready() {
            let Some(notices) = self.notices.remove(&cluster) else {
                continue;
            };
            if !self.notify_cluster(&cluster, env) {
                continue;
            }
            let started: Vec<&Alert> = notices
                .started
                .iter()
                .filter(|a| a.severity.at_least(&min))
                .collect();
            let resolved: Vec<&Alert> = if self.settings.notify.resolved {
                notices
                    .resolved
                    .iter()
                    .filter(|a| a.severity.at_least(&min))
                    .collect()
            } else {
                Vec::new()
            };
            let name = env
                .clusters
                .iter()
                .find(|c| c.id == cluster)
                .and_then(|c| c.notify.as_ref())
                .and_then(|n| n.display_name.clone())
                .unwrap_or_else(|| cluster.to_string());
            let Some(notice) = notice(&name, &started, &resolved) else {
                continue;
            };
            self.notified.insert(cluster.clone(), Instant::now());
            host.effect(AlertsEffect::Notify { cluster, notice });
        }
    }

    // ----- Writes -----

    /// Creates (or with `id`, replaces) a silence on `source` (`None`: the first Alertmanager).
    /// The list updates right away and is read again after 2 s.
    pub fn post_silence(
        &mut self,
        cluster: &ClusterId,
        source: Option<&str>,
        body: Value,
        host: &mut dyn Host<Self>,
    ) -> Write<String> {
        let Some(conn) = self
            .clusters
            .get(cluster)
            .and_then(|s| s.source(source))
            .map(|s| s.conn.clone())
        else {
            return Write::failed("No Alertmanager for this cluster.");
        };
        let label = conn.label();
        let cluster = cluster.clone();
        let (tx, done) = oneshot::channel();
        let task = host.spawn(
            async move {
                conn.post_silence(&body)
                    .await
                    .map_err(|e| e.to_string())
                    .map(|id| (id, body))
            },
            move |this, result, host| match result {
                Ok((id, body)) => {
                    this.silence_written(&cluster, &label, &id, Some(&body), host);
                    tx.send(Ok(id)).ok();
                }
                Err(err) => {
                    tx.send(Err(err)).ok();
                }
            },
        );
        Write { done, _task: task }
    }

    /// Expires a silence.
    pub fn expire_silence(
        &mut self,
        cluster: &ClusterId,
        source: &str,
        id: &str,
        host: &mut dyn Host<Self>,
    ) -> Write<()> {
        let Some(conn) = self
            .clusters
            .get(cluster)
            .and_then(|s| s.source(Some(source)))
            .map(|s| s.conn.clone())
        else {
            return Write::failed("That Alertmanager isn't connected anymore.");
        };
        let silence = id.to_string();
        let cluster = cluster.clone();
        let label = source.to_string();
        let id = id.to_string();
        let (tx, done) = oneshot::channel();
        let task = host.spawn(
            async move {
                conn.expire_silence(&silence)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |this, result, host| match result {
                Ok(()) => {
                    this.silence_written(&cluster, &label, &id, None, host);
                    tx.send(Ok(())).ok();
                }
                Err(err) => {
                    tx.send(Err(err)).ok();
                }
            },
        );
        Write { done, _task: task }
    }

    /// Shows a write right away (HA replicas may answer the next read without it) and reads
    /// again shortly.
    fn silence_written(
        &mut self,
        cluster: &ClusterId,
        source: &str,
        id: &str,
        body: Option<&Value>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let now = Timestamp::now();
        let mut silences: Vec<Silence> = state.silences.as_ref().clone();
        match body {
            Some(body) => {
                // Editing replaces the silence: the old one expires.
                if let Some(old) = body["id"].as_str() {
                    for silence in silences.iter_mut().filter(|s| s.id == old && s.id != id) {
                        silence.state = "expired".into();
                        silence.ends_at = Some(now);
                    }
                }
                let mut parsed = model::parse_silences(&Value::Array(vec![body.clone()]), source);
                if let Some(mut silence) = parsed.pop() {
                    silence.id = id.to_string();
                    let started = silence.starts_at.is_none_or(|t| t <= now);
                    silence.state = if started { "active" } else { "pending" }.into();
                    silences.retain(|s| s.id != id);
                    if started {
                        let mut alerts: Vec<Alert> = state.alerts.as_ref().clone();
                        for alert in alerts.iter_mut().filter(|a| {
                            matches!(a.state, AlertState::Firing | AlertState::Unprocessed)
                                && silence.matches(&a.labels)
                        }) {
                            alert.state = AlertState::Silenced;
                            alert.silenced_by.push(id.to_string());
                        }
                        state.alerts = Arc::new(alerts);
                    }
                    silences.insert(0, silence);
                }
            }
            None => {
                for silence in silences.iter_mut().filter(|s| s.id == id) {
                    silence.state = "expired".into();
                    silence.ends_at = Some(now);
                }
                let mut alerts: Vec<Alert> = state.alerts.as_ref().clone();
                for alert in alerts.iter_mut() {
                    if alert.silenced_by.iter().any(|s| s == id) {
                        alert.silenced_by.retain(|s| s != id);
                        if alert.silenced_by.is_empty() && alert.state == AlertState::Silenced {
                            alert.state = if alert.inhibited_by.is_empty() {
                                AlertState::Firing
                            } else {
                                AlertState::Inhibited
                            };
                        }
                    }
                }
                state.alerts = Arc::new(alerts);
            }
        }
        state.silences = Arc::new(silences);
        state.refetch_at = Some(Instant::now() + AFTER_WRITE);
        state.revision += 1;
        host.effect(AlertsEffect::Changed);
        host.notify();
    }

    // ----- Tests of the views -----

    /// Marks `cluster` as locked behind basic auth (tests of the views).
    #[cfg(any(test, feature = "test-support"))]
    pub fn lock_for_test(&mut self, cluster: &ClusterId, target: AmTarget) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.locked.push(Locked::new(target));
            state.revision += 1;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn forget_demand_for_test(&mut self) {
        self.demand.borrow_mut().clear();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn wanted_for_test(&self, cluster: &ClusterId) -> bool {
        self.demand.borrow().contains_key(cluster)
    }

    /// Puts a cluster with an Alertmanager into the cache (tests of the views).
    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        alerts: Vec<Alert>,
        host: &mut dyn Host<Self>,
    ) {
        let generation = self.generation;
        let state = self
            .clusters
            .entry(cluster.clone())
            .or_insert_with(|| ClusterAlerts::new(generation));
        state.phase = Phase::Ready;
        state.checked_at = Some(Timestamp::now());
        state.alerts = Arc::new(alerts);
        state.revision += 1;
        host.notify();
    }
}

/// The toast for alerts that started (or resolved) in one cluster, `None` without any.
pub fn notice(name: &str, started: &[&Alert], resolved: &[&Alert]) -> Option<Notice> {
    if started.is_empty() && resolved.is_empty() {
        return None;
    }
    let describe = |alerts: &[&Alert]| {
        let mut names: Vec<&str> = alerts.iter().map(|a| a.name.as_str()).collect();
        names.dedup();
        let shown: Vec<&str> = names.iter().take(3).copied().collect();
        let more = names.len().saturating_sub(3);
        let mut text = shown.join(", ");
        if more > 0 {
            text.push_str(&format!(" and {more} more"));
        }
        text
    };
    let (level, message) = if !started.is_empty() {
        let critical = started.iter().any(|a| a.severity == Severity::Critical);
        let what = if started.len() == 1 {
            format!("{} started firing", started[0].name)
        } else {
            format!(
                "{} alerts started firing: {}",
                started.len(),
                describe(started)
            )
        };
        (
            if critical {
                NotificationLevel::Error
            } else {
                NotificationLevel::Warning
            },
            what,
        )
    } else {
        (
            NotificationLevel::Success,
            format!("Resolved: {}", describe(resolved)),
        )
    };
    Some(Notice::new(level, message).title(format!("Alerts · {name}")))
}

/// Opens a temporary loopback forward to `target` (a Service) and waits until it listens.
async fn open_forward(
    reach: &dyn Reach,
    cluster: &ClusterId,
    client: &kube::Client,
    target: &AmTarget,
) -> Result<Reached, String> {
    let AmTarget::Service {
        namespace,
        service,
        port,
        ..
    } = target
    else {
        return Err("not a Service".into());
    };
    // Forwards take the Service's port number: look a named one up.
    let number = match port.parse::<u16>() {
        Ok(number) => number,
        Err(_) => service_port(client, namespace, service, port)
            .await
            .ok_or_else(|| format!("{} has no port {port}", target.label()))?,
    };
    reach
        .reach(ReachRequest {
            client: client.clone(),
            cluster: cluster.clone(),
            namespace: namespace.clone(),
            service: service.clone(),
            port: ReachPort::Number(number),
            title: format!("Alertmanager · svc/{service}"),
            https: true,
            timeout: FORWARD_TIMEOUT,
            reconnect_grace: None,
        })
        .await
}

/// Opens a temporary loopback forward to `target` and connects through it.
async fn forward(
    reach: &dyn Reach,
    cluster: &ClusterId,
    client: &kube::Client,
    target: &AmTarget,
    credentials: &Credentials,
) -> Result<(AmConn, Reached), String> {
    let reached = open_forward(reach, cluster, client, target).await?;
    match client::probe_forward(client, target, reached.local_port, credentials).await {
        Probed::Connected(conn) => Ok((*conn, reached)),
        Probed::Failed(_, err) | Probed::NeedsForward(_, err) => Err(err),
        Probed::NeedsPassword(_) => Err("asks for a username and password".into()),
    }
}

/// Opens a temporary loopback forward to an Alertmanager behind basic auth and connects with
/// `header`.
async fn password_forward(
    reach: &dyn Reach,
    cluster: &ClusterId,
    client: &kube::Client,
    target: &AmTarget,
    header: &SecretString,
) -> Result<(AmConn, Reached), Unlock> {
    let reached = open_forward(reach, cluster, client, target)
        .await
        .map_err(Unlock::Failed)?;
    match client::probe_password(target, reached.local_port, header).await {
        Ok(conn) => Ok((conn, reached)),
        Err(PromError::Http(401, _)) => Err(Unlock::Rejected),
        Err(err) => Err(Unlock::Failed(err.to_string())),
    }
}

/// The first readable entry of `keys`.
async fn read_saved(
    secrets: &kubyl_kube_core::auth::Credentials,
    keys: &[String],
) -> Option<SavedBasic> {
    let keys = keys.to_vec();
    let secrets = secrets.clone();
    tokio::task::spawn_blocking(move || {
        keys.iter().find_map(|key| {
            secrets
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

/// The UID of the Service behind `target`.
async fn service_uid(client: &kube::Client, target: &AmTarget) -> Result<String, Unlock> {
    let AmTarget::Service {
        namespace, service, ..
    } = target
    else {
        return Err(Unlock::Failed("not a Service".into()));
    };
    basic_auth::service_uid(client, namespace, service)
        .await
        .map_err(Unlock::Failed)
}

async fn write_secret(
    secrets: &kubyl_kube_core::auth::Credentials,
    key: String,
    secret: SecretString,
) -> Result<(), String> {
    let secrets = secrets.clone();
    tokio::task::spawn_blocking(move || secrets.set(&key, &secret))
        .await
        .map_err(|e| e.to_string())?
}

/// Deletes keychain entries without waiting. Must run inside the Tokio runtime.
fn delete_secrets(secrets: kubyl_kube_core::auth::Credentials, keys: Vec<String>) {
    drop(tokio::task::spawn_blocking(move || {
        for key in keys {
            secrets.delete(&key).ok();
        }
    }));
}

/// [`delete_secrets`] from the host's thread.
fn delete_secrets_on(
    secrets: kubyl_kube_core::auth::Credentials,
    keys: Vec<String>,
    host: &mut dyn Host<AlertsCore>,
) {
    host.spawn(async move { delete_secrets(secrets, keys) }, |_, (), _| {})
        .detach();
}
