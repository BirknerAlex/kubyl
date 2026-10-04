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
use kubyl_metrics::basic_auth::{self, SavedBasic};
use kubyl_metrics::prometheus::{PromClient, PromError, basic_authorization};
use kubyl_portforward::manager::{
    Ephemeral, ForwardId, ForwardSpec, ForwardState, PortForwardManager,
};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};
use kubyl_settings::Settings;
use secrecy::SecretString;
use serde_json::Value;

pub use kubyl_alerts_core::cache::*;

use crate::client::{self, AmConn, Credentials, Discovered, Probed, Tried};
use crate::discover::AmTarget;
use crate::matchers::{self, Matcher};
use crate::merge::Heartbeat;
use crate::model::{self, Alert, AlertState, RuleGroup, Severity, Silence};
use crate::settings::{AlertsSettings, ClusterSettings, NotifyClusters};

/// Keychain entries of the username and password (a `Basic …` header with the Service's UID)
/// of an in-cluster Alertmanager behind basic auth: `alerts-password:<cluster id>/<ns>/<svc>`,
/// the entry's id first, then its members' (see [`basic_auth::keys`]).
fn password_keys(cluster: &ClusterId, label: &str, cx: &App) -> Vec<String> {
    let ids = ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).settings_keys(cluster))
        .unwrap_or_else(|| vec![cluster.to_string()]);
    basic_auth::keys("alerts-password", &ids, label)
}

/// How signing in to a locked Alertmanager ended.
enum Unlock {
    NoCredentials,
    /// The Service was re-created since the credentials were saved.
    Replaced,
    Rejected,
    Failed(String),
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
    forwards: Vec<ForwardId>,
    /// The forwards of signed-in Alertmanagers (also in `forwards`), by label.
    password_forwards: HashMap<String, ForwardId>,
    prom: Option<PromClient>,
    prometheus_alertmanagers: Option<usize>,
    nodes: Arc<NodeMap>,
    refetch_at: Option<Instant>,
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
            // Results of tasks started for the dropped state must not apply to a reconnect.
            self.generation += 1;
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
                // Discovery and reads in flight report under the old id and are dropped: start
                // the new id over (new generation), or `discovering`/`in_flight` stay set.
                if let Some(old) = self.clusters.remove(from) {
                    stop_forwards(old.forwards, cx);
                    self.reset(to, cx);
                }
                if let Some(t) = self.demand.borrow_mut().remove(from) {
                    self.demand.borrow_mut().insert(to.clone(), t);
                }
                if let Some(n) = self.notices.remove(from) {
                    self.notices.insert(to.clone(), n);
                }
                cx.notify();
                self.tick(cx);
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
            if state.phase == Phase::Ready && state.fetch_wanted(every) {
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
        stop_forwards(previous, cx);
        tracing::info!(
            cluster = %cluster,
            alertmanagers = state.sources.len(),
            rules = state.prom.is_some(),
            "alert sources"
        );
        crate::changed(cx);
        cx.notify();
        // Signs in with what the keychain has, if anything.
        for label in locked {
            self.unlock(cluster, &label, None, cx);
        }
        self.tick(cx);
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
        cx: &mut Context<Self>,
    ) {
        let header = basic_authorization(username, password);
        self.unlock(cluster, label, Some(header), cx);
    }

    /// Signs in again with the credentials in the keychain.
    pub fn reconnect(&mut self, cluster: &ClusterId, label: &str, cx: &mut Context<Self>) {
        self.unlock(cluster, label, None, cx);
    }

    /// Connects to a locked Alertmanager through a loopback forward with `header` (`None`: the
    /// one in the keychain, if the Service is still the one it was given to). A header the user
    /// typed is kept once it works; saved ones the server refused are deleted.
    fn unlock(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        header: Option<SecretString>,
        cx: &mut Context<Self>,
    ) {
        let client = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster));
        let keys = password_keys(cluster, label, cx);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let generation = state.generation;
        let Some(locked) = state.locked.iter_mut().find(|l| l.label() == label) else {
            return;
        };
        if locked.busy {
            return;
        }
        let Some(client) = client else {
            locked.problem = Some("The cluster isn't connected.".into());
            cx.notify();
            return;
        };
        let was_saved = locked.saved;
        locked.busy = true;
        let target = locked.target.clone();
        cx.notify();
        let typed = header.is_some();
        let cluster = cluster.clone();
        let label = label.to_string();
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
                let connected = password_forward(&cluster, &target, &header, cx).await?;
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
                        && let Err(err) = write_secret(cx, key.clone(), saved.to_secret()).await
                    {
                        tracing::warn!("Alertmanager credentials not stored: {err}");
                    }
                }
                Err(Unlock::Rejected) if !typed => cx.update(|cx| delete_secrets(keys, cx)),
                Err(Unlock::Replaced) => cx.update(|cx| delete_secrets(keys, cx)),
                _ => {}
            }
            let result = result.map(|(connected, _, _)| connected);
            this.update(cx, |this, cx| {
                let current = this
                    .clusters
                    .get(&cluster)
                    .is_some_and(|s| s.generation == generation);
                if !current {
                    if let Ok((_, forward)) = result {
                        stop_forwards(vec![forward], cx);
                    }
                    return;
                }
                this.unlocked(&cluster, &label, typed, was_saved, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn unlocked(
        &mut self,
        cluster: &ClusterId,
        label: &str,
        typed: bool,
        was_saved: bool,
        result: Result<(AmConn, ForwardId), Unlock>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(index) = state.locked.iter().position(|l| l.label() == label) else {
            if let Ok((_, forward)) = result {
                stop_forwards(vec![forward], cx);
            }
            return;
        };
        match result {
            Ok((conn, forward)) => {
                state.locked.remove(index);
                state.forwards.push(forward);
                state.password_forwards.insert(label.to_string(), forward);
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
                crate::changed(cx);
                cx.notify();
                self.tick(cx);
            }
            Err(err) => {
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
                cx.notify();
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
        cx: &mut Context<Self>,
    ) {
        for (label, port) in rejected {
            let keys = password_keys(cluster, label, cx);
            let Some(state) = self.clusters.get_mut(cluster) else {
                return;
            };
            let Some(index) = state.sources.iter().position(|s| {
                &s.label() == label && s.conn.via == client::Via::Password { local_port: *port }
            }) else {
                continue;
            };
            let source = state.sources.remove(index);
            if let Some(forward) = state.password_forwards.remove(label) {
                state.forwards.retain(|f| *f != forward);
                stop_forwards(vec![forward], cx);
            }
            delete_secrets(keys, cx);
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
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(forward) = state.password_forwards.remove(label) else {
            return;
        };
        state.forwards.retain(|f| *f != forward);
        if let Some(index) = state.sources.iter().position(|s| s.label() == label) {
            let source = state.sources.remove(index);
            let mut locked = Locked::new(source.conn.target);
            locked.saved = true;
            locked.problem = Some("The port-forward was stopped.".into());
            state.locked.push(locked);
        }
        state.revision += 1;
        crate::changed(cx);
        cx.notify();
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
        if !output.rejected.is_empty() {
            self.rejected(cluster, &output.rejected, cx);
            // The sources changed under this read: the next one starts over.
            crate::changed(cx);
            cx.notify();
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
            crate::changed(cx);
            cx.notify();
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
        crate::changed(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn lock_for_test(&mut self, cluster: &ClusterId, target: AmTarget) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.locked.push(Locked::new(target));
            state.revision += 1;
        }
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
                // Saved views don't make their cluster a favorite.
                .filter(|f| f.view.is_none())
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

/// Opens a temporary loopback forward to `target` (a Service) and waits until it listens.
/// `on_stop` runs when the user stops it in Active Sessions.
async fn open_forward(
    cluster: &ClusterId,
    target: &AmTarget,
    on_stop: Arc<dyn Fn(&mut App)>,
    cx: &mut AsyncApp,
) -> Result<(kube::Client, ForwardId, u16), String> {
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
            on_stop,
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
    Ok((client, id, local_port))
}

/// Opens a temporary loopback forward to `target` and connects through it.
async fn forward(
    cluster: &ClusterId,
    target: &AmTarget,
    credentials: &Credentials,
    cx: &mut AsyncApp,
) -> Result<(AmConn, ForwardId), String> {
    let on_stop: Arc<dyn Fn(&mut App)> = Arc::new({
        let cluster = cluster.clone();
        move |cx| {
            if let Some(service) = AlertsService::global(cx) {
                let cluster = cluster.clone();
                service.update(cx, |this, cx| this.redetect(&cluster, cx));
            }
        }
    });
    let (client, id, local_port) = open_forward(cluster, target, on_stop, cx).await?;
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
        Probed::NeedsPassword(_) => {
            cx.update(|cx| stop_forwards(vec![id], cx));
            Err("asks for a username and password".into())
        }
    }
}

/// Opens a temporary loopback forward to an Alertmanager behind basic auth and connects with
/// `header`.
async fn password_forward(
    cluster: &ClusterId,
    target: &AmTarget,
    header: &SecretString,
    cx: &mut AsyncApp,
) -> Result<(AmConn, ForwardId), Unlock> {
    let on_stop: Arc<dyn Fn(&mut App)> = Arc::new({
        let (cluster, label) = (cluster.clone(), target.label());
        move |cx| {
            if let Some(service) = AlertsService::global(cx) {
                service.update(cx, |this, cx| {
                    this.password_forward_stopped(&cluster, &label, cx)
                });
            }
        }
    });
    let (_, id, local_port) = open_forward(cluster, target, on_stop, cx)
        .await
        .map_err(Unlock::Failed)?;
    let (target, header) = (target.clone(), header.clone());
    let probed = cx
        .update(|cx| {
            spawn_kube(cx, async move {
                client::probe_password(&target, local_port, &header).await
            })
        })
        .await;
    match probed {
        Ok(conn) => Ok((conn, id)),
        Err(err) => {
            cx.update(|cx| stop_forwards(vec![id], cx));
            Err(match err {
                PromError::Http(401, _) => Unlock::Rejected,
                err => Unlock::Failed(err.to_string()),
            })
        }
    }
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

/// The UID of the Service behind `target`.
async fn service_uid(
    client: &kube::Client,
    target: &AmTarget,
    cx: &mut AsyncApp,
) -> Result<String, Unlock> {
    let AmTarget::Service {
        namespace, service, ..
    } = target
    else {
        return Err(Unlock::Failed("not a Service".into()));
    };
    let (client, namespace, service) = (client.clone(), namespace.clone(), service.clone());
    let task = cx.update(|cx| {
        spawn_kube(cx, async move {
            basic_auth::service_uid(&client, &namespace, &service).await
        })
    });
    task.await.map_err(Unlock::Failed)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge;
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
            rejected: Vec::new(),
            sources: Vec::new(),
            silences: Vec::new(),
            silences_ok: true,
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
    fn results_from_before_a_drop_do_not_apply_after_reconnect(cx: &mut TestAppContext) {
        let (_dir, service) = setup(cx, serde_json::json!({}));
        let cluster = ClusterId::new("c");
        service.update(cx, |s, cx| {
            s.insert_for_test(&cluster, Vec::new(), cx);
            let stale = s.clusters[&cluster].generation;
            s.drop_cluster(&cluster, cx);
            s.insert_for_test(&cluster, Vec::new(), cx);
            assert_ne!(s.clusters[&cluster].generation, stale);
            s.fetched(
                &cluster,
                stale,
                output(vec![firing("Stale", Severity::Critical)]),
                cx,
            );
            assert!(s.clusters[&cluster].alerts.is_empty());
        });
    }

    #[test]
    fn a_pending_refetch_waits_for_the_read_in_flight() {
        let mut state = ClusterAlerts::new(0);
        state.refetch_at = Some(Instant::now());
        assert!(state.fetch_wanted(Duration::from_secs(60)));
        state.fetch.in_flight = true;
        assert!(!state.fetch_wanted(Duration::from_secs(60)));
        state.fetch.in_flight = false;
        state.fetch.last = Some(Instant::now());
        assert!(
            state.fetch_wanted(Duration::from_secs(60)),
            "refetch is due"
        );
        state.refetch_at = None;
        assert!(!state.fetch_wanted(Duration::from_secs(60)));
    }

    #[gpui::test]
    fn a_failed_silences_read_keeps_the_previous_list(cx: &mut TestAppContext) {
        let (_dir, service) = setup(cx, serde_json::json!({}));
        let cluster = ClusterId::new("c");
        service.update(cx, |s, cx| {
            s.insert_for_test(&cluster, Vec::new(), cx);
            let generation = s.clusters[&cluster].generation;
            let silence = Silence {
                id: "s1".into(),
                matchers: Vec::new(),
                starts_at: None,
                ends_at: None,
                created_by: String::new(),
                comment: String::new(),
                state: "active".into(),
                source: "am".into(),
            };
            s.clusters.get_mut(&cluster).unwrap().silences = Arc::new(vec![silence]);
            let mut failed = output(Vec::new());
            failed.silences_ok = false;
            failed.error = Some("silences: denied".into());
            s.fetched(&cluster, generation, failed, cx);
            let state = &s.clusters[&cluster];
            assert_eq!(state.silences.len(), 1);
            assert!(state.error.is_some());
        });
    }

    #[test]
    fn an_alertmanager_outage_keeps_the_previous_state() {
        assert!(!can_merge(2, false, true), "Prometheus alone is not enough");
        assert!(can_merge(2, true, false));
        assert!(can_merge(0, false, true));
        assert!(!can_merge(0, false, false));
    }

    #[gpui::test]
    fn a_rekeyed_cluster_starts_over(cx: &mut TestAppContext) {
        let (_dir, service) = setup(cx, serde_json::json!({}));
        let from = ClusterId::new("old");
        let to = ClusterId::new("new");
        service.update(cx, |s, cx| {
            s.insert_for_test(&from, vec![firing("A", Severity::Critical)], cx);
            let state = s.clusters.get_mut(&from).unwrap();
            state.discovering = true;
            state.fetch.in_flight = true;
            let old_generation = state.generation;
            s.connection_event(
                &ConnectionEvent::Rekeyed {
                    from: from.clone(),
                    to: to.clone(),
                },
                cx,
            );
            assert!(!s.clusters.contains_key(&from));
            let state = &s.clusters[&to];
            // Nothing stays stuck waiting for a task that reports under the old id.
            assert!(!state.discovering && !state.fetch.in_flight);
            assert_eq!(state.phase, Phase::Unknown);
            assert!(state.generation > old_generation);
        });
    }

    #[gpui::test]
    fn a_failed_sign_in_says_why_and_keeps_it_locked(cx: &mut TestAppContext) {
        let (_dir, service) = setup(cx, serde_json::json!({}));
        let cluster = ClusterId::new("c");
        let target = AmTarget::service("prometheus", "alertmanager", "9093", "http", "");
        service.update(cx, |s, cx| {
            s.insert_for_test(&cluster, Vec::new(), cx);
            let state = s.clusters.get_mut(&cluster).unwrap();
            state.locked.push(Locked::new(target.clone()));
            state.locked[0].busy = true;
            let label = target.label();
            s.unlocked(&cluster, &label, true, false, Err(Unlock::Rejected), cx);
            let locked = &s.clusters[&cluster].locked[0];
            assert!(!locked.busy && !locked.saved);
            assert!(locked.problem.as_ref().unwrap().contains("rejected this"));
            // Saved credentials that couldn't be tried stay saved: "Reconnect" may work.
            s.unlocked(
                &cluster,
                &label,
                false,
                false,
                Err(Unlock::Failed("port-forward: no pods".into())),
                cx,
            );
            let locked = &s.clusters[&cluster].locked[0];
            assert!(locked.saved);
            assert_eq!(locked.problem.as_deref(), Some("port-forward: no pods"));
            // A mistyped password doesn't hide saved credentials.
            s.unlocked(&cluster, &label, true, true, Err(Unlock::Rejected), cx);
            assert!(s.clusters[&cluster].locked[0].saved);
            s.unlocked(&cluster, &label, false, true, Err(Unlock::Replaced), cx);
            let locked = &s.clusters[&cluster].locked[0];
            assert!(!locked.saved);
            assert!(locked.problem.as_ref().unwrap().contains("re-created"));
            s.unlocked(
                &cluster,
                &label,
                false,
                false,
                Err(Unlock::NoCredentials),
                cx,
            );
            let locked = &s.clusters[&cluster].locked[0];
            assert!(!locked.saved && locked.problem.is_none());
            assert_eq!(
                password_keys(&cluster, &label, cx),
                ["alerts-password:c/prometheus/alertmanager"]
            );
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
