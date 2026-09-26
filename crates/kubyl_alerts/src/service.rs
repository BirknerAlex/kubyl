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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, Global, SharedString, Subscription, Task,
};
use jiff::Timestamp;
use kubyl_core::{ClusterId, Notification, NotificationCenter, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_metrics::MetricsService;
use kubyl_metrics::prometheus::PromClient;
use kubyl_portforward::manager::{
    Ephemeral, ForwardId, ForwardSpec, ForwardState, PortForwardManager,
};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};
use kubyl_settings::Settings;
use secrecy::SecretString;
use serde_json::Value;

use crate::client::{self, AmConn, Credentials, Discovered, Probed, Tried};
use crate::discover::AmTarget;
use crate::matchers::{self, Matcher};
use crate::merge::{self, Heartbeat, MergeOptions};
use crate::model::{
    self, Alert, AlertState, ParseOptions, PromAlert, RuleGroup, Severity, Silence,
};
use crate::settings::{AlertsSettings, ClusterSettings, NotifyClusters};

/// A view that asked within this long keeps the fast pace.
pub const DEMAND_TTL: Duration = Duration::from_secs(30);
/// Discovery runs again this often while nothing was found.
const REDISCOVER: Duration = Duration::from_secs(300);
/// Consecutive failed fetches before the sources are looked for again.
const MAX_FAILURES: u32 = 3;
const RULES_EVERY: Duration = Duration::from_secs(120);
/// Resolved alerts stay listed this long.
pub const RESOLVED_KEEP: Duration = Duration::from_secs(900);
/// At most one toast per cluster this often (the rest are batched).
const NOTIFY_EVERY: Duration = Duration::from_secs(60);
/// Silences take a moment to spread between HA replicas: read again this long after a write.
const AFTER_WRITE: Duration = Duration::from_secs(2);
/// How long discovery waits for phase 07 to find Prometheus.
const METRICS_WAIT: Duration = Duration::from_secs(20);
const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);

/// Keychain entry of the Authorization header of an external Alertmanager URL.
pub fn auth_key(cluster: &str, url: &str) -> String {
    format!("alerts-auth:{cluster}/{url}")
}

/// Where a cluster's alerts stand.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Not looked at yet (or the cluster isn't connected).
    Unknown,
    /// Alerts are off (`alerts.enabled`, or the cluster's `disabled`).
    Disabled,
    /// Looking for Alertmanager and Prometheus.
    Discovering,
    /// At least one source.
    Ready,
    /// No Alertmanager and no Prometheus rules API: see `tried` and `notes`.
    NoSource,
}

/// One Alertmanager and how its last read went.
#[derive(Clone, Debug)]
pub struct Source {
    pub conn: AmConn,
    pub error: Option<SharedString>,
    pub receivers: Vec<String>,
}

impl Source {
    pub fn label(&self) -> String {
        self.conn.label()
    }
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

/// Firing, pending, silenced… counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub critical: usize,
    pub warning: usize,
    pub info: usize,
    /// Firing with another (or no) severity.
    pub other: usize,
    pub pending: usize,
    pub silenced: usize,
    pub inhibited: usize,
}

impl Counts {
    pub fn of(alerts: &[Alert]) -> Self {
        let mut counts = Counts::default();
        for alert in alerts {
            match alert.state {
                AlertState::Firing | AlertState::Unprocessed => match alert.severity {
                    Severity::Critical => counts.critical += 1,
                    Severity::Warning => counts.warning += 1,
                    Severity::Info => counts.info += 1,
                    _ => counts.other += 1,
                },
                AlertState::Pending => counts.pending += 1,
                AlertState::Silenced => counts.silenced += 1,
                AlertState::Inhibited => counts.inhibited += 1,
                AlertState::Resolved => {}
            }
        }
        counts
    }

    pub fn firing(&self) -> usize {
        self.critical + self.warning + self.info + self.other
    }

    /// The most severe firing severity.
    pub fn worst(&self) -> Option<Severity> {
        if self.critical > 0 {
            Some(Severity::Critical)
        } else if self.warning > 0 {
            Some(Severity::Warning)
        } else if self.info > 0 {
            Some(Severity::Info)
        } else if self.other > 0 {
            Some(Severity::None)
        } else {
            None
        }
    }
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
    forwards: Vec<ForwardId>,
    prom: Option<PromClient>,
    prometheus_alertmanagers: Option<usize>,
    nodes: Arc<NodeMap>,
    refetch_at: Option<Instant>,
}

impl ClusterAlerts {
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
            prom: None,
            prometheus_alertmanagers: None,
            nodes: Arc::default(),
            refetch_at: None,
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
}

/// Node names by name and InternalIP, to map `instance` labels to nodes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeMap(HashMap<String, String>);

impl NodeMap {
    /// From a node list (`/api/v1/nodes`).
    pub fn from_list(items: &[Value]) -> Self {
        let mut map = HashMap::new();
        for node in items {
            let Some(name) = node.pointer("/metadata/name").and_then(Value::as_str) else {
                continue;
            };
            map.insert(name.to_string(), name.to_string());
            for address in node
                .pointer("/status/addresses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(value) = address["address"].as_str() {
                    map.insert(value.to_string(), name.to_string());
                }
            }
        }
        Self(map)
    }

    /// The node an `instance` (`10.0.1.5:9100`, `node-1`, `[fd00::1]:9100`) names.
    pub fn node_of(&self, instance: &str) -> Option<String> {
        if let Some(name) = self.0.get(instance) {
            return Some(name.clone());
        }
        let host = match instance.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => instance,
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        self.0.get(host).cloned()
    }
}

/// Views asked with this pace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// A view shows the alerts: `refresh_interval`.
    View,
    /// Badges, the status bar: `background_refresh_interval`.
    Background,
}

#[derive(Default)]
struct Notices {
    started: Vec<Alert>,
    resolved: Vec<Alert>,
}

/// The app-wide alerts cache. See the module docs.
pub struct AlertsService {
    clusters: HashMap<ClusterId, ClusterAlerts>,
    demand: RefCell<HashMap<ClusterId, Instant>>,
    settings: AlertsSettings,
    generation: u64,
    notices: HashMap<ClusterId, Notices>,
    notified: HashMap<ClusterId, Instant>,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

struct GlobalService(Entity<AlertsService>);

impl Global for GlobalService {}

impl AlertsService {
    /// Creates the service and makes it global. `tick` starts the refresh loop (off in tests).
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
        let weak = cx.weak_entity();
        subscriptions.push(Settings::observe::<AlertsSettings>(
            cx,
            move |settings, cx| {
                let settings = settings.clone();
                weak.update(cx, |this, cx| {
                    if this.settings != settings {
                        this.settings = settings;
                        this.reset_all(cx);
                    }
                })
                .ok();
            },
        ));
        let task = if tick {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                        break;
                    }
                }
            })
        } else {
            Task::ready(())
        };
        Self {
            clusters: HashMap::new(),
            demand: RefCell::default(),
            settings: Settings::get::<AlertsSettings>(cx).clone(),
            generation: 0,
            notices: HashMap::new(),
            notified: HashMap::new(),
            _tick: task,
            _subscriptions: subscriptions,
        }
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

    // ----- Control -----

    /// Forgets the cluster's sources and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.reset(cluster, cx);
        self.demand
            .borrow_mut()
            .insert(cluster.clone(), Instant::now());
        self.tick(cx);
        cx.notify();
    }

    /// Reads the cluster again now (after a write, or "Refresh").
    pub fn refresh_now(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.fetch.last = None;
        }
        self.tick(cx);
    }

    fn reset(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.generation += 1;
        if let Some(old) = self
            .clusters
            .insert(cluster.clone(), ClusterAlerts::new(self.generation))
        {
            stop_forwards(old.forwards, cx);
        }
        crate::changed(cx);
    }

    fn reset_all(&mut self, cx: &mut Context<Self>) {
        let clusters: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for cluster in clusters {
            self.reset(&cluster, cx);
        }
        self.tick(cx);
        cx.notify();
    }

    fn drop_cluster(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if let Some(old) = self.clusters.remove(cluster) {
            stop_forwards(old.forwards, cx);
            self.notices.remove(cluster);
            crate::changed(cx);
            cx.notify();
        }
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                // A reconnect may reach another cluster (kubeconfig edited): start over.
                if !connected {
                    self.drop_cluster(id, cx);
                }
            }
            ConnectionEvent::DiscoveryChanged(id) => {
                // Alertmanager or Prometheus may have been installed.
                if matches!(self.phase(id), Phase::NoSource) {
                    self.reset(id, cx);
                    cx.notify();
                }
            }
            ConnectionEvent::Rekeyed { from, to } => {
                if let Some(state) = self.clusters.remove(from) {
                    self.clusters.insert(to.clone(), state);
                }
                if let Some(t) = self.demand.borrow_mut().remove(from) {
                    self.demand.borrow_mut().insert(to.clone(), t);
                }
                if let Some(n) = self.notices.remove(from) {
                    self.notices.insert(to.clone(), n);
                }
                cx.notify();
            }
            ConnectionEvent::ContextsChanged => {
                // Production/read-only flags or settings keys may have changed.
                crate::changed(cx);
            }
            _ => {}
        }
    }

    // ----- The refresh loop -----

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let window_active = cx.active_window().is_some();
        let connected: Vec<(ClusterId, kube::Client)> = {
            let manager = manager.read(cx);
            manager
                .entries()
                .iter()
                .filter_map(|c| Some((c.id.clone(), manager.client(&c.id)?)))
                .collect()
        };
        let connected_ids: HashSet<ClusterId> =
            connected.iter().map(|(id, _)| id.clone()).collect();
        let gone: Vec<ClusterId> = self
            .clusters
            .keys()
            .filter(|id| !connected_ids.contains(*id))
            .cloned()
            .collect();
        for id in gone {
            self.drop_cluster(&id, cx);
        }
        self.demand
            .borrow_mut()
            .retain(|_, t| t.elapsed() < DEMAND_TTL * 10);
        for (cluster, client) in connected {
            let keys = manager.read(cx).settings_keys(&cluster);
            let cluster_settings = self.settings.cluster(&keys);
            let generation = self.generation;
            let state = self
                .clusters
                .entry(cluster.clone())
                .or_insert_with(|| ClusterAlerts::new(generation));
            if !self.settings.enabled || cluster_settings.disabled {
                if state.phase != Phase::Disabled {
                    state.phase = Phase::Disabled;
                    state.revision += 1;
                    crate::changed(cx);
                    cx.notify();
                }
                continue;
            }
            if state.phase == Phase::Disabled {
                state.phase = Phase::Unknown;
            }
            let viewed = self
                .demand
                .borrow()
                .get(&cluster)
                .is_some_and(|t| t.elapsed() < DEMAND_TTL);
            let every = if viewed && window_active {
                self.settings.refresh()
            } else {
                self.settings.background_refresh()
            };
            let rediscover = match state.phase {
                // Also while waiting for phase 07's Prometheus (nothing in flight yet).
                Phase::Unknown | Phase::Discovering => true,
                Phase::NoSource => state.discovered_at.is_some_and(|t| {
                    Timestamp::now().duration_since(t).as_secs() >= REDISCOVER.as_secs() as i64
                }),
                Phase::Ready => state.failures >= MAX_FAILURES,
                _ => false,
            };
            if rediscover && !state.discovering {
                self.discover(&cluster, client, keys, cluster_settings, cx);
                continue;
            }
            let Some(state) = self.clusters.get_mut(&cluster) else {
                continue;
            };
            let refetch = state.refetch_at.is_some_and(|t| t <= Instant::now());
            if state.phase == Phase::Ready && (refetch || state.fetch.due(every)) {
                state.refetch_at = None;
                self.fetch(&cluster, client, cx);
            }
        }
        self.flush_notices(cx);
    }

    fn discover(
        &mut self,
        cluster: &ClusterId,
        client: kube::Client,
        keys: Vec<String>,
        cluster_settings: ClusterSettings,
        cx: &mut Context<Self>,
    ) {
        // Prometheus (phase 07) gives the rules and Prometheus' Alertmanager list: wait a
        // little for its detection.
        let (prom, metrics_pending) = match MetricsService::global(cx) {
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
            if state.phase == Phase::Unknown {
                state.phase = Phase::Discovering;
                cx.notify();
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
        let old_forwards = std::mem::take(&mut state.forwards);
        stop_forwards(old_forwards, cx);
        let user_token = ConnectionManager::global(cx).read(cx).bearer_token(cluster);
        // Keychain entries for external URLs: the entry's id first, then its members' ids.
        // (the URL as in settings, the keychain's key; the API URL a probe compares against)
        let urls: Vec<(String, String)> = cluster_settings
            .alertmanagers
            .iter()
            .filter_map(|a| Some((a.url.clone()?, crate::discover::url_of(a)?)))
            .collect();
        let id_keys: Vec<String> = keys.iter().filter(|k| k.contains('@')).cloned().collect();
        let settings = self.settings.clone();
        let use_rules = cluster_settings.rules;
        let prom_for_rules = prom.clone().filter(|_| use_rules);
        let task = spawn_kube(cx, async move {
            let headers = read_headers(&id_keys, &urls).await;
            let credentials = Credentials {
                user_token,
                headers,
            };
            let found = client::discover(
                &client,
                &cluster_settings,
                settings.discover,
                prom.as_ref(),
                &credentials,
            )
            .await;
            let nodes = read_nodes(&client).await;
            (found, credentials, nodes)
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let (found, credentials, nodes) = task.await;
            // Trusted Services behind an auth proxy without a Route: temporary forwards.
            let mut found = found;
            let mut forwards = Vec::new();
            for target in std::mem::take(&mut found.forwards) {
                match forward(&cluster, &target, &credentials, cx).await {
                    Ok((conn, id)) => {
                        forwards.push(id);
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
            this.update(cx, |this, cx| {
                let current = this
                    .clusters
                    .get(&cluster)
                    .is_some_and(|s| s.generation == generation);
                if !current {
                    stop_forwards(forwards, cx);
                    return;
                }
                this.discovered(&cluster, found, prom_for_rules, nodes, forwards, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn discovered(
        &mut self,
        cluster: &ClusterId,
        found: Discovered,
        prom: Option<PromClient>,
        nodes: NodeMap,
        forwards: Vec<ForwardId>,
        cx: &mut Context<Self>,
    ) {
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
        state.forwards = forwards;
        state.phase = if state.sources.is_empty() && state.prom.is_none() {
            Phase::NoSource
        } else {
            Phase::Ready
        };
        state.fetch = Fetch::default();
        state.rules_fetch = Fetch::default();
        state.revision += 1;
        tracing::info!(
            cluster = %cluster,
            alertmanagers = state.sources.len(),
            rules = state.prom.is_some(),
            "alert sources"
        );
        crate::changed(cx);
        cx.notify();
        self.tick(cx);
    }

    fn fetch(&mut self, cluster: &ClusterId, client: kube::Client, cx: &mut Context<Self>) {
        let _ = client;
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
        let task = spawn_kube(cx, fetch(input));
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let output = task.await;
            this.update(cx, |this, cx| {
                this.fetched(&cluster, generation, output, cx)
            })
            .ok();
        })
        .detach();
    }

    fn fetched(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        output: FetchOutput,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if state.generation != generation {
            return;
        }
        state.fetch.in_flight = false;
        state.fetch.last = Some(Instant::now());
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
            crate::changed(cx);
            cx.notify();
            return;
        };
        state.failures = 0;
        state.error = output.error.map(SharedString::from);
        state.checked_at = Some(output.now);
        state.heartbeat = merged.heartbeat;
        state.silences = Arc::new(output.silences);

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
        crate::changed(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn forget_demand_for_test(&mut self) {
        self.demand.borrow_mut().clear();
    }

    #[cfg(test)]
    pub(crate) fn wanted_for_test(&self, cluster: &ClusterId) -> bool {
        self.demand.borrow().contains_key(cluster)
    }

    /// Puts a cluster with an Alertmanager into the cache (tests of the views).
    #[cfg(test)]
    pub fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        alerts: Vec<Alert>,
        cx: &mut Context<Self>,
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
        cx.notify();
    }

    // ----- Notifications -----

    fn notify_cluster(&self, cluster: &ClusterId, cx: &App) -> bool {
        let notify = &self.settings.notify;
        if !notify.enabled {
            return false;
        }
        if notify.clusters == NotifyClusters::All {
            return true;
        }
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return false;
        };
        let manager = manager.read(cx);
        match notify.clusters {
            NotifyClusters::All => true,
            NotifyClusters::Active => manager.active() == Some(cluster),
            NotifyClusters::Production => manager.caps(cluster).production,
            NotifyClusters::Favorites => kubyl_explorer::favorites::Favorites::global(cx)
                .read(cx)
                .items()
                .iter()
                .any(|f| kubyl_explorer::favorites::cluster_of(f, cx).as_ref() == Some(cluster)),
        }
    }

    fn flush_notices(&mut self, cx: &mut Context<Self>) {
        let ready: Vec<ClusterId> = self
            .notices
            .keys()
            .filter(|c| {
                self.notified
                    .get(*c)
                    .is_none_or(|t| t.elapsed() >= NOTIFY_EVERY)
            })
            .cloned()
            .collect();
        let min = self.settings.min_notify_severity();
        for cluster in ready {
            let Some(notices) = self.notices.remove(&cluster) else {
                continue;
            };
            if !self.notify_cluster(&cluster, cx) {
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
            let Some(notification) = notice(&cluster, &started, &resolved, cx) else {
                continue;
            };
            self.notified.insert(cluster.clone(), Instant::now());
            NotificationCenter::push(cx, notification);
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
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        let Some(conn) = self
            .clusters
            .get(cluster)
            .and_then(|s| s.source(source))
            .map(|s| s.conn.clone())
        else {
            return Task::ready(Err("No Alertmanager for this cluster.".into()));
        };
        let label = conn.label();
        let task = spawn_kube(cx, async move {
            conn.post_silence(&body)
                .await
                .map_err(|e| e.to_string())
                .map(|id| (id, body))
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let (id, body) = task.await?;
            this.update(cx, |this, cx| {
                this.silence_written(&cluster, &label, &id, Some(&body), cx)
            })
            .ok();
            Ok(id)
        })
    }

    /// Expires a silence.
    pub fn expire_silence(
        &mut self,
        cluster: &ClusterId,
        source: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Some(conn) = self
            .clusters
            .get(cluster)
            .and_then(|s| s.source(Some(source)))
            .map(|s| s.conn.clone())
        else {
            return Task::ready(Err("That Alertmanager isn't connected anymore.".into()));
        };
        let silence = id.to_string();
        let task = spawn_kube(cx, async move {
            conn.expire_silence(&silence)
                .await
                .map_err(|e| e.to_string())
        });
        let cluster = cluster.clone();
        let label = source.to_string();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            task.await?;
            this.update(cx, |this, cx| {
                this.silence_written(&cluster, &label, &id, None, cx)
            })
            .ok();
            Ok(())
        })
    }

    /// Shows a write right away (HA replicas may answer the next read without it) and reads
    /// again shortly.
    fn silence_written(
        &mut self,
        cluster: &ClusterId,
        source: &str,
        id: &str,
        body: Option<&Value>,
        cx: &mut Context<Self>,
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
        crate::changed(cx);
        cx.notify();
    }
}

/// The toast for alerts that started (or resolved) in one cluster.
fn notice(
    cluster: &ClusterId,
    started: &[&Alert],
    resolved: &[&Alert],
    cx: &App,
) -> Option<Notification> {
    if started.is_empty() && resolved.is_empty() {
        return None;
    }
    let name = ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(cluster).to_string())
        .unwrap_or_else(|| cluster.to_string());
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
                kubyl_core::NotificationLevel::Error
            } else {
                kubyl_core::NotificationLevel::Warning
            },
            what,
        )
    } else {
        (
            kubyl_core::NotificationLevel::Success,
            format!("Resolved: {}", describe(resolved)),
        )
    };
    let show = cluster.clone();
    Some(
        Notification::new(level, message)
            .title(format!("Alerts · {name}"))
            .action("Show", move |window, cx| {
                crate::view::open(&show, None, window, cx)
            }),
    )
}

fn stop_forwards(forwards: Vec<ForwardId>, cx: &mut App) {
    if forwards.is_empty() {
        return;
    }
    let manager = PortForwardManager::global(cx);
    manager.update(cx, |m, cx| {
        for id in forwards {
            m.stop(id, cx);
        }
    });
}

/// Authorization headers for `urls` from the keychain (first id with an entry wins). The
/// keychain key uses the URL as written in settings; the result is keyed by the API URL the
/// probe compares against (with `path`, without a trailing slash).
async fn read_headers(ids: &[String], urls: &[(String, String)]) -> Vec<(String, SecretString)> {
    if urls.is_empty() {
        return Vec::new();
    }
    let ids = ids.to_vec();
    let urls = urls.to_vec();
    tokio::task::spawn_blocking(move || {
        headers_for(&ids, &urls, |key| {
            kubyl_kube::auth::store::get(key)
                .inspect_err(|e| tracing::warn!("keychain: {e}"))
                .ok()
                .flatten()
        })
    })
    .await
    .unwrap_or_default()
}

fn headers_for(
    ids: &[String],
    urls: &[(String, String)],
    get: impl Fn(&str) -> Option<SecretString>,
) -> Vec<(String, SecretString)> {
    urls.iter()
        .filter_map(|(raw, api)| {
            ids.iter()
                .find_map(|id| get(&auth_key(id, raw)))
                .map(|header| (api.clone(), header))
        })
        .collect()
}

async fn read_nodes(client: &kube::Client) -> NodeMap {
    let Ok(request) = http::Request::get("/api/v1/nodes").body(Vec::new()) else {
        return NodeMap::default();
    };
    match client.request::<Value>(request).await {
        Ok(list) => NodeMap::from_list(
            list["items"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        ),
        Err(_) => NodeMap::default(),
    }
}

/// Opens a temporary loopback forward to `target` and connects through it.
async fn forward(
    cluster: &ClusterId,
    target: &AmTarget,
    credentials: &Credentials,
    cx: &mut AsyncApp,
) -> Result<(AmConn, ForwardId), String> {
    let AmTarget::Service {
        namespace,
        service,
        port,
        ..
    } = target
    else {
        return Err("not a Service".into());
    };
    let client = cx
        .update(|cx| ConnectionManager::global(cx).read(cx).client(cluster))
        .ok_or("the cluster isn't connected")?;
    // Forwards take the Service's port number: look a named one up.
    let number = match port.parse::<u16>() {
        Ok(number) => number,
        Err(_) => {
            let (client, namespace, service, name) = (
                client.clone(),
                namespace.clone(),
                service.clone(),
                port.clone(),
            );
            cx.update(|cx| {
                spawn_kube(cx, async move {
                    service_port(&client, &namespace, &service, &name).await
                })
            })
            .await
            .ok_or_else(|| format!("{} has no port {port}", target.label()))?
        }
    };
    let remote = RemotePort::Service(Some(number));
    let spec = ForwardSpec {
        cluster: cluster.clone(),
        namespace: namespace.clone(),
        kind: ForwardKind::Service {
            service: service.clone(),
        },
        port: remote,
        // Loopback only.
        bind_address: "127.0.0.1".into(),
        local_port: 0,
        target: kubyl_core::ResourceRef::object(
            cluster.clone(),
            kubyl_core::Gvr::new("", "v1", "services"),
            Some(namespace.clone()),
            service.clone(),
        ),
        remote_port: Some(number),
        http: false,
        https: true,
        open_browser: false,
        ephemeral: Some(Ephemeral {
            title: format!("Alertmanager · svc/{service}"),
            buttons: Vec::new(),
            on_stop: Arc::new({
                let cluster = cluster.clone();
                move |cx| {
                    if let Some(service) = AlertsService::global(cx) {
                        let cluster = cluster.clone();
                        service.update(cx, |this, cx| this.redetect(&cluster, cx));
                    }
                }
            }),
        }),
    };
    let id = cx.update(|cx| PortForwardManager::start(client.clone(), spec, cx));
    let start = Instant::now();
    let local_port = loop {
        let info = cx.update(|cx| PortForwardManager::global(cx).read(cx).info(id));
        match info {
            Some(info) if info.state == ForwardState::Listening && info.local_port != 0 => {
                break info.local_port;
            }
            Some(info) if matches!(info.state, ForwardState::Failed(_)) => {
                cx.update(|cx| stop_forwards(vec![id], cx));
                return Err(match info.state {
                    ForwardState::Failed(err) => format!("port-forward: {err}"),
                    _ => unreachable!(),
                });
            }
            None => return Err("the port-forward stopped".into()),
            _ => {}
        }
        if start.elapsed() > FORWARD_TIMEOUT {
            cx.update(|cx| stop_forwards(vec![id], cx));
            return Err("the port-forward didn't start (needs create pods/portforward)".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    };
    let target = target.clone();
    let credentials = credentials.clone();
    let probed = cx
        .update(|cx| {
            spawn_kube(cx, async move {
                client::probe_forward(&client, &target, local_port, &credentials).await
            })
        })
        .await;
    match probed {
        Probed::Connected(conn) => Ok((*conn, id)),
        Probed::Failed(_, err) | Probed::NeedsForward(_, err) => {
            cx.update(|cx| stop_forwards(vec![id], cx));
            Err(err)
        }
    }
}

/// The number of a Service's named port.
async fn service_port(
    client: &kube::Client,
    namespace: &str,
    service: &str,
    name: &str,
) -> Option<u16> {
    let request = http::Request::get(format!("/api/v1/namespaces/{namespace}/services/{service}"))
        .body(Vec::new())
        .ok()?;
    let svc: Value = client.request(request).await.ok()?;
    svc.pointer("/spec/ports")?
        .as_array()?
        .iter()
        .find(|p| p["name"].as_str() == Some(name))
        .and_then(|p| p["port"].as_u64())
        .and_then(|p| u16::try_from(p).ok())
}

struct FetchInput {
    conns: Vec<AmConn>,
    prom: Option<PromClient>,
    want_rules: bool,
    rules: Arc<Vec<RuleGroup>>,
    matchers: Vec<Matcher>,
    settings: AlertsSettings,
    nodes: Arc<NodeMap>,
    prometheus_alertmanagers: Option<usize>,
}

struct FetchOutput {
    /// Per Alertmanager: its receivers when read this time, or its error.
    sources: Vec<Result<Option<Vec<String>>, String>>,
    silences: Vec<Silence>,
    rules: Option<Result<Vec<RuleGroup>, String>>,
    /// `None`: nothing answered (keep the last data).
    merged: Option<merge::Merged>,
    error: Option<String>,
    now: Timestamp,
}

/// Reads every source and merges (on the Tokio runtime).
async fn fetch(input: FetchInput) -> FetchOutput {
    let matchers = &input.matchers;
    let reads = input.conns.iter().map(|conn| async move {
        let (alerts, silences, receivers) = futures::join!(
            conn.alerts(matchers),
            conn.silences(matchers),
            conn.receivers()
        );
        (alerts, silences, receivers)
    });
    let prom_reads = async {
        let Some(prom) = &input.prom else {
            return (None, None);
        };
        let alerts = prom.api("/api/v1/alerts", &[]);
        let rules = async {
            if input.want_rules {
                Some(
                    prom.api(
                        "/api/v1/rules",
                        &[
                            ("type", "alert".to_string()),
                            ("exclude_alerts", "true".to_string()),
                        ],
                    )
                    .await,
                )
            } else {
                None
            }
        };
        let (alerts, rules) = futures::join!(alerts, rules);
        (Some(alerts), rules)
    };
    let (am, (prom_alerts, rules)) = futures::join!(futures::future::join_all(reads), prom_reads);

    let severities = input.settings.severities.clone();
    let nodes = input.nodes.clone();
    let node_of = move |instance: &str| nodes.node_of(instance);
    let parse = ParseOptions {
        severity_label: &input.settings.severity_label,
        severities: &severities,
        node_of: &node_of,
    };
    let mut errors = Vec::new();
    let mut am_alerts: Vec<Alert> = Vec::new();
    let mut silences: Vec<Silence> = Vec::new();
    let mut sources = Vec::new();
    let mut am_ok = false;
    for (conn, (alerts, silence_list, receivers)) in input.conns.iter().zip(am) {
        let label = conn.label();
        match alerts {
            Ok(body) => {
                am_ok = true;
                am_alerts.extend(model::parse_am_alerts(&body, &label, &parse));
                if let Ok(body) = silence_list {
                    silences.extend(model::parse_silences(&body, &label));
                }
                sources.push(Ok(receivers.ok().map(|b| model::parse_receivers(&b))));
            }
            Err(err) => {
                let message = client::explain(&conn.target, via_name(conn), &err);
                errors.push(format!("{label}: {message}"));
                sources.push(Err(message));
            }
        }
    }
    let prom_alerts: Option<Result<Vec<PromAlert>, String>> = prom_alerts.map(|r| {
        r.map_err(|e| e.to_string())
            .and_then(|b| model::parse_prom_alerts(&b))
    });
    let rules: Option<Result<Vec<RuleGroup>, String>> = rules.map(|r| {
        r.map_err(|e| e.to_string())
            .and_then(|b| model::parse_rules(&b))
    });
    if let Some(Err(err)) = &prom_alerts {
        errors.push(format!("Prometheus: {err}"));
    }
    let current_rules: &[RuleGroup] = match &rules {
        Some(Ok(rules)) => rules,
        _ => &input.rules,
    };
    let now = Timestamp::now();
    let prom_ok = matches!(prom_alerts, Some(Ok(_)));
    let merged = if am_ok || prom_ok {
        let options = MergeOptions {
            heartbeat_alerts: &input.settings.heartbeat_alerts,
            hidden_alerts: &input.settings.hidden_alerts,
            parse: &parse,
        };
        let prom_list = match &prom_alerts {
            Some(Ok(list)) => Some(list.as_slice()),
            _ => None,
        };
        Some(merge::merge(
            (!input.conns.is_empty() && am_ok).then_some(am_alerts.as_slice()),
            prom_list,
            current_rules,
            input.prometheus_alertmanagers,
            &options,
            now,
        ))
    } else {
        None
    };
    silences.sort_by_key(|s| std::cmp::Reverse(s.starts_at));
    FetchOutput {
        sources,
        silences,
        rules,
        merged,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
        now,
    }
}

fn via_name(conn: &AmConn) -> &'static str {
    match conn.via {
        client::Via::Proxy => "proxy",
        client::Via::Route { .. } => "route",
        client::Via::Forward { .. } => "forward",
        client::Via::Url => "url",
    }
}

/// Per-cluster counts for chrome (badges, markers), without marking demand.
pub fn counts(cluster: &ClusterId, cx: &App) -> Option<Counts> {
    AlertsService::global(cx)?.read(cx).counts(cluster)
}

/// Firing alerts of `cluster`, most severe first, for tooltips and cards.
pub fn top_alerts(cluster: &ClusterId, n: usize, cx: &App) -> Vec<Alert> {
    let Some(service) = AlertsService::global(cx) else {
        return Vec::new();
    };
    let service = service.read(cx);
    let Some(state) = service.clusters.get(cluster) else {
        return Vec::new();
    };
    state
        .alerts
        .iter()
        .filter(|a| matches!(a.state, AlertState::Firing | AlertState::Unprocessed))
        .take(n)
        .cloned()
        .collect()
}

/// The user a silence names as its creator: from `SelfSubjectReview`, else the kubeconfig user.
pub fn created_by(cluster: &ClusterId, cx: &App) -> (String, &'static str) {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return (String::new(), "");
    };
    let manager = manager.read(cx);
    if let Some(user) = manager
        .cluster(cluster)
        .and_then(|c| c.info.as_ref()?.user.clone())
        .filter(|u| !u.is_empty())
    {
        return (user, "from your sign-in");
    }
    match manager.context(cluster).and_then(|c| c.user.clone()) {
        Some(user) if !user.is_empty() => (user, "the kubeconfig user"),
        _ => (String::new(), ""),
    }
}

/// The active alerts of one object (the details section).
pub fn alerts_for(
    cluster: &ClusterId,
    object: &crate::view::rows::ObjectFilter,
    cx: &App,
) -> Vec<Alert> {
    let Some(service) = AlertsService::global(cx) else {
        return Vec::new();
    };
    let service = service.read(cx);
    let Some(state) = service.cluster(cluster, Pace::Background) else {
        return Vec::new();
    };
    state
        .alerts
        .iter()
        .filter(|a| {
            matches!(
                a.state,
                AlertState::Firing | AlertState::Pending | AlertState::Unprocessed
            )
        })
        .filter(|a| object.matches(a))
        .cloned()
        .collect()
}

/// Severity counts by namespace (`None`: every namespace).
pub fn namespace_counts(alerts: &[Alert], namespace: Option<&str>) -> Counts {
    match namespace {
        None => Counts::of(alerts),
        Some(ns) => {
            let scoped: Vec<Alert> = alerts
                .iter()
                .filter(|a| a.namespace() == Some(ns))
                .cloned()
                .collect();
            Counts::of(&scoped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use gpui::TestAppContext;

    fn firing(name: &str, severity: Severity) -> Alert {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("alertname".to_string(), name.to_string());
        labels.insert("namespace".to_string(), "payments".to_string());
        Alert {
            fingerprint: format!("fp-{name}"),
            name: name.into(),
            labels,
            annotations: Default::default(),
            severity,
            state: AlertState::Firing,
            silenced_by: Vec::new(),
            inhibited_by: Vec::new(),
            starts_at: Some(Timestamp::now()),
            starts_approx: false,
            active_at: None,
            updated_at: None,
            ends_at: None,
            receivers: Vec::new(),
            generator_url: None,
            value: None,
            source: "monitoring/am".into(),
            target: None,
        }
    }

    fn output(alerts: Vec<Alert>) -> FetchOutput {
        FetchOutput {
            sources: Vec::new(),
            silences: Vec::new(),
            rules: None,
            merged: Some(merge::Merged {
                alerts,
                heartbeat: None,
            }),
            error: None,
            now: Timestamp::now(),
        }
    }

    fn setup(
        cx: &mut TestAppContext,
        settings: serde_json::Value,
    ) -> (tempfile::TempDir, Entity<AlertsService>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
        let service = cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            Settings::register::<AlertsSettings>(cx);
            AlertsService::install(false, cx)
        });
        (dir, service)
    }

    #[gpui::test]
    fn only_alerts_that_start_after_the_first_read_notify(cx: &mut TestAppContext) {
        let (_dir, service) = setup(
            cx,
            serde_json::json!({"alerts": {"notify": {"enabled": true, "clusters": "all", "min_severity": "warning", "resolved": true}}}),
        );
        let cluster = ClusterId::new("c");
        let before = cx.update(|cx| NotificationCenter::global(cx).latest_id());
        service.update(cx, |s, cx| {
            s.insert_for_test(&cluster, Vec::new(), cx);
            let generation = s.clusters[&cluster].generation;
            // The first read is the baseline: nothing is new.
            s.fetched(
                &cluster,
                generation,
                output(vec![firing("Old", Severity::Critical)]),
                cx,
            );
            s.flush_notices(cx);
        });
        let after_baseline = cx.update(|cx| NotificationCenter::global(cx).latest_id());
        assert_eq!(
            after_baseline, before,
            "no toast for alerts firing when Kubyl first looked"
        );

        service.update(cx, |s, cx| {
            let generation = s.clusters[&cluster].generation;
            s.fetched(
                &cluster,
                generation,
                output(vec![
                    firing("Old", Severity::Critical),
                    firing("New", Severity::Warning),
                    firing("Quiet", Severity::Info),
                ]),
                cx,
            );
            s.flush_notices(cx);
        });
        let messages: Vec<String> = cx.update(|cx| {
            NotificationCenter::global(cx)
                .since(after_baseline)
                .map(|(_, n)| n.message.to_string())
                .collect()
        });
        assert_eq!(
            messages,
            ["New started firing"],
            "info is below min_severity"
        );

        // Resolved: listed for a while, and batched into the next toast (a minute later).
        service.update(cx, |s, cx| {
            let generation = s.clusters[&cluster].generation;
            s.fetched(
                &cluster,
                generation,
                output(vec![firing("New", Severity::Warning)]),
                cx,
            );
            s.flush_notices(cx);
            let state = &s.clusters[&cluster];
            let resolved: Vec<&str> = state.resolved.iter().map(|a| a.name.as_str()).collect();
            assert_eq!(resolved, ["Old", "Quiet"]);
            assert!(
                state
                    .resolved
                    .iter()
                    .all(|a| a.state == AlertState::Resolved && a.ends_at.is_some())
            );
            assert!(
                s.notices.contains_key(&cluster),
                "held back: one toast per minute"
            );
        });
        // A stale generation is dropped.
        service.update(cx, |s, cx| {
            let generation = s.clusters[&cluster].generation;
            s.fetched(&cluster, generation + 1, output(Vec::new()), cx);
            assert_eq!(s.clusters[&cluster].alerts.len(), 1);
        });
    }

    #[gpui::test]
    fn writes_show_right_away_and_read_again(cx: &mut TestAppContext) {
        let (_dir, service) = setup(cx, serde_json::json!({}));
        let cluster = ClusterId::new("c");
        service.update(cx, |s, cx| {
            s.insert_for_test(
                &cluster,
                vec![
                    firing("A", Severity::Critical),
                    firing("B", Severity::Warning),
                ],
                cx,
            );
            let matchers = [Matcher::new(
                "alertname",
                crate::matchers::MatchOp::Equal,
                "A",
            )];
            let now = Timestamp::now();
            let body = crate::client::silence_body(
                None,
                &matchers,
                now,
                now.checked_add(jiff::SignedDuration::from_hours(1))
                    .unwrap(),
                "alice@example.com",
                "test",
            );
            s.silence_written(&cluster, "monitoring/am", "s1", Some(&body), cx);
            let state = &s.clusters[&cluster];
            assert!(
                state.refetch_at.is_some(),
                "read again shortly (HA replicas)"
            );
            let a = state.alerts.iter().find(|a| a.name == "A").unwrap();
            assert_eq!(a.state, AlertState::Silenced);
            assert_eq!(a.silenced_by, ["s1"]);
            assert_eq!(
                state.alerts.iter().find(|a| a.name == "B").unwrap().state,
                AlertState::Firing
            );
            assert_eq!(state.silences[0].id, "s1");
            assert_eq!(state.silences[0].state, "active");

            s.silence_written(&cluster, "monitoring/am", "s1", None, cx);
            let state = &s.clusters[&cluster];
            assert_eq!(
                state.alerts.iter().find(|a| a.name == "A").unwrap().state,
                AlertState::Firing
            );
            assert_eq!(state.silences[0].state, "expired");
        });
    }

    #[test]
    fn nodes_by_name_and_address() {
        let nodes = NodeMap::from_list(&[json!({
            "metadata": {"name": "ip-10-0-15-3"},
            "status": {"addresses": [{"type": "InternalIP", "address": "10.0.15.3"}]}
        })]);
        assert_eq!(
            nodes.node_of("10.0.15.3:9100").as_deref(),
            Some("ip-10-0-15-3")
        );
        assert_eq!(
            nodes.node_of("ip-10-0-15-3").as_deref(),
            Some("ip-10-0-15-3")
        );
        assert_eq!(
            nodes.node_of("ip-10-0-15-3:9100").as_deref(),
            Some("ip-10-0-15-3")
        );
        assert_eq!(nodes.node_of("10.0.15.4:9100"), None);
    }

    #[test]
    fn counts_by_state_and_severity() {
        let alert = |severity: Severity, state: AlertState| Alert {
            fingerprint: String::new(),
            name: "A".into(),
            labels: Default::default(),
            annotations: Default::default(),
            severity,
            state,
            silenced_by: Vec::new(),
            inhibited_by: Vec::new(),
            starts_at: None,
            starts_approx: false,
            active_at: None,
            updated_at: None,
            ends_at: None,
            receivers: Vec::new(),
            generator_url: None,
            value: None,
            source: String::new(),
            target: None,
        };
        let counts = Counts::of(&[
            alert(Severity::Critical, AlertState::Firing),
            alert(Severity::Warning, AlertState::Firing),
            alert(Severity::Warning, AlertState::Pending),
            alert(Severity::Critical, AlertState::Silenced),
            alert(Severity::Info, AlertState::Inhibited),
        ]);
        assert_eq!(counts.firing(), 2);
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.silenced, 1);
        assert_eq!(counts.inhibited, 1);
        assert_eq!(counts.worst(), Some(Severity::Critical));
        assert_eq!(Counts::default().worst(), None);
    }

    #[test]
    fn headers_are_keyed_by_the_url_the_probe_uses() {
        use secrecy::ExposeSecret as _;
        let config = crate::settings::AlertmanagerConfig {
            url: Some("https://mimir.example.com/".into()),
            path: Some("alertmanager".into()),
            ..Default::default()
        };
        let api = crate::discover::url_of(&config).unwrap();
        assert_eq!(api, "https://mimir.example.com/alertmanager");
        let target = crate::discover::from_settings(std::slice::from_ref(&config))
            .remove(0)
            .target;
        assert!(matches!(&target, AmTarget::Url { url, .. } if *url == api));
        let ids = vec!["shop/c/u@/k".to_string()];
        let urls = vec![(config.url.clone().unwrap(), api.clone())];
        // Saved under the URL as written in settings (what "Set … Header" stores).
        let saved = auth_key(&ids[0], "https://mimir.example.com/");
        let headers = headers_for(&ids, &urls, |key| {
            (key == saved).then(|| SecretString::from("Bearer t".to_string()))
        });
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, api, "the probe finds it by the API URL");
        assert_eq!(headers[0].1.expose_secret(), "Bearer t");
    }

    #[test]
    fn keychain_keys_name_the_cluster_and_url() {
        assert_eq!(
            auth_key("shop/c/u@/k", "https://am.example.com"),
            "alerts-auth:shop/c/u@/k/https://am.example.com"
        );
    }
}
