//! The metrics cache: one entry per cluster, filled on demand in the background.
//!
//! Views never fetch metrics themselves. They read from [`MetricsService`] (or through the
//! [`MetricsProvider`](kubyl_resources::metrics::MetricsProvider) it backs), and every read marks
//! the data as *wanted*. Once a second the service looks at what was wanted in the last
//! [`DEMAND_TTL`] and refreshes whatever is stale, so the same numbers are fetched once no matter
//! how many views show them, and nothing is fetched for clusters nobody looks at.
//!
//! Per cluster, the source is detected first (settings override, else Prometheus discovery,
//! else metrics-server), then:
//! - current pod and node usage every `refresh_interval` (Prometheus instant queries, or
//!   metrics-server);
//! - pod histories for the details dock (Prometheus range queries; with metrics-server, the
//!   samples collected while the pod was shown);
//! - range queries for the overview charts, cached per (query, filters, time range) and
//!   refreshed at the range's pace.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Subscription, Task};
use kubyl_charts::TimeRange;
use kubyl_charts::data::align;
use kubyl_core::{ClusterId, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::metrics::{Metrics, Usage, UsageHistory};
use kubyl_resources::{ObjectKey, object_key};
use kubyl_settings::Settings;
use secrecy::SecretString;

use crate::discover::{self, Found};
use crate::metrics_server;
use crate::openshift;
use crate::prometheus::{PromClient, PromError, RangeSeries, Sample, Target};
use crate::queries::Queries;
use crate::settings::{MetricsSettings, PrometheusOverride, SourcePreference};

/// Data asked for within this long is kept fresh.
pub const DEMAND_TTL: Duration = Duration::from_secs(30);
/// Cached data nobody asked for within this long is dropped.
const KEEP: Duration = Duration::from_secs(300);
/// How often a cluster without Prometheus is searched again.
const REDETECT: Duration = Duration::from_secs(300);
/// Consecutive target failures before Prometheus is searched again. Any answer resets the count.
const MAX_FAILURES: u32 = 3;
/// Pod histories (details dock): window and resolution.
const HISTORY_WINDOW: Duration = Duration::from_secs(3600);
const HISTORY_STEP: f64 = 60.0;
const HISTORY_REFRESH: Duration = Duration::from_secs(60);
/// metrics-server samples kept per pod / for cluster totals.
const MAX_SAMPLES: usize = 120;
/// Namespaces fetched one by one when metrics-server forbids listing across namespaces.
const MAX_NAMESPACES: usize = 20;

/// Keychain entry of the Authorization header for a cluster's external Prometheus URL.
pub fn auth_key(cluster: &ClusterId) -> String {
    format!("metrics-auth:{cluster}")
}

/// Where a cluster's usage comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Not looked for yet (nobody asked, or the cluster isn't connected).
    Unknown,
    Detecting,
    Prometheus {
        target: Target,
    },
    /// Current values only. `note` says why Prometheus isn't used.
    MetricsServer {
        note: SharedString,
    },
    /// No metrics.
    None {
        reason: SharedString,
    },
}

impl Source {
    /// Short label for chips and the status bar.
    pub fn label(&self) -> SharedString {
        match self {
            Source::Unknown => "".into(),
            Source::Detecting => "Looking for metrics…".into(),
            Source::Prometheus { .. } => "Prometheus".into(),
            Source::MetricsServer { .. } => "metrics-server".into(),
            Source::None { .. } => "No metrics".into(),
        }
    }

    /// Prometheus: charts and time ranges work.
    pub fn has_history(&self) -> bool {
        matches!(self, Source::Prometheus { .. })
    }

    pub fn has_usage(&self) -> bool {
        matches!(
            self,
            Source::Prometheus { .. } | Source::MetricsServer { .. }
        )
    }
}

/// One range query of a chart.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RangeKey {
    /// A [`crate::queries::LIBRARY`] id.
    pub query: &'static str,
    pub filters: Vec<(String, String)>,
    pub range: TimeRange,
}

impl RangeKey {
    pub fn new(query: &'static str, range: TimeRange) -> Self {
        Self {
            query,
            filters: Vec::new(),
            range,
        }
    }

    pub fn filter(mut self, label: &str, value: &str) -> Self {
        self.filters.push((label.to_string(), value.to_string()));
        self
    }
}

/// A range query's answer, on the grid `start..=end` every `step` seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeResult {
    pub series: Vec<RangeSeries>,
    pub start: f64,
    pub end: f64,
    pub step: f64,
}

impl RangeResult {
    /// The grid timestamps.
    pub fn times(&self) -> Vec<f64> {
        kubyl_charts::data::grid(self.start, self.end, self.step)
    }

    /// A series aligned to the grid.
    pub fn aligned(&self, series: &RangeSeries) -> Vec<Option<f64>> {
        align(&series.values, self.start, self.end, self.step)
    }

    /// The sum over all series, aligned (e.g. a single-series query).
    pub fn total(&self) -> Vec<Option<f64>> {
        let mut out: Vec<Option<f64>> = vec![None; self.times().len()];
        for series in &self.series {
            for (slot, value) in out.iter_mut().zip(self.aligned(series)) {
                if let Some(value) = value {
                    *slot = Some(slot.unwrap_or(0.0) + value);
                }
            }
        }
        out
    }
}

/// What a chart can show right now.
#[derive(Clone, Debug, PartialEq)]
pub enum RangeState<'a> {
    /// The cluster has no Prometheus (or it's still being looked for).
    Unavailable,
    Loading,
    Ready(&'a RangeResult),
    Failed(SharedString),
}

#[derive(Default)]
struct Fetch {
    last: Option<Instant>,
    in_flight: bool,
    error: Option<SharedString>,
}

impl Fetch {
    fn due(&self, every: Duration) -> bool {
        !self.in_flight && self.last.is_none_or(|last| last.elapsed() >= every)
    }

    fn start(&mut self) {
        self.in_flight = true;
    }

    fn finish(&mut self, error: Option<SharedString>) {
        self.in_flight = false;
        self.last = Some(Instant::now());
        self.error = error;
    }
}

#[derive(Default)]
struct HistoryEntry {
    data: Option<UsageHistory>,
    fetch: Fetch,
}

#[derive(Default)]
struct RangeEntry {
    result: Option<RangeResult>,
    fetch: Fetch,
}

/// Everything known about one cluster's metrics.
struct ClusterMetrics {
    source: Source,
    /// Bumped on every reset; results of older fetches are dropped.
    generation: u64,
    detected_at: Option<Instant>,
    prom: Option<PromClient>,
    queries: Queries,
    failures: u32,
    pods: HashMap<ObjectKey, Usage>,
    pods_fetch: Fetch,
    /// The scope of the last pod fetch: a wider one is fetched right away.
    pods_scope: Option<PodScope>,
    /// metrics-server forbade listing across namespaces: fetch per namespace.
    pods_per_namespace: bool,
    nodes: HashMap<String, Usage>,
    nodes_fetch: Fetch,
    histories: HashMap<ObjectKey, HistoryEntry>,
    /// metrics-server samples of pods whose history was asked for.
    samples: HashMap<ObjectKey, VecDeque<(f64, Usage)>>,
    /// metrics-server samples of the cluster total (sum over nodes).
    totals: VecDeque<(f64, Usage)>,
    ranges: HashMap<RangeKey, RangeEntry>,
}

impl ClusterMetrics {
    fn new(generation: u64) -> Self {
        Self {
            source: Source::Unknown,
            generation,
            detected_at: None,
            prom: None,
            queries: Queries::default(),
            failures: 0,
            pods: HashMap::new(),
            pods_fetch: Fetch::default(),
            pods_scope: None,
            pods_per_namespace: false,
            nodes: HashMap::new(),
            nodes_fetch: Fetch::default(),
            histories: HashMap::new(),
            samples: HashMap::new(),
            totals: VecDeque::new(),
            ranges: HashMap::new(),
        }
    }
}

/// What views asked for, with the time they last asked.
#[derive(Default)]
struct Demand {
    source: HashMap<ClusterId, Instant>,
    /// Pod usage per namespace asked about; `None` = every namespace.
    pods: HashMap<ClusterId, HashMap<Option<String>, Instant>>,
    nodes: HashMap<ClusterId, Instant>,
    histories: HashMap<(ClusterId, ObjectKey), Instant>,
    ranges: HashMap<(ClusterId, RangeKey), Instant>,
}

impl Demand {
    fn clusters(&self) -> HashSet<ClusterId> {
        let fresh = |t: &Instant| t.elapsed() < DEMAND_TTL;
        self.source
            .iter()
            .filter(|(_, t)| fresh(t))
            .map(|(c, _)| c.clone())
            .chain(
                self.pods
                    .iter()
                    .filter(|(_, scopes)| scopes.values().any(fresh))
                    .map(|(c, _)| c.clone()),
            )
            .chain(
                self.nodes
                    .iter()
                    .filter(|(_, t)| fresh(t))
                    .map(|(c, _)| c.clone()),
            )
            .chain(
                self.histories
                    .iter()
                    .filter(|(_, t)| fresh(t))
                    .map(|((c, _), _)| c.clone()),
            )
            .chain(
                self.ranges
                    .iter()
                    .filter(|(_, t)| fresh(t))
                    .map(|((c, _), _)| c.clone()),
            )
            .collect()
    }

    fn prune(&mut self) {
        let keep = |t: &Instant| t.elapsed() < KEEP;
        self.source.retain(|_, t| keep(t));
        self.pods.retain(|_, scopes| {
            scopes.retain(|_, t| keep(t));
            !scopes.is_empty()
        });
        self.nodes.retain(|_, t| keep(t));
        self.histories.retain(|_, t| keep(t));
        self.ranges.retain(|_, t| keep(t));
    }
}

/// The app-wide metrics cache. See the module docs.
pub struct MetricsService {
    clusters: HashMap<ClusterId, ClusterMetrics>,
    demand: RefCell<Demand>,
    settings: MetricsSettings,
    generation: u64,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

struct GlobalService(Entity<MetricsService>);

impl Global for GlobalService {}

/// Unix seconds now.
pub fn now() -> f64 {
    jiff::Timestamp::now().as_millisecond() as f64 / 1000.0
}

impl MetricsService {
    /// Creates the service and makes it global. `tick` starts the refresh loop (off in tests
    /// that must not wake timers).
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
        subscriptions.push(Settings::observe::<MetricsSettings>(
            cx,
            move |settings, cx| {
                let settings = settings.clone();
                weak.update(cx, |this, cx| {
                    this.settings = settings;
                    this.reset_all(cx);
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
            settings: Settings::get::<MetricsSettings>(cx).clone(),
            generation: 0,
            _tick: task,
            _subscriptions: subscriptions,
        }
    }

    // ----- Reading (marks demand) -----

    /// The cluster's source. Asking keeps detection going for it.
    pub fn source(&self, cluster: &ClusterId) -> Source {
        self.demand
            .borrow_mut()
            .source
            .insert(cluster.clone(), Instant::now());
        self.clusters
            .get(cluster)
            .map(|c| c.source.clone())
            .unwrap_or(Source::Unknown)
    }

    /// Current usage of one pod.
    pub fn pod_usage(&self, cluster: &ClusterId, namespace: &str, name: &str) -> Option<Usage> {
        self.want_pods(cluster, Some(namespace));
        self.clusters
            .get(cluster)?
            .pods
            .get(&object_key(Some(namespace), name))
            .copied()
    }

    /// Current usage of every pod (`namespace/name`), if known.
    pub fn pods(&self, cluster: &ClusterId) -> Option<&HashMap<ObjectKey, Usage>> {
        self.pods_in(cluster, None)
    }

    /// Current pod usage, asking only for `namespace` (`None`: every namespace). The map may
    /// hold other namespaces too; filter by the `namespace/` key prefix.
    pub fn pods_in(
        &self,
        cluster: &ClusterId,
        namespace: Option<&str>,
    ) -> Option<&HashMap<ObjectKey, Usage>> {
        self.want_pods(cluster, namespace);
        let state = self.clusters.get(cluster)?;
        state.pods_fetch.last.map(|_| &state.pods)
    }

    /// Marks pod usage in `namespace` (`None`: every namespace) as wanted.
    fn want_pods(&self, cluster: &ClusterId, namespace: Option<&str>) {
        self.demand
            .borrow_mut()
            .pods
            .entry(cluster.clone())
            .or_default()
            .insert(namespace.map(str::to_string), Instant::now());
    }

    /// Current usage of one node.
    pub fn node_usage(&self, cluster: &ClusterId, name: &str) -> Option<Usage> {
        self.nodes(cluster)?.get(name).copied()
    }

    /// Current usage of every node, if known.
    pub fn nodes(&self, cluster: &ClusterId) -> Option<&HashMap<String, Usage>> {
        self.demand
            .borrow_mut()
            .nodes
            .insert(cluster.clone(), Instant::now());
        let state = self.clusters.get(cluster)?;
        state.nodes_fetch.last.map(|_| &state.nodes)
    }

    /// Recent CPU/memory of a pod (last hour from Prometheus, or the metrics-server samples
    /// collected since it was first shown).
    pub fn pod_history(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        name: &str,
    ) -> Option<UsageHistory> {
        let key = object_key(Some(namespace), name);
        self.demand
            .borrow_mut()
            .histories
            .insert((cluster.clone(), key.clone()), Instant::now());
        self.want_pods(cluster, Some(namespace));
        let state = self.clusters.get(cluster)?;
        match &state.source {
            Source::Prometheus { .. } => state.histories.get(&key)?.data.clone(),
            Source::MetricsServer { .. } => {
                let samples = state.samples.get(&key)?;
                (samples.len() >= 2).then(|| {
                    let span = samples.back()?.0 - samples.front()?.0;
                    Some(UsageHistory {
                        cpu: samples.iter().map(|(_, u)| u.cpu).collect(),
                        memory: samples.iter().map(|(_, u)| u.memory).collect(),
                        source: "metrics-server".into(),
                        window: format!("last {}", human_span(span)).into(),
                    })
                })?
            }
            _ => None,
        }
    }

    /// A chart's range query.
    pub fn range(&self, cluster: &ClusterId, key: &RangeKey) -> RangeState<'_> {
        self.demand
            .borrow_mut()
            .ranges
            .insert((cluster.clone(), key.clone()), Instant::now());
        let Some(state) = self.clusters.get(cluster) else {
            return RangeState::Loading;
        };
        match &state.source {
            Source::Prometheus { .. } => {}
            Source::Unknown | Source::Detecting => return RangeState::Loading,
            _ => return RangeState::Unavailable,
        }
        match state.ranges.get(key) {
            Some(RangeEntry {
                result: Some(result),
                ..
            }) => RangeState::Ready(result),
            Some(RangeEntry { fetch, .. }) if fetch.error.is_some() => {
                RangeState::Failed(fetch.error.clone().unwrap_or_default())
            }
            _ => RangeState::Loading,
        }
    }

    /// Cluster totals sampled from metrics-server while someone watched (oldest first).
    pub fn sampled_totals(&self, cluster: &ClusterId) -> Vec<(f64, Usage)> {
        self.clusters
            .get(cluster)
            .map(|c| c.totals.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The last error of the current-usage fetch, if any.
    pub fn error(&self, cluster: &ClusterId) -> Option<SharedString> {
        let state = self.clusters.get(cluster)?;
        state
            .pods_fetch
            .error
            .clone()
            .or_else(|| state.nodes_fetch.error.clone())
    }

    /// Whether the cluster's Prometheus has series named `metric` (node-exporter, PSI…).
    pub fn has_metric(&self, cluster: &ClusterId, metric: &str) -> bool {
        self.clusters
            .get(cluster)
            .is_some_and(|c| c.queries.has(metric))
    }

    // ----- Control -----

    /// Forgets the cluster's source and data and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.generation += 1;
        self.clusters
            .insert(cluster.clone(), ClusterMetrics::new(self.generation));
        self.demand
            .borrow_mut()
            .source
            .insert(cluster.clone(), Instant::now());
        self.tick(cx);
        Metrics::changed(cx);
        cx.notify();
    }

    fn reset_all(&mut self, cx: &mut Context<Self>) {
        let clusters: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for cluster in clusters {
            self.generation += 1;
            self.clusters
                .insert(cluster, ClusterMetrics::new(self.generation));
        }
        self.tick(cx);
        Metrics::changed(cx);
        cx.notify();
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                // A reconnect may point at a different cluster (kubeconfig edited); start over.
                if !connected && self.clusters.remove(id).is_some() {
                    Metrics::changed(cx);
                    cx.notify();
                }
            }
            ConnectionEvent::DiscoveryChanged(id) => {
                // metrics-server (or a Prometheus) may have been installed.
                if matches!(
                    self.clusters.get(id).map(|c| &c.source),
                    Some(Source::None { .. } | Source::MetricsServer { .. })
                ) {
                    self.generation += 1;
                    self.clusters
                        .insert(id.clone(), ClusterMetrics::new(self.generation));
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    // ----- The refresh loop -----

    fn tick(&mut self, cx: &mut Context<Self>) {
        let wanted = self.demand.borrow().clusters();
        self.demand.borrow_mut().prune();
        self.clusters.retain(|id, state| {
            wanted.contains(id)
                || state.pods_fetch.last.is_some_and(|t| t.elapsed() < KEEP)
                || state.nodes_fetch.last.is_some_and(|t| t.elapsed() < KEEP)
        });
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        for cluster in wanted {
            let Some(client) = manager.read(cx).client(&cluster) else {
                continue;
            };
            let generation = self.generation;
            let state = self
                .clusters
                .entry(cluster.clone())
                .or_insert_with(|| ClusterMetrics::new(generation));
            let redetect = match &state.source {
                Source::Unknown => true,
                Source::None { .. } | Source::MetricsServer { .. } => {
                    state.detected_at.is_none_or(|t| t.elapsed() >= REDETECT)
                }
                _ => false,
            };
            if redetect {
                self.detect(&cluster, client, cx);
                continue;
            }
            self.refresh(&cluster, client, cx);
        }
    }

    fn detect(&mut self, cluster: &ClusterId, client: kube::Client, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let manager = ConnectionManager::global(cx);
        let (context, metrics_server, user_token) = {
            let manager = manager.read(cx);
            (
                manager
                    .context(cluster)
                    .map(|c| c.context.clone())
                    .unwrap_or_default(),
                manager.caps(cluster).metrics_server,
                manager.bearer_token(cluster),
            )
        };
        let settings = self.settings.clone();
        let override_ = settings
            .prometheus_for(cluster.as_str(), &context)
            .cloned()
            .unwrap_or_default();
        let generation = state.generation;
        let first = matches!(state.source, Source::Unknown);
        state.source = if first {
            Source::Detecting
        } else {
            state.source.clone()
        };
        state.detected_at = Some(Instant::now());
        let auth_key = auth_key(cluster);
        let task = spawn_kube(cx, async move {
            let auth = Credentials {
                keychain_key: auth_key,
                user_token,
            };
            detect(client, settings, override_, auth, metrics_server).await
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.clusters.get_mut(&cluster) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                let before = state.source.clone();
                match outcome {
                    Detected::Prometheus(prom, rules) => {
                        state.source = Source::Prometheus {
                            target: prom.target().clone(),
                        };
                        state.queries = Queries::new(this.settings.queries.clone(), rules);
                        state.prom = Some(*prom);
                        state.failures = 0;
                    }
                    Detected::MetricsServer(note) => {
                        state.prom = None;
                        state.source = Source::MetricsServer { note: note.into() };
                    }
                    Detected::None(reason) => {
                        state.prom = None;
                        state.source = Source::None {
                            reason: reason.into(),
                        };
                    }
                }
                if state.source != before {
                    // Different source: current values come from somewhere else now.
                    state.pods_fetch = Fetch::default();
                    state.pods_scope = None;
                    state.nodes_fetch = Fetch::default();
                    state.histories.clear();
                    state.ranges.clear();
                    tracing::info!(%cluster, source = %state.source.label(), "metrics source");
                    Metrics::changed(cx);
                }
                cx.notify();
                this.tick(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Starts every due fetch of one cluster.
    fn refresh(&mut self, cluster: &ClusterId, client: kube::Client, cx: &mut Context<Self>) {
        let every = Duration::from_secs(self.settings.refresh_interval.max(5));
        let demand = self.demand.borrow();
        let fresh = |t: &Instant| t.elapsed() < DEMAND_TTL;
        // Only the namespaces views look at, unless something wants them all (the overview)
        // or there are too many to list.
        let pods_wanted: Option<PodScope> = demand.pods.get(cluster).and_then(|scopes| {
            let fresh_scopes: Vec<&Option<String>> = scopes
                .iter()
                .filter(|(_, t)| fresh(t))
                .map(|(scope, _)| scope)
                .collect();
            let mut namespaces: Vec<String> =
                fresh_scopes.iter().copied().flatten().cloned().collect();
            namespaces.sort();
            if fresh_scopes.is_empty() {
                None
            } else if fresh_scopes.iter().any(|s| s.is_none()) || namespaces.len() > MAX_NAMESPACES
            {
                namespaces.truncate(MAX_NAMESPACES);
                Some(PodScope::All(namespaces))
            } else {
                Some(PodScope::Namespaces(namespaces))
            }
        });
        let nodes_wanted = demand.nodes.get(cluster).is_some_and(fresh);
        let histories: Vec<ObjectKey> = demand
            .histories
            .iter()
            .filter(|((c, _), t)| c == cluster && fresh(t))
            .map(|((_, key), _)| key.clone())
            .collect();
        let ranges: Vec<RangeKey> = demand
            .ranges
            .iter()
            .filter(|((c, _), t)| c == cluster && fresh(t))
            .map(|((_, key), _)| key.clone())
            .collect();
        drop(demand);

        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        // Forget what nobody looks at anymore.
        state.histories.retain(|key, _| histories.contains(key));
        state.samples.retain(|key, _| histories.contains(key));
        state
            .ranges
            .retain(|key, entry| ranges.contains(key) || entry.fetch.in_flight);

        let generation = state.generation;
        let prom = match &state.source {
            Source::Prometheus { .. } => state.prom.clone(),
            Source::MetricsServer { .. } => None,
            _ => return,
        };
        let per_namespace = state.pods_per_namespace;
        let queries = state.queries.clone();
        // A view that asks for a namespace the last fetch didn't cover doesn't wait for the
        // next interval.
        let scope_grew = match (&pods_wanted, &state.pods_scope) {
            (Some(wanted), Some(fetched)) => !fetched.covers(wanted, state.pods_per_namespace),
            _ => false,
        };
        let pods_due = pods_wanted.is_some()
            && (state.pods_fetch.due(every) || (scope_grew && !state.pods_fetch.in_flight));
        if pods_due {
            state.pods_fetch.start();
            state.pods_scope = pods_wanted.clone();
        }
        let nodes_due = nodes_wanted && state.nodes_fetch.due(every);
        if nodes_due {
            state.nodes_fetch.start();
        }

        match prom {
            Some(prom) => {
                if pods_due {
                    let (prom, queries) = (prom.clone(), queries.clone());
                    let filter = match &pods_wanted {
                        Some(PodScope::Namespaces(namespaces)) => {
                            Some(("namespace=~".to_string(), namespaces.join("|")))
                        }
                        _ => None,
                    };
                    let task = spawn_kube(cx, async move {
                        let filters: Vec<(&str, &str)> = filter
                            .iter()
                            .map(|(k, v)| (k.as_str(), v.as_str()))
                            .collect();
                        prom_usage(&prom, &queries, "pod_cpu", "pod_memory", &filters, |s| {
                            let ns = s.labels.get("namespace")?;
                            let pod = s.labels.get("pod")?;
                            Some(object_key(Some(ns), pod))
                        })
                        .await
                    });
                    self.finish_pods(cluster, generation, task, cx);
                }
                if nodes_due {
                    let task = spawn_kube(cx, async move {
                        prom_usage(&prom, &queries, "node_cpu", "node_memory", &[], |s| {
                            s.labels.get("node").cloned()
                        })
                        .await
                    });
                    self.finish_nodes(cluster, generation, task, cx);
                }
                for key in histories {
                    self.refresh_history(cluster, key, cx);
                }
                for key in ranges {
                    self.refresh_range(cluster, key, cx);
                }
            }
            None => {
                if pods_due {
                    let client = client.clone();
                    let scope = pods_wanted.clone().unwrap_or(PodScope::All(Vec::new()));
                    let task = spawn_kube(cx, async move {
                        server_pods(&client, per_namespace, scope).await
                    });
                    self.finish_server_pods(cluster, generation, task, cx);
                }
                if nodes_due {
                    let task = spawn_kube(cx, async move {
                        metrics_server::nodes(&client)
                            .await
                            .map_err(|e| PromError::Transport(e.message))
                    });
                    self.finish_nodes(cluster, generation, task, cx);
                }
            }
        }
    }

    fn finish_pods(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        task: Task<Result<HashMap<ObjectKey, Usage>, PromError>>,
        cx: &mut Context<Self>,
    ) {
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.current(&cluster, generation) else {
                    return;
                };
                match result {
                    Ok(pods) => {
                        state.pods = pods;
                        state.pods_fetch.finish(None);
                        state.failures = 0;
                    }
                    Err(err) => {
                        tracing::debug!(%cluster, %err, "pod usage failed");
                        state.pods_fetch.finish(Some(err.to_string().into()));
                        this.failed(&cluster, &err);
                    }
                }
                Metrics::changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn finish_server_pods(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        task: Task<Result<ServerPods, metrics_server::FetchError>>,
        cx: &mut Context<Self>,
    ) {
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.current(&cluster, generation) else {
                    return;
                };
                match result {
                    Ok((pods, per_namespace)) => {
                        let time = now();
                        for (key, samples) in state.samples.iter_mut() {
                            if let Some(usage) = pods.get(key) {
                                samples.push_back((time, *usage));
                                while samples.len() > MAX_SAMPLES {
                                    samples.pop_front();
                                }
                            }
                        }
                        state.pods = pods;
                        state.pods_per_namespace = per_namespace;
                        state.pods_fetch.finish(None);
                    }
                    Err(err) => {
                        tracing::debug!(%cluster, err = %err.message, "metrics-server pods failed");
                        state.pods_fetch.finish(Some(err.message.into()));
                    }
                }
                // Pods whose history is wanted start collecting samples.
                let wanted: Vec<ObjectKey> = this
                    .demand
                    .borrow()
                    .histories
                    .keys()
                    .filter(|(c, _)| *c == cluster)
                    .map(|(_, key)| key.clone())
                    .collect();
                if let Some(state) = this.current(&cluster, generation) {
                    for key in wanted {
                        if let Some(usage) = state.pods.get(&key).copied() {
                            state
                                .samples
                                .entry(key)
                                .or_insert_with(|| VecDeque::from([(now(), usage)]));
                        }
                    }
                }
                Metrics::changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn finish_nodes(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        task: Task<Result<HashMap<String, Usage>, PromError>>,
        cx: &mut Context<Self>,
    ) {
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.current(&cluster, generation) else {
                    return;
                };
                match result {
                    Ok(nodes) => {
                        let total = nodes.values().fold(Usage::default(), |a, b| Usage {
                            cpu: a.cpu + b.cpu,
                            memory: a.memory + b.memory,
                        });
                        state.totals.push_back((now(), total));
                        while state.totals.len() > MAX_SAMPLES {
                            state.totals.pop_front();
                        }
                        state.nodes = nodes;
                        state.nodes_fetch.finish(None);
                        state.failures = 0;
                    }
                    Err(err) => {
                        tracing::debug!(%cluster, %err, "node usage failed");
                        state.nodes_fetch.finish(Some(err.to_string().into()));
                        this.failed(&cluster, &err);
                    }
                }
                Metrics::changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_history(&mut self, cluster: &ClusterId, key: ObjectKey, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(prom) = state.prom.clone() else {
            return;
        };
        let entry = state.histories.entry(key.clone()).or_default();
        if !entry.fetch.due(HISTORY_REFRESH) {
            return;
        }
        entry.fetch.start();
        let Some((namespace, name)) = key.split_once('/') else {
            entry.fetch.finish(Some("not a pod".into()));
            return;
        };
        let filters = [("namespace", namespace), ("pod", name)];
        let (Some(cpu), Some(memory)) = (
            state.queries.render("cluster_cpu", &filters),
            state.queries.render("cluster_memory", &filters),
        ) else {
            return;
        };
        let generation = state.generation;
        let end = (now() / HISTORY_STEP).floor() * HISTORY_STEP;
        let start = end - HISTORY_WINDOW.as_secs_f64();
        let task = spawn_kube(cx, async move {
            let cpu = prom.query_range(&cpu, start, end, HISTORY_STEP).await?;
            let memory = prom.query_range(&memory, start, end, HISTORY_STEP).await?;
            Ok::<_, PromError>((cpu, memory))
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.current(&cluster, generation) else {
                    return;
                };
                let Some(entry) = state.histories.get_mut(&key) else {
                    return;
                };
                match result {
                    Ok((cpu, memory)) => {
                        // Gaps (pod not running yet) are skipped: sparklines show what exists.
                        let values = |series: &[RangeSeries]| -> Vec<f64> {
                            series
                                .first()
                                .map(|s| {
                                    align(&s.values, start, end, HISTORY_STEP)
                                        .into_iter()
                                        .flatten()
                                        .collect()
                                })
                                .unwrap_or_default()
                        };
                        entry.data = Some(UsageHistory {
                            cpu: values(&cpu),
                            memory: values(&memory),
                            source: "Prometheus".into(),
                            window: "last 1h".into(),
                        });
                        entry.fetch.finish(None);
                        state.failures = 0;
                    }
                    Err(err) => entry.fetch.finish(Some(err.to_string().into())),
                }
                Metrics::changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_range(&mut self, cluster: &ClusterId, key: RangeKey, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(prom) = state.prom.clone() else {
            return;
        };
        let entry = state.ranges.entry(key.clone()).or_default();
        if !entry.fetch.due(key.range.refresh()) {
            return;
        }
        let filters: Vec<(&str, &str)> = key
            .filters
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let Some(promql) = state.queries.render(key.query, &filters) else {
            entry
                .fetch
                .finish(Some(format!("unknown query {}", key.query).into()));
            return;
        };
        entry.fetch.start();
        let generation = state.generation;
        let (start, end, step) = key.range.window(now());
        let task = spawn_kube(cx, async move {
            prom.query_range(&promql, start, end, step).await
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.current(&cluster, generation) else {
                    return;
                };
                let Some(entry) = state.ranges.get_mut(&key) else {
                    return;
                };
                match result {
                    Ok(series) => {
                        entry.result = Some(RangeResult {
                            series,
                            start,
                            end,
                            step,
                        });
                        entry.fetch.finish(None);
                        state.failures = 0;
                    }
                    Err(err) => {
                        tracing::debug!(%cluster, query = key.query, %err, "range query failed");
                        entry.fetch.finish(Some(err.to_string().into()));
                        // A heavy query (7d) timing out says nothing about the target.
                        if !matches!(err, PromError::Timeout) {
                            this.failed(&cluster, &err);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current(&mut self, cluster: &ClusterId, generation: u64) -> Option<&mut ClusterMetrics> {
        self.clusters
            .get_mut(cluster)
            .filter(|s| s.generation == generation)
    }

    /// Counts target failures; after [`MAX_FAILURES`] Prometheus is searched again.
    fn failed(&mut self, cluster: &ClusterId, err: &PromError) {
        if !err.is_target_error() {
            return;
        }
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if !matches!(state.source, Source::Prometheus { .. }) {
            return;
        }
        state.failures += 1;
        if state.failures >= MAX_FAILURES {
            tracing::info!(%cluster, %err, "Prometheus keeps failing; looking again");
            self.generation += 1;
            self.clusters
                .insert(cluster.clone(), ClusterMetrics::new(self.generation));
        }
    }
}

/// `5m`, `1h 20m`.
fn human_span(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    match minutes {
        0 => format!("{}s", seconds.round() as u64),
        m if m < 60 => format!("{m}m"),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

/// Pod usage from metrics-server, and whether listing across namespaces is forbidden.
type ServerPods = (HashMap<ObjectKey, Usage>, bool);

/// Which pods' usage views want.
#[derive(Clone, Debug, PartialEq)]
enum PodScope {
    /// Every namespace; the ones views named are kept for fetching per namespace when listing
    /// across namespaces is forbidden.
    All(Vec<String>),
    Namespaces(Vec<String>),
}

impl PodScope {
    /// Whether a fetch of `self` has the pods `wanted` asks for. `per_namespace`: listing across
    /// namespaces is forbidden, so only the named namespaces were fetched.
    fn covers(&self, wanted: &PodScope, per_namespace: bool) -> bool {
        match (self, wanted) {
            (PodScope::All(_), _) if !per_namespace => true,
            (PodScope::Namespaces(_), PodScope::All(_)) if !per_namespace => false,
            (
                PodScope::All(fetched) | PodScope::Namespaces(fetched),
                PodScope::All(wanted) | PodScope::Namespaces(wanted),
            ) => wanted.iter().all(|ns| fetched.contains(ns)),
        }
    }
}

/// The result of looking for a source.
enum Detected {
    Prometheus(Box<PromClient>, HashSet<String>),
    MetricsServer(String),
    None(String),
}

/// What detection may authenticate with. Never logged.
struct Credentials {
    /// Keychain entry of the Authorization header for an external URL.
    keychain_key: String,
    /// The user's own bearer token, for Services behind an auth proxy (OpenShift).
    user_token: Option<kubyl_kube::auth::BearerToken>,
}

async fn detect(
    client: kube::Client,
    settings: MetricsSettings,
    override_: PrometheusOverride,
    auth: Credentials,
    metrics_server: bool,
) -> Detected {
    if settings.source == SourcePreference::Off {
        return Detected::None("Metrics are off in settings (metrics.source).".into());
    }
    let want_prometheus = settings.source != SourcePreference::MetricsServer && !override_.disabled;
    let mut why = String::from("No Prometheus found.");
    if want_prometheus {
        match find_prometheus(&client, &settings, &override_, auth).await {
            Ok(prom) => {
                // Which recording rules and optional exporters (node-exporter, PSI, volume
                // stats…) exist decides which queries and panels are used.
                let rules: HashSet<String> = prom
                    .metric_names()
                    .await
                    .map(|names| names.into_iter().collect())
                    .unwrap_or_default();
                return Detected::Prometheus(Box::new(prom), rules);
            }
            Err(reason) => why = reason,
        }
    } else if override_.disabled {
        why = "Prometheus is disabled for this cluster in settings.".into();
    }
    if metrics_server && settings.source != SourcePreference::Prometheus {
        Detected::MetricsServer(why)
    } else if settings.source == SourcePreference::Prometheus {
        Detected::None(why)
    } else {
        Detected::None(format!("{why} metrics-server isn't installed either."))
    }
}

async fn find_prometheus(
    client: &kube::Client,
    settings: &MetricsSettings,
    override_: &PrometheusOverride,
    auth: Credentials,
) -> Result<PromClient, String> {
    let service_account = override_
        .service_account
        .as_deref()
        .and_then(|sa| sa.split_once('/'))
        .map(|(ns, name)| (ns.to_string(), name.to_string()));
    // Behind an auth proxy (OpenShift): the service proxy strips credentials, so call the
    // Service's Route with a token instead.
    let through_route = |target: Target, err: PromError| {
        let user = auth.user_token.clone();
        let service_account = service_account.clone();
        async move {
            if !matches!(err, PromError::Http(401 | 403, _)) {
                return Err(err.to_string());
            }
            openshift::through_route(client, &target, user, service_account)
                .await
                .map_err(|route| format!("{err} through the API server ({route})"))
        }
    };
    if let Some(url) = &override_.url {
        let auth_key = auth.keychain_key.clone();
        let header = tokio::task::spawn_blocking(move || kubyl_kube::auth::store::get(&auth_key))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        let prom = PromClient::external(url, override_.insecure_skip_tls_verify, header.as_ref())
            .map_err(|e| format!("Prometheus at {url}: {e}"))?;
        return match prom.probe(Duration::from_secs(8)).await {
            Ok(()) => Ok(prom),
            Err(err) => Err(format!("Prometheus at {}: {err}", prom.target().label())),
        };
    }
    if let Some(service) = &override_.service {
        let namespace = override_
            .namespace
            .clone()
            .unwrap_or_else(|| "monitoring".into());
        let port = match &override_.port {
            Some(port) => port.clone(),
            None => service_port(client, &namespace, service)
                .await
                .unwrap_or_else(|| "9090".into()),
        };
        let target = Target::Service {
            namespace,
            service: service.clone(),
            port,
            scheme: override_.scheme.clone().unwrap_or_else(|| "http".into()),
            path: override_.path.clone().unwrap_or_default(),
        };
        return match discover::probe(client, vec![target.clone()]).await {
            Found::Prometheus(prom) => Ok(prom),
            Found::Nothing {
                best: Some((target, err)),
            } => through_route(target.clone(), err)
                .await
                .map_err(|why| format!("Prometheus {} (from settings): {why}", target.label())),
            Found::Nothing { best: None } => Err(format!(
                "Prometheus {} (from settings) didn't answer.",
                target.label()
            )),
        };
    }
    if !settings.discover {
        return Err("Prometheus discovery is off (metrics.discover).".into());
    }
    match discover::discover(client).await {
        Found::Prometheus(prom) => Ok(prom),
        Found::Nothing {
            best: Some((target, err)),
        } => through_route(target.clone(), err)
            .await
            .map_err(|why| format!("Found {} but it didn't answer: {why}.", target.label())),
        Found::Nothing { best: None } => Err("No Prometheus found.".into()),
    }
}

/// The API port of a Service named in settings without a port.
async fn service_port(client: &kube::Client, namespace: &str, service: &str) -> Option<String> {
    let request = http::Request::get(format!("/api/v1/namespaces/{namespace}/services/{service}"))
        .body(Vec::new())
        .ok()?;
    let object: serde_json::Value = client.request(request).await.ok()?;
    let found = discover::candidates(std::slice::from_ref(&object));
    match found.first().map(|c| &c.target) {
        Some(Target::Service { port, .. }) => Some(port.clone()),
        _ => object["spec"]["ports"][0]["port"]
            .as_i64()
            .map(|p| p.to_string()),
    }
}

/// Current CPU and memory from two instant queries, keyed by `key`.
async fn prom_usage<K: std::hash::Hash + Eq>(
    prom: &PromClient,
    queries: &Queries,
    cpu: &str,
    memory: &str,
    filters: &[(&str, &str)],
    key: impl Fn(&Sample) -> Option<K>,
) -> Result<HashMap<K, Usage>, PromError> {
    let (Some(cpu), Some(memory)) = (
        queries.render(cpu, filters),
        queries.render(memory, filters),
    ) else {
        return Err(PromError::Query("unknown query".into()));
    };
    let (cpu, memory) = futures::join!(prom.query(&cpu), prom.query(&memory));
    let mut out: HashMap<K, Usage> = HashMap::new();
    for sample in cpu? {
        if let Some(k) = key(&sample) {
            out.entry(k).or_default().cpu = sample.value;
        }
    }
    for sample in memory? {
        if let Some(k) = key(&sample) {
            out.entry(k).or_default().memory = sample.value;
        }
    }
    Ok(out)
}

/// Pod usage from metrics-server for `scope`: across namespaces in one request, or per
/// namespace (asked for, or when listing across namespaces is forbidden). Returns whether that
/// is forbidden, so later fetches go per namespace right away.
async fn server_pods(
    client: &kube::Client,
    forbidden_before: bool,
    scope: PodScope,
) -> Result<ServerPods, metrics_server::FetchError> {
    // A few namespaces: one small request each. More: one request across namespaces is cheaper.
    let scope = match scope {
        PodScope::Namespaces(namespaces) if namespaces.len() > 5 => PodScope::All(namespaces),
        scope => scope,
    };
    let (namespaces, forbidden) = match scope {
        PodScope::Namespaces(namespaces) => (namespaces, forbidden_before),
        PodScope::All(namespaces) if forbidden_before => (namespaces, true),
        PodScope::All(namespaces) => match metrics_server::pods(client, None).await {
            Ok(pods) => return Ok((pods, false)),
            Err(err) if err.forbidden && !namespaces.is_empty() => (namespaces, true),
            Err(err) => return Err(err),
        },
    };
    let mut out = HashMap::new();
    let mut last_error = None;
    for ns in &namespaces {
        match metrics_server::pods(client, Some(ns)).await {
            Ok(pods) => out.extend(pods),
            Err(err) => last_error = Some(err),
        }
    }
    match last_error {
        Some(err) if out.is_empty() => Err(err),
        _ => Ok((out, forbidden)),
    }
}

/// Settings overrides of queries, for tests and the overview's subtitles.
pub fn render_query(cx: &App, cluster: &ClusterId, id: &str) -> Option<String> {
    let service = MetricsService::global(cx)?;
    let service = service.read(cx);
    let queries = service
        .clusters
        .get(cluster)
        .map(|c| c.queries.clone())
        .unwrap_or_else(|| Queries::new(BTreeMap::new(), HashSet::new()));
    queries.render(id, &[])
}

/// Stores (or clears, with `None`) the Authorization header for a cluster's external
/// Prometheus in the OS keychain, off the UI thread.
pub fn store_auth_header(
    cluster: ClusterId,
    header: Option<SecretString>,
    cx: &mut App,
) -> Task<Result<(), String>> {
    let key = auth_key(&cluster);
    cx.background_executor().spawn(async move {
        match header {
            Some(header) => kubyl_kube::auth::store::set(&key, &header),
            None => kubyl_kube::auth::store::delete(&key),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_scopes_cover() {
        let ns = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let all = PodScope::All(ns(&["a"]));
        let a = PodScope::Namespaces(ns(&["a"]));
        let ab = PodScope::Namespaces(ns(&["a", "b"]));
        assert!(all.covers(&ab, false));
        assert!(ab.covers(&a, false));
        assert!(!a.covers(&ab, false), "b wasn't fetched");
        assert!(!a.covers(&all, false));
        // Listing across namespaces is forbidden: only the named ones were fetched.
        assert!(!all.covers(&ab, true));
        assert!(ab.covers(&PodScope::All(ns(&["b"])), true));
    }

    #[test]
    fn spans() {
        assert_eq!(human_span(20.0), "20s");
        assert_eq!(human_span(300.0), "5m");
        assert_eq!(human_span(3600.0), "1h");
        assert_eq!(human_span(4800.0), "1h 20m");
    }

    #[test]
    fn range_results_align() {
        let result = RangeResult {
            series: vec![
                RangeSeries {
                    labels: Default::default(),
                    values: vec![(0.0, 1.0), (10.0, 2.0)],
                },
                RangeSeries {
                    labels: Default::default(),
                    values: vec![(10.0, 3.0), (20.0, 1.0)],
                },
            ],
            start: 0.0,
            end: 20.0,
            step: 10.0,
        };
        assert_eq!(result.times(), vec![0.0, 10.0, 20.0]);
        assert_eq!(result.total(), vec![Some(1.0), Some(5.0), Some(1.0)]);
    }

    #[test]
    fn demand_expires() {
        let mut demand = Demand::default();
        let cluster = ClusterId::new("a");
        demand.nodes.insert(cluster.clone(), Instant::now());
        demand
            .source
            .insert(ClusterId::new("old"), Instant::now() - DEMAND_TTL * 2);
        assert_eq!(demand.clusters(), HashSet::from([cluster]));
        demand
            .nodes
            .insert(ClusterId::new("gone"), Instant::now() - KEEP * 2);
        demand.prune();
        assert_eq!(demand.nodes.len(), 1);
        assert_eq!(demand.source.len(), 1);
    }

    #[test]
    fn sources() {
        assert!(
            Source::Prometheus {
                target: Target::service("m", "p", "9090")
            }
            .has_history()
        );
        let ms = Source::MetricsServer { note: "".into() };
        assert!(ms.has_usage() && !ms.has_history());
        assert!(!Source::Detecting.has_usage());
    }
}
