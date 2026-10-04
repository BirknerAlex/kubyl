//! [`FlowService`]: the app-wide, demand-driven flow state (README "Flow buffer and
//! streaming"). Per cluster it detects the backends, picks one (settings may override), checks
//! the permissions it needs, connects (a temporary loopback forward for Hubble Relay, the
//! service proxy for the others) and runs one stream per (cluster, server-side filter) while a
//! view holds a [`FlowLease`]. Streams run on Tokio and are drained on the UI thread at most 60
//! times a second into ring buffers. When the last lease goes, the streams stop after a short
//! grace and the forward with them. Nothing is written anywhere.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{App, AppContext as _, AsyncApp, Context, Entity, Global, Task, WeakEntity};
use jiff::Timestamp;
use kubyl_core::{ClusterId, Gvr, ResourceRef, spawn_kube};
use kubyl_kube::ConnectionManager;
use kubyl_kube::access::{self, AccessQuery};
use kubyl_portforward::manager::{
    Ephemeral, ForwardId, ForwardSpec, ForwardState, PortForwardManager,
};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};

pub use kubyl_netflow_core::state::*;

use crate::aggregate::{Topology, Zoom};
use crate::backends::hubble::{Hubble, HubbleTarget};
use crate::backends::netobserv::{LokiAccess, NetObserv};
use crate::backends::whisker::Whisker;
use crate::buffer::FlowBuffer;
use crate::detect::{self, Candidate, Detection, Inputs, LokiTarget, PromTarget, Served};
use crate::filter::FlowFilter;
use crate::provider::{
    BackendKind, BackendStatus, FlowProvider, ProviderError, StreamEvent, StreamQuery,
};
use crate::settings::{BackendSetting, ClusterNetflowSettings, NetflowSettings};

/// Detection's progress.
#[derive(Clone, Debug)]
enum Detect {
    /// Not connected or discovery unknown yet.
    Waiting,
    Running,
    Done(Arc<Detection>),
}

/// The chosen backend's state.
#[derive(Clone)]
enum Backend {
    None,
    /// Settings turned flows off for this cluster.
    Off,
    Connecting(BackendKind),
    Ready {
        kind: BackendKind,
        provider: Arc<dyn FlowProvider>,
        status: BackendStatus,
    },
    Failed(BackendKind, ProviderError),
}

/// One stream and its buffer.
pub struct FlowStream {
    pub filter: FlowFilter,
    pub buffer: FlowBuffer,
    /// Bumped on every change of the buffer.
    pub revision: u64,
    pub status: StreamStatus,
    /// The backend's history has arrived.
    pub caught_up: bool,
    window: Duration,
    lease: Rc<()>,
    idle_since: Option<Instant>,
    arrivals: VecDeque<(Instant, usize)>,
    failures: u32,
    generation: u64,
    tasks: Vec<Task<()>>,
}

impl FlowStream {
    /// Flows per second over the last 10 s.
    pub fn rate(&self) -> f64 {
        let now = Instant::now();
        let recent: usize = self
            .arrivals
            .iter()
            .filter(|(at, _)| now.duration_since(*at) <= Duration::from_secs(10))
            .map(|(_, n)| n)
            .sum();
        recent as f64 / 10.0
    }
}

/// Keeps a cluster's stream (and its backend) going while held.
#[derive(Clone)]
pub struct FlowLease {
    _cluster: Rc<()>,
    _stream: Rc<()>,
}

/// The metrics topology of one (zoom, window, filter).
#[derive(Clone, Debug)]
pub struct MetricsGraph {
    pub topology: Option<Arc<Topology>>,
    pub error: Option<ProviderError>,
    pub loading: bool,
    fetched: Option<Instant>,
    wanted: Instant,
}

struct ClusterFlows {
    detect: Detect,
    /// The inputs detection ran with (a change runs it again).
    detected_with: Option<(Served, ClusterNetflowSettings, bool)>,
    backend: Backend,
    forward: Option<ForwardId>,
    streams: HashMap<String, FlowStream>,
    graphs: HashMap<String, MetricsGraph>,
    lease: Rc<()>,
    idle_since: Option<Instant>,
    status_read: Option<Instant>,
    /// Bumped when the backend is rebuilt; older work is dropped.
    generation: u64,
    tasks: Vec<Task<()>>,
}

impl ClusterFlows {
    fn new() -> Self {
        Self {
            detect: Detect::Waiting,
            detected_with: None,
            backend: Backend::None,
            forward: None,
            streams: HashMap::new(),
            graphs: HashMap::new(),
            lease: Rc::new(()),
            idle_since: None,
            status_read: None,
            generation: 0,
            tasks: Vec::new(),
        }
    }

    fn provider(&self) -> Option<Arc<dyn FlowProvider>> {
        match &self.backend {
            Backend::Ready { provider, .. } => Some(provider.clone()),
            _ => None,
        }
    }
}

pub struct FlowService {
    clusters: HashMap<ClusterId, ClusterFlows>,
    /// Timers and connections run (off in GPUI tests).
    live: bool,
    _tick: Option<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
    /// GPUI tests have no connected clusters.
    #[cfg(test)]
    assume_connected: bool,
}

struct GlobalFlows(Entity<FlowService>);

impl Global for GlobalFlows {}

impl FlowService {
    /// Installs the global. `live`: timers and connections (off in GPUI tests).
    pub fn install(live: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this: &mut Self, _, event, cx| {
                    this.connection_event(event, cx)
                }));
            }
            if cx.has_global::<kubyl_settings::Settings>() {
                subscriptions.push(cx.observe_global::<kubyl_settings::Settings>(
                    |this: &mut Self, cx| this.settings_changed(cx),
                ));
            }
            let tick = live.then(|| {
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    loop {
                        cx.background_executor().timer(TICK).await;
                        if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                            break;
                        }
                    }
                })
            });
            Self {
                clusters: HashMap::new(),
                live,
                _tick: tick,
                _subscriptions: subscriptions,
                #[cfg(test)]
                assume_connected: false,
            }
        });
        cx.set_global(GlobalFlows(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalFlows>().map(|g| g.0.clone())
    }

    fn settings(cx: &App) -> NetflowSettings {
        if cx.has_global::<kubyl_settings::Settings>() {
            kubyl_settings::Settings::get::<NetflowSettings>(cx).clone()
        } else {
            NetflowSettings::default()
        }
    }

    fn cluster_settings(cluster: &ClusterId, cx: &App) -> ClusterNetflowSettings {
        let keys = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).settings_keys(cluster))
            .unwrap_or_else(|| vec![cluster.to_string()]);
        Self::settings(cx).for_keys(&keys)
    }

    // ----- Leases -----

    /// Keeps `cluster`'s stream with the server-side `filter` going (history from `window`
    /// back) while the lease lives. Starts detection and the backend when needed.
    pub fn lease(
        &mut self,
        cluster: &ClusterId,
        filter: &FlowFilter,
        window: Duration,
        cx: &mut Context<Self>,
    ) -> FlowLease {
        let settings = Self::settings(cx);
        self.ensure(cluster, cx);
        let state = self.clusters.get_mut(cluster).expect("ensured");
        state.idle_since = None;
        let key = filter.canonical();
        let max_age = Duration::from_secs(settings.max_age_minutes.max(1) * 60).max(window);
        let stream = state
            .streams
            .entry(key.clone())
            .or_insert_with(|| FlowStream {
                filter: filter.clone(),
                buffer: FlowBuffer::new(settings.max_flows, max_age),
                revision: 0,
                status: StreamStatus::Starting,
                caught_up: false,
                window,
                lease: Rc::new(()),
                idle_since: None,
                arrivals: VecDeque::new(),
                failures: 0,
                generation: 0,
                tasks: Vec::new(),
            });
        stream.idle_since = None;
        if window > stream.window {
            // A longer window: the buffer keeps more, and a fresh stream fetches the older
            // history (the flows held so far come again with it). The numbering goes on, so
            // what views held just expires.
            stream.window = window;
            stream.buffer.set_limits(settings.max_flows, max_age);
            stream.buffer.clear();
            stream.tasks.clear();
            stream.status = StreamStatus::Starting;
            stream.caught_up = false;
            stream.revision += 1;
        }
        let lease = FlowLease {
            _cluster: state.lease.clone(),
            _stream: stream.lease.clone(),
        };
        let starts = stream.tasks.is_empty() && stream.status == StreamStatus::Starting;
        if starts && matches!(state.backend, Backend::Ready { .. }) {
            self.start_stream(cluster, &key, cx);
        }
        lease
    }

    /// Keeps `cluster` detected and its backend connected while the lease lives (a view before
    /// it knows its stream).
    pub fn watch(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) -> FlowLease {
        self.ensure(cluster, cx);
        let state = self.clusters.get_mut(cluster).expect("ensured");
        state.idle_since = None;
        FlowLease {
            _cluster: state.lease.clone(),
            _stream: Rc::new(()),
        }
    }

    /// Starts tracking a cluster (detection, then the backend).
    fn ensure(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if !self.clusters.contains_key(cluster) {
            self.clusters.insert(cluster.clone(), ClusterFlows::new());
            self.detect(cluster, cx);
        } else if matches!(self.clusters[cluster].backend, Backend::None)
            && matches!(self.clusters[cluster].detect, Detect::Done(_))
        {
            // It went idle: connect again.
            self.choose(cluster, cx);
        }
    }

    // ----- Detection and connection -----

    fn inputs(cluster: &ClusterId, cx: &App) -> Option<(Inputs, bool)> {
        let manager = ConnectionManager::try_global(cx)?;
        let manager = manager.read(cx);
        let client = manager.client(cluster)?;
        let connection = manager.cluster(cluster)?;
        let discovery = connection.discovery.as_ref()?;
        let settings = Self::cluster_settings(cluster, cx);
        let keep = Self::settings(cx).keep_query_values;
        Some((
            Inputs {
                client,
                served: Served::from_discovery(discovery),
                git_version: connection
                    .info
                    .as_ref()
                    .map(|i| i.version.clone())
                    .unwrap_or_default(),
                settings,
            },
            keep,
        ))
    }

    /// Detects again when the inputs changed (or `force`).
    fn detect_with(&mut self, cluster: &ClusterId, force: bool, cx: &mut Context<Self>) {
        let inputs = Self::inputs(cluster, cx);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some((inputs, keep)) = inputs else {
            // Disconnected, or discovery unknown: never "no backend" before it's known.
            if !matches!(state.detect, Detect::Waiting) || state.forward.is_some() {
                self.reset(cluster, cx);
            }
            if let Some(state) = self.clusters.get_mut(cluster) {
                state.detect = Detect::Waiting;
                state.detected_with = None;
            }
            cx.notify();
            return;
        };
        let key = (inputs.served.clone(), inputs.settings.clone(), keep);
        if !force && state.detected_with.as_ref() == Some(&key) {
            return;
        }
        state.detected_with = Some(key);
        self.reset(cluster, cx);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.detect = Detect::Running;
        state.generation += 1;
        let generation = state.generation;
        if !self.live {
            return;
        }
        let work = spawn_kube(cx, detect::detect(inputs));
        let id = cluster.clone();
        let task = cx.spawn(async move |this, cx| {
            let detection = work.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.clusters.get_mut(&id) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.detect = Detect::Done(Arc::new(detection));
                this.choose(&id, cx);
                cx.notify();
            })
            .ok();
        });
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.tasks.retain(|t| !t.is_ready());
            state.tasks.push(task);
        }
        cx.notify();
    }

    fn detect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.detect_with(cluster, false, cx);
    }

    /// Looks for backends again (the view's "Look again").
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if self.clusters.contains_key(cluster) {
            self.detect_with(cluster, true, cx);
        }
    }

    /// Connects again after a failure (the view's "Try again").
    pub fn reconnect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if self.clusters.contains_key(cluster) {
            self.reset(cluster, cx);
            self.choose(cluster, cx);
        }
    }

    /// Stops streams, the forward and the backend (detection stays).
    fn reset(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.generation += 1;
        state.backend = Backend::None;
        state.status_read = None;
        state.graphs.clear();
        for stream in state.streams.values_mut() {
            stream.tasks.clear();
            stream.status = StreamStatus::Starting;
            stream.caught_up = false;
            stream.generation += 1;
        }
        if let Some(forward) = state.forward.take() {
            PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx));
        }
    }

    /// Picks the backend from detection and settings, then connects.
    fn choose(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let settings = Self::cluster_settings(cluster, cx);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Detect::Done(detection) = &state.detect else {
            return;
        };
        let detection = detection.clone();
        let candidate = match settings.backend {
            Some(BackendSetting::Off) => {
                state.backend = Backend::Off;
                cx.notify();
                return;
            }
            Some(setting) => {
                let kind = setting.kind().expect("not off");
                match detection.find(kind) {
                    Some(candidate) => candidate.clone(),
                    None => {
                        state.backend = Backend::Failed(
                            kind,
                            ProviderError::NotFound(format!(
                                "{} (picked in settings, netflow.clusters.<cluster>.backend)",
                                kind.label()
                            )),
                        );
                        cx.notify();
                        return;
                    }
                }
            }
            None => match detection.candidates.first() {
                Some(candidate) => candidate.clone(),
                None => {
                    state.backend = Backend::None;
                    cx.notify();
                    return;
                }
            },
        };
        let kind = candidate.kind();
        state.backend = Backend::Connecting(kind);
        let generation = state.generation;
        cx.notify();
        if !self.live {
            return;
        }
        let id = cluster.clone();
        let keep_query_values = Self::settings(cx).keep_query_values;
        let task = cx.spawn(async move |this, cx| {
            let result = connect(&id, candidate, keep_query_values, generation, cx).await;
            this.update(cx, |this, cx| {
                let current = this
                    .clusters
                    .get(&id)
                    .is_some_and(|state| state.generation == generation);
                if !current {
                    // Rebuilt or rekeyed meanwhile: a forward this connection started must go.
                    if let Ok((_, _, Some(forward))) = &result {
                        let forward = *forward;
                        PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx));
                    }
                    return;
                }
                let Some(state) = this.clusters.get_mut(&id) else {
                    return;
                };
                match result {
                    Ok((provider, status, forward)) => {
                        state.forward = forward;
                        state.backend = Backend::Ready {
                            kind,
                            provider,
                            status,
                        };
                        state.status_read = Some(Instant::now());
                        let keys: Vec<String> = state.streams.keys().cloned().collect();
                        for key in keys {
                            this.start_stream(&id, &key, cx);
                        }
                    }
                    Err(error) => state.backend = Backend::Failed(kind, error),
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.tasks.retain(|t| !t.is_ready());
            state.tasks.push(task);
        }
    }

    /// Writes `netflow.clusters.<cluster>.backend` (the header's backend menu).
    pub fn use_backend(cluster: &ClusterId, kind: BackendKind, cx: &mut App) {
        let keys = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).settings_keys(cluster))
            .unwrap_or_else(|| vec![cluster.to_string()]);
        let settings = Self::settings(cx);
        // Where an override already is, else the context name.
        let key = keys
            .iter()
            .find(|k| settings.clusters.contains_key(*k))
            .or(keys.last())
            .cloned()
            .unwrap_or_else(|| cluster.to_string());
        kubyl_settings::Settings::update::<NetflowSettings>(cx, move |s| {
            s.clusters.entry(key).or_default().backend = Some(BackendSetting::of(kind));
        });
    }

    // ----- Streams -----

    fn start_stream(&mut self, cluster: &ClusterId, key: &str, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(provider) = state.provider() else {
            return;
        };
        let Some(stream) = state.streams.get_mut(key) else {
            return;
        };
        stream.tasks.retain(|t| !t.is_ready());
        stream.generation += 1;
        let generation = stream.generation;
        stream.status = StreamStatus::Starting;
        // History from the window's start, or resume after the newest flow held.
        let since = match stream.buffer.newest() {
            Some(newest) => newest.checked_add(jiff::SignedDuration::from_nanos(1)).ok(),
            None => {
                stream.caught_up = false;
                Timestamp::now()
                    .checked_sub(jiff::SignedDuration::try_from(stream.window).unwrap_or_default())
                    .ok()
            }
        };
        let query = StreamQuery {
            filter: stream.filter.clone(),
            since,
        };
        if !self.live {
            return;
        }
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(256);
        let work = spawn_kube(cx, provider.stream(query, tx));
        let id = cluster.clone();
        let key = key.to_string();
        let task = cx.spawn(async move |this, cx| {
            // Batches go to the views at most once a frame.
            while let Some(first) = rx.next().await {
                let mut events = vec![first];
                while let Ok(event) = rx.try_recv() {
                    events.push(event);
                }
                let alive = this
                    .update(cx, |this, cx| this.apply(&id, &key, generation, events, cx))
                    .unwrap_or(false);
                if !alive {
                    return;
                }
                cx.background_executor().timer(FRAME).await;
            }
            let result = work.await;
            this.update(cx, |this, cx| {
                this.stream_ended(&id, &key, generation, result, cx)
            })
            .ok();
        });
        stream.tasks.push(task);
    }

    fn apply(
        &mut self,
        cluster: &ClusterId,
        key: &str,
        generation: u64,
        events: Vec<StreamEvent>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return false;
        };
        let Some(stream) = state.streams.get_mut(key) else {
            return false;
        };
        if stream.generation != generation {
            return false;
        }
        let mut arrived = 0;
        for event in events {
            match event {
                StreamEvent::Flows(flows) => {
                    arrived += flows.len();
                    stream.buffer.push(flows);
                }
                StreamEvent::CaughtUp => stream.caught_up = true,
                StreamEvent::Status(status) => {
                    if let Backend::Ready {
                        status: current, ..
                    } = &mut state.backend
                    {
                        *current = status;
                    }
                }
            }
        }
        if arrived > 0 {
            stream.arrivals.push_back((Instant::now(), arrived));
            while stream.arrivals.len() > 1_000 {
                stream.arrivals.pop_front();
            }
            stream.failures = 0;
        }
        if stream.status != StreamStatus::Live {
            stream.status = StreamStatus::Live;
        }
        stream.revision += 1;
        cx.notify();
        true
    }

    fn stream_ended(
        &mut self,
        cluster: &ClusterId,
        key: &str,
        generation: u64,
        result: Result<(), ProviderError>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(stream) = state.streams.get_mut(key) else {
            return;
        };
        if stream.generation != generation {
            return;
        }
        let error = match result {
            Ok(()) => ProviderError::Unavailable("The flow stream ended.".into()),
            Err(error) => error,
        };
        stream.failures += 1;
        stream.status = StreamStatus::Retrying(error);
        let backoff = Duration::from_secs(2u64.saturating_pow(stream.failures.min(4)).min(30));
        let id = cluster.clone();
        let key = key.to_string();
        // A new task: this one must not drop itself.
        let retry = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(backoff).await;
            this.update(cx, |this, cx| {
                let current = this
                    .clusters
                    .get(&id)
                    .and_then(|s| s.streams.get(&key))
                    .map(|s| s.generation);
                if current == Some(generation) {
                    this.start_stream(&id, &key, cx);
                }
            })
            .ok();
        });
        stream.tasks.push(retry);
        cx.notify();
    }

    /// The Active Sessions row's stop button stopped the forward to Relay or Whisker.
    fn forward_stopped(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.forward = None;
        let kind = match &state.backend {
            Backend::Ready { kind, .. } | Backend::Connecting(kind) => *kind,
            _ => return,
        };
        self.reset(cluster, cx);
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.backend = Backend::Failed(
                kind,
                ProviderError::Unavailable(format!(
                    "The port-forward to {} was stopped in Active Sessions.",
                    kind.label()
                )),
            );
        }
        cx.notify();
    }

    // ----- Housekeeping -----

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.tick_at(Instant::now(), cx);
    }

    pub(crate) fn tick_at(&mut self, now: Instant, cx: &mut Context<Self>) {
        let stamp = Timestamp::now();
        let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        let mut changed = false;
        for id in ids {
            let Some(state) = self.clusters.get_mut(&id) else {
                continue;
            };
            // Streams nobody holds.
            let mut idle = Vec::new();
            for (key, stream) in &mut state.streams {
                if Rc::strong_count(&stream.lease) > 1 {
                    stream.idle_since = None;
                } else if stream.idle_since.is_none() {
                    stream.idle_since = Some(now);
                } else if stream
                    .idle_since
                    .is_some_and(|t| now.duration_since(t) >= GRACE)
                {
                    idle.push(key.clone());
                }
                if stream.buffer.expire(stamp) > 0 {
                    stream.revision += 1;
                    changed = true;
                }
            }
            for key in idle {
                state.streams.remove(&key);
                changed = true;
            }
            state
                .graphs
                .retain(|_, g| now.duration_since(g.wanted) < GRACE * 5);
            // The cluster nobody looks at: stop the backend and its forward.
            if Rc::strong_count(&state.lease) > 1 {
                state.idle_since = None;
            } else if state.idle_since.is_none() {
                state.idle_since = Some(now);
            } else if state
                .idle_since
                .is_some_and(|t| now.duration_since(t) >= GRACE)
                && !matches!(state.backend, Backend::None | Backend::Off)
            {
                self.reset(&id, cx);
                changed = true;
                continue;
            }
            // Status (nodes, buffered flows) now and then.
            if Rc::strong_count(&state.lease) > 1
                && state
                    .status_read
                    .is_some_and(|t| now.duration_since(t) >= STATUS_EVERY)
                && let Some(provider) = state.provider()
            {
                state.status_read = Some(now);
                let generation = state.generation;
                let work = spawn_kube(cx, provider.probe());
                let cluster = id.clone();
                let task = cx.spawn(async move |this, cx| {
                    if let Ok(status) = work.await {
                        this.update(cx, |this, cx| {
                            if let Some(state) = this.clusters.get_mut(&cluster)
                                && state.generation == generation
                                && let Backend::Ready {
                                    status: current, ..
                                } = &mut state.backend
                            {
                                *current = status;
                                cx.notify();
                            }
                        })
                        .ok();
                    }
                });
                state.tasks.retain(|t| !t.is_ready());
                state.tasks.push(task);
            }
        }
        // Metrics graphs due for a refresh.
        let due: Vec<(ClusterId, String)> = self
            .clusters
            .iter()
            .flat_map(|(id, s)| {
                s.graphs
                    .iter()
                    .filter(|(_, g)| {
                        !g.loading
                            && g.fetched
                                .is_none_or(|t| now.duration_since(t) >= GRAPH_EVERY)
                    })
                    .map(|(k, _)| (id.clone(), k.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (id, key) in due {
            self.fetch_graph(&id, &key, cx);
        }
        // Rates and ages in the header.
        if changed || self.clusters.values().any(|s| !s.streams.is_empty()) {
            cx.notify();
        }
    }

    fn connection_event(&mut self, event: &kubyl_kube::ConnectionEvent, cx: &mut Context<Self>) {
        use kubyl_kube::ConnectionEvent;
        match event {
            ConnectionEvent::StateChanged(id) | ConnectionEvent::DiscoveryChanged(id) => {
                if self.clusters.contains_key(id) {
                    self.detect(id, cx);
                }
                cx.notify();
            }
            ConnectionEvent::Rekeyed { from, to } => {
                if let Some(state) = self.clusters.remove(from) {
                    // The backend holds the old connection's client: build it again.
                    // A state already under the new id is replaced: its forward must not leak.
                    if let Some(old) = self.clusters.insert(to.clone(), state)
                        && let Some(forward) = old.forward
                    {
                        PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx));
                    }
                    self.reset(to, cx);
                    self.redetect(to, cx);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn settings_changed(&mut self, cx: &mut Context<Self>) {
        let settings = Self::settings(cx);
        let ids: Vec<ClusterId> = self.clusters.keys().cloned().collect();
        for id in ids {
            if let Some(state) = self.clusters.get_mut(&id) {
                for stream in state.streams.values_mut() {
                    let max_age = Duration::from_secs(settings.max_age_minutes.max(1) * 60)
                        .max(stream.window);
                    stream.buffer.set_limits(settings.max_flows, max_age);
                }
            }
            // A changed override or keep_query_values runs detection again.
            self.detect(&id, cx);
        }
    }

    // ----- For views -----

    /// What a view shows for `cluster`.
    pub fn state(&self, cluster: &ClusterId, cx: &App) -> FlowState {
        let connected =
            ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).client(cluster).is_some());
        #[cfg(test)]
        let connected = connected || self.assume_connected;
        if !connected {
            return FlowState::NotConnected;
        }
        let Some(state) = self.clusters.get(cluster) else {
            return FlowState::Detecting;
        };
        let Detect::Done(detection) = &state.detect else {
            return FlowState::Detecting;
        };
        let detection = detection.clone();
        match &state.backend {
            Backend::Off => FlowState::Off,
            Backend::None if detection.candidates.is_empty() => FlowState::NoBackend(detection),
            // Chosen but not connected yet (after going idle).
            Backend::None => FlowState::Connecting {
                kind: detection.candidates[0].kind(),
                detection,
            },
            Backend::Connecting(kind) => FlowState::Connecting {
                kind: *kind,
                detection,
            },
            Backend::Failed(kind, error) => FlowState::Failed {
                kind: *kind,
                error: error.clone(),
                detection,
            },
            Backend::Ready {
                kind,
                provider,
                status,
            } => FlowState::Ready {
                kind: *kind,
                status: status.clone(),
                capabilities: provider.capabilities(),
                detection,
            },
        }
    }

    /// The part of `filter` the connected backend applies itself.
    pub fn pushdown(&self, cluster: &ClusterId, filter: &FlowFilter) -> FlowFilter {
        self.clusters
            .get(cluster)
            .and_then(ClusterFlows::provider)
            .map(|p| p.pushdown(filter))
            .unwrap_or_default()
    }

    pub fn stream(&self, cluster: &ClusterId, filter: &FlowFilter) -> Option<&FlowStream> {
        self.clusters.get(cluster)?.streams.get(&filter.canonical())
    }

    /// Every stream of a cluster (a details section asking for an object's flows).
    pub fn streams(&self, cluster: &ClusterId) -> impl Iterator<Item = &FlowStream> {
        self.clusters
            .get(cluster)
            .into_iter()
            .flat_map(|s| s.streams.values())
    }

    /// The topology from the backend's metrics, fetched while asked for (every 30 s).
    pub fn metrics_graph(
        &mut self,
        cluster: &ClusterId,
        zoom: Zoom,
        window: Duration,
        filter: &FlowFilter,
        cx: &mut Context<Self>,
    ) -> Option<MetricsGraph> {
        let key = format!("{}|{}|{}", zoom.key(), window.as_secs(), filter.canonical());
        let state = self.clusters.get_mut(cluster)?;
        let provider = state.provider()?;
        if !provider.capabilities().graph_from_metrics {
            return None;
        }
        let fresh = !state.graphs.contains_key(&key);
        let graph = state
            .graphs
            .entry(key.clone())
            .or_insert_with(|| MetricsGraph {
                topology: None,
                error: None,
                loading: false,
                fetched: None,
                wanted: Instant::now(),
            });
        graph.wanted = Instant::now();
        let snapshot = graph.clone();
        if fresh {
            self.fetch_graph(cluster, &key, cx);
        }
        Some(snapshot)
    }

    fn fetch_graph(&mut self, cluster: &ClusterId, key: &str, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(provider) = state.provider() else {
            return;
        };
        let Some(graph) = state.graphs.get_mut(key) else {
            return;
        };
        let mut parts = key.splitn(3, '|');
        let zoom = if parts.next() == Some("workloads") {
            Zoom::Workloads
        } else {
            Zoom::Namespaces
        };
        let window = Duration::from_secs(parts.next().and_then(|w| w.parse().ok()).unwrap_or(900));
        let filter = FlowFilter::parse(parts.next().unwrap_or_default()).unwrap_or_default();
        graph.loading = true;
        if !self.live {
            return;
        }
        let generation = state.generation;
        let work = spawn_kube(cx, provider.graph(window, zoom, filter));
        let id = cluster.clone();
        let key = key.to_string();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.clusters.get_mut(&id) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                if let Some(graph) = state.graphs.get_mut(&key) {
                    graph.loading = false;
                    graph.fetched = Some(Instant::now());
                    match result {
                        Ok(topology) => {
                            graph.topology = topology.map(Arc::new);
                            graph.error = None;
                        }
                        Err(error) => graph.error = Some(error),
                    }
                }
                cx.notify();
            })
            .ok();
        });
        state.tasks.retain(|t| !t.is_ready());
        state.tasks.push(task);
    }

    /// Puts a ready backend and flows in place without a cluster (GPUI tests).
    #[cfg(test)]
    pub fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        provider: Arc<dyn FlowProvider>,
        detection: Detection,
        cx: &mut Context<Self>,
    ) {
        let mut state = ClusterFlows::new();
        state.detect = Detect::Done(Arc::new(detection));
        state.backend = Backend::Ready {
            kind: provider.kind(),
            provider,
            status: BackendStatus::default(),
        };
        self.clusters.insert(cluster.clone(), state);
        self.assume_connected = true;
        cx.notify();
    }

    /// Adds flows to a stream (GPUI tests).
    #[cfg(test)]
    pub fn push_for_test(
        &mut self,
        cluster: &ClusterId,
        filter: &FlowFilter,
        flows: Vec<crate::model::Flow>,
        cx: &mut Context<Self>,
    ) {
        if let Some(stream) = self
            .clusters
            .get_mut(cluster)
            .and_then(|s| s.streams.get_mut(&filter.canonical()))
        {
            let generation = stream.generation;
            self.apply(
                cluster,
                &filter.canonical(),
                generation,
                vec![StreamEvent::Flows(flows), StreamEvent::CaughtUp],
                cx,
            );
        }
    }

    /// Whether a cluster's backend and forward are gone (GPUI tests).
    #[cfg(test)]
    pub fn idle_for_test(&self, cluster: &ClusterId) -> bool {
        self.clusters
            .get(cluster)
            .is_none_or(|s| s.forward.is_none() && s.streams.is_empty())
    }
}

/// Connects to a candidate: checks permissions, opens the forward (Hubble), builds the
/// provider and probes it. Returns the provider, its status and the forward it opened.
async fn connect(
    cluster: &ClusterId,
    candidate: Candidate,
    keep_query_values: bool,
    generation: u64,
    cx: &mut AsyncApp,
) -> Result<(Arc<dyn FlowProvider>, BackendStatus, Option<ForwardId>), ProviderError> {
    let client = cx
        .update(|cx| ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster)))
        .ok_or_else(|| ProviderError::Unavailable("The cluster isn't connected.".into()))?;
    let can = |verb: &'static str,
               resource: &'static str,
               subresource: Option<&'static str>,
               namespace: String,
               cx: &mut AsyncApp| {
        let client = client.clone();
        cx.update(|cx| {
            spawn_kube(cx, async move {
                let mut query =
                    AccessQuery::new(verb, &Gvr::new("", "v1", resource), Some(&namespace));
                if let Some(sub) = subresource {
                    query = query.subresource(sub);
                }
                // An access review that fails isn't a denial: the call itself will tell.
                access::check(client, query).await.unwrap_or(true)
            })
        })
    };
    match candidate {
        Candidate::Hubble {
            namespace,
            service: name,
            port,
            tls,
        } => {
            let tls = relay_tls(tls)?;
            if !can("create", "pods", Some("portforward"), namespace.clone(), cx).await {
                return Err(ProviderError::forbidden(
                    "create",
                    "pods/portforward",
                    Some(&namespace),
                ));
            }
            let local_port =
                start_forward(cluster, &client, &namespace, &name, port, generation, cx).await?;
            let hubble = Hubble {
                target: HubbleTarget {
                    namespace,
                    service: name,
                    port,
                    tls,
                },
                local_port: local_port.1,
                keep_query_values,
            };
            let probe = cx.update(|cx| spawn_kube(cx, hubble.probe()));
            match probe.await {
                Ok(status) => Ok((Arc::new(hubble), status, Some(local_port.0))),
                Err(err) => {
                    let forward = local_port.0;
                    cx.update(|cx| {
                        PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx))
                    });
                    Err(err)
                }
            }
        }
        Candidate::Whisker(target) => {
            // A forward, not the service proxy: Calico's own policy only lets Whisker be
            // reached from inside its pod (see backends::whisker).
            if !can(
                "create",
                "pods",
                Some("portforward"),
                target.namespace.clone(),
                cx,
            )
            .await
            {
                return Err(ProviderError::forbidden(
                    "create",
                    "pods/portforward",
                    Some(&target.namespace),
                ));
            }
            let port: u16 = target.port.parse().map_err(|_| {
                ProviderError::Unsupported(format!(
                    "Whisker's port {} isn't a number (netflow.clusters.<cluster>.whisker.port)",
                    target.port
                ))
            })?;
            let (forward, local_port) = start_forward(
                cluster,
                &client,
                &target.namespace,
                &target.service,
                port,
                generation,
                cx,
            )
            .await?;
            let whisker = Whisker {
                client,
                target,
                local_port,
            };
            match cx.update(|cx| spawn_kube(cx, whisker.probe())).await {
                Ok(status) => Ok((Arc::new(whisker), status, Some(forward))),
                Err(err) => {
                    cx.update(|cx| {
                        PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx))
                    });
                    Err(err)
                }
            }
        }
        Candidate::NetObserv { loki, prometheus } => {
            let loki = match loki {
                LokiTarget::Service(target) => {
                    if can("get", "services", Some("proxy"), target.namespace.clone(), cx).await {
                        NetObserv::loki_target(client.clone(), &target)
                    } else {
                        LokiAccess::Missing(format!(
                            "No single flows: reading NetObserv's Loki ({}) needs get services/proxy in {}.",
                            target.label(),
                            target.namespace
                        ))
                    }
                }
                // Built on Tokio: kube's client spawns a task when it's made.
                LokiTarget::Url(url) => {
                    cx.update(|cx| {
                        spawn_kube(cx, async move {
                            match kubyl_metrics::transport::Transport::external(&url, None, &Default::default()) {
                                Ok(transport) => LokiAccess::Ready {
                                    transport,
                                    label: format!("Loki {url}"),
                                },
                                Err(err) => LokiAccess::Missing(format!("NetObserv's Loki URL {url}: {err}")),
                            }
                        })
                    })
                    .await
                }
                LokiTarget::External(url) => LokiAccess::Missing(format!(
                    "No single flows: the FlowCollector points at Loki {url}, outside the cluster. Set the Loki URL in Kubyl's NetObserv settings to read it from here."
                )),
                LokiTarget::LokiStack { namespace, name } => LokiAccess::Missing(format!(
                    "No single flows: NetObserv stores them in the LokiStack {namespace}/{name}, whose gateway needs your token, and the API server's service proxy doesn't pass it on. The topology comes from NetObserv's metrics."
                )),
                LokiTarget::Disabled => LokiAccess::Missing(
                    "No single flows: Loki is off in NetObserv's FlowCollector (spec.loki.enable). The topology comes from NetObserv's metrics.".into(),
                ),
            };
            let prometheus = match prometheus {
                PromTarget::Service(target) => Some(kubyl_metrics::prometheus::PromClient::new(
                    client.clone(),
                    kubyl_metrics::prometheus::Target::Service {
                        namespace: target.namespace,
                        service: target.service,
                        port: target.port,
                        scheme: target.scheme,
                        path: target.path,
                    },
                )),
                PromTarget::Cluster => cx.update(|cx| {
                    kubyl_metrics::MetricsService::global(cx)
                        .and_then(|m| m.read(cx).prometheus(cluster))
                }),
            };
            let netobserv = NetObserv { loki, prometheus };
            let status = cx.update(|cx| spawn_kube(cx, netobserv.probe())).await?;
            Ok((Arc::new(netobserv), status, None))
        }
    }
}

/// Opens the temporary forward to Relay (shown in Active Sessions) and waits until it
/// listens. Returns the forward and its loopback port.
async fn start_forward(
    cluster: &ClusterId,
    client: &kube::Client,
    namespace: &str,
    service: &str,
    port: u16,
    generation: u64,
    cx: &mut AsyncApp,
) -> Result<(ForwardId, u16), ProviderError> {
    let on_stop_cluster = cluster.clone();
    let spec = ForwardSpec {
        cluster: cluster.clone(),
        namespace: namespace.to_string(),
        kind: ForwardKind::Service {
            service: service.to_string(),
        },
        port: RemotePort::Service(Some(port)),
        // Loopback only.
        bind_address: "127.0.0.1".into(),
        local_port: 0,
        target: ResourceRef::object(
            cluster.clone(),
            Gvr::new("", "v1", "services"),
            Some(namespace.to_string()),
            service.to_string(),
        ),
        remote_port: Some(port),
        http: false,
        https: false,
        open_browser: false,
        ephemeral: Some(Ephemeral {
            title: format!("Network flows · svc/{service}"),
            buttons: Vec::new(),
            on_stop: Arc::new(move |cx| {
                let cluster = on_stop_cluster.clone();
                if let Some(flows) = FlowService::global(cx) {
                    flows.update(cx, |this, cx| {
                        let current = this.clusters.get(&cluster).map(|s| s.generation);
                        if current == Some(generation) {
                            this.forward_stopped(&cluster, cx);
                        }
                    });
                }
            }),
        }),
    };
    let id = cx.update(|cx| PortForwardManager::start(client.clone(), spec, cx));
    let started = Instant::now();
    let mut reconnecting_since: Option<Instant> = None;
    loop {
        let info = cx.update(|cx| PortForwardManager::global(cx).read(cx).info(id));
        match info {
            Some(info) if info.state == ForwardState::Listening && info.local_port != 0 => {
                return Ok((id, info.local_port));
            }
            Some(info) if matches!(info.state, ForwardState::Failed(_)) => {
                cx.update(|cx| PortForwardManager::global(cx).update(cx, |m, cx| m.stop(id, cx)));
                let ForwardState::Failed(err) = info.state else {
                    unreachable!()
                };
                return Err(ProviderError::Unavailable(format!(
                    "The port-forward to {namespace}/{service} failed: {err}"
                )));
            }
            Some(info) => {
                // A target that doesn't resolve (no ready Relay pod, a denied read).
                if let ForwardState::Reconnecting(err) = &info.state {
                    let since = *reconnecting_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(8) {
                        let err = err.clone();
                        cx.update(|cx| {
                            PortForwardManager::global(cx).update(cx, |m, cx| m.stop(id, cx))
                        });
                        return Err(if err.to_lowercase().contains("forbidden") {
                            ProviderError::forbidden("get", "pods", Some(namespace))
                        } else {
                            ProviderError::Unavailable(format!(
                                "The port-forward to {namespace}/{service} doesn't reach a pod: {err}"
                            ))
                        });
                    }
                }
            }
            None => {
                return Err(ProviderError::Unavailable(
                    "The port-forward stopped.".into(),
                ));
            }
        }
        if started.elapsed() > FORWARD_TIMEOUT {
            cx.update(|cx| PortForwardManager::global(cx).update(cx, |m, cx| m.stop(id, cx)));
            return Err(ProviderError::Unavailable(format!(
                "The port-forward to {namespace}/{service} didn't start in {} s.",
                FORWARD_TIMEOUT.as_secs()
            )));
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}
