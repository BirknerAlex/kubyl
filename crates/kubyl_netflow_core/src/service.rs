//! [`FlowsCore`]: the app-wide, demand-driven flow state (README "Flow buffer and streaming").
//! Per cluster it detects the backends, picks one (settings may override), checks the
//! permissions it needs, connects (a temporary loopback forward for Hubble Relay and Whisker,
//! the service proxy for NetObserv) and runs one stream per (cluster, server-side filter) while
//! a view holds a [`FlowLease`]. Streams run on Tokio and are drained on the host's thread at
//! most 60 times a second into ring buffers. When the last lease goes, the streams stop after a
//! short grace and the forward with them. Nothing is written anywhere.
//!
//! The service runs on any [`Host`]. What it reads from the app (settings, the clusters'
//! connections) comes in as an [`Env`] snapshot with each call that can need it; forwards come
//! from a [`Reach`]; the only effect it asks for is [`FlowsEffect::Prometheus`].

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use futures::{StreamExt as _, stream};
use jiff::Timestamp;
use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::{ClusterId, Gvr};
use kubyl_kube_core::access::{self, AccessQuery};
use kubyl_metrics_core::prometheus::{PromClient, Target};
use kubyl_portforward_core::reach::{Reach, ReachPort, ReachRequest, Reached};

pub use crate::state::*;

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

/// How long Relay's forward may reconnect (its target doesn't resolve) before connecting fails.
const RECONNECT_GRACE: Duration = Duration::from_secs(8);

/// A cluster's connection as the service needs it.
#[derive(Clone)]
pub struct Connection {
    pub client: kube::Client,
    /// What discovery says the cluster serves; `None`: discovery isn't known yet.
    pub served: Option<Served>,
    /// `/version`'s `gitVersion`.
    pub git_version: String,
}

/// What the app knows about one cluster.
#[derive(Clone)]
pub struct ClusterEnv {
    /// Where the cluster's settings live (`ConnectionManager::settings_keys`).
    pub settings_keys: Vec<String>,
    /// `None`: not connected.
    pub connection: Option<Connection>,
}

/// What the service reads from the app: the settings and the connections of the clusters it
/// is asked about. Passed to every call that can need it ([`FlowsCore::tracked`] lists the
/// clusters a snapshot should cover besides the one asked about).
#[derive(Clone, Default)]
pub struct Env {
    pub settings: NetflowSettings,
    pub clusters: HashMap<ClusterId, ClusterEnv>,
}

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
    /// The backend's work and the drain of its flows.
    tasks: Vec<TaskHandle>,
    /// The wait before the stream starts again after a failure.
    retry: Option<TaskHandle>,
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
    /// The settings keys and connection of the last [`Env`].
    keys: Vec<String>,
    connection: Option<Connection>,
    /// The forward to Relay or Whisker.
    forward: Option<Reached>,
    /// Hears when the forward was stopped from outside.
    watcher: Option<TaskHandle>,
    /// NetObserv's Loki, waiting for the app to say where the cluster's Prometheus is.
    pending_loki: Option<LokiAccess>,
    streams: HashMap<String, FlowStream>,
    graphs: HashMap<String, MetricsGraph>,
    graph_tasks: HashMap<String, TaskHandle>,
    lease: Rc<()>,
    idle_since: Option<Instant>,
    status_read: Option<Instant>,
    /// Bumped when the backend is rebuilt; older work is dropped.
    generation: u64,
    detect_task: Option<TaskHandle>,
    connect_task: Option<TaskHandle>,
    status_task: Option<TaskHandle>,
}

impl ClusterFlows {
    fn new(cluster: &ClusterId) -> Self {
        Self {
            detect: Detect::Waiting,
            detected_with: None,
            backend: Backend::None,
            keys: vec![cluster.to_string()],
            connection: None,
            forward: None,
            watcher: None,
            pending_loki: None,
            streams: HashMap::new(),
            graphs: HashMap::new(),
            graph_tasks: HashMap::new(),
            lease: Rc::new(()),
            idle_since: None,
            status_read: None,
            generation: 0,
            detect_task: None,
            connect_task: None,
            status_task: None,
        }
    }

    fn provider(&self) -> Option<Arc<dyn FlowProvider>> {
        match &self.backend {
            Backend::Ready { provider, .. } => Some(provider.clone()),
            _ => None,
        }
    }
}

/// What the service asks the app to do.
pub enum FlowsEffect {
    /// NetObserv's topology can come from the cluster's Prometheus: read it from the metrics
    /// service and answer with [`FlowsCore::prometheus_found`].
    Prometheus { cluster: ClusterId, generation: u64 },
}

/// The flow service's state. See the module docs.
pub struct FlowsCore {
    clusters: HashMap<ClusterId, ClusterFlows>,
    settings: NetflowSettings,
    /// Timers and connections run (off in GPUI tests).
    live: bool,
    reach: Arc<dyn Reach>,
    tick: Option<TaskHandle>,
}

impl Service for FlowsCore {
    type Event = Infallible;
    type Effect = FlowsEffect;
}

impl FlowsCore {
    /// `live`: timers and connections (off in tests that must not wake timers). `reach` opens
    /// the forwards to Relay and Whisker.
    pub fn new(live: bool, reach: Arc<dyn Reach>) -> Self {
        Self {
            clusters: HashMap::new(),
            settings: NetflowSettings::default(),
            live,
            reach,
            tick: None,
        }
    }

    /// Starts the once-a-second housekeeping (when live).
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        if self.live {
            self.tick = Some(host.every(TICK, |this, host| {
                this.tick_at(Instant::now(), host);
                Flow::Continue
            }));
        }
    }

    /// The clusters the service follows; an [`Env`] should cover them.
    pub fn tracked(&self) -> Vec<ClusterId> {
        self.clusters.keys().cloned().collect()
    }

    /// Takes in the latest settings and connections.
    fn sync(&mut self, env: &Env) {
        self.settings = env.settings.clone();
        for (id, cluster) in &env.clusters {
            if let Some(state) = self.clusters.get_mut(id) {
                state.keys = cluster.settings_keys.clone();
                state.connection = cluster.connection.clone();
            }
        }
    }

    fn cluster_settings(&self, cluster: &ClusterId) -> ClusterNetflowSettings {
        match self.clusters.get(cluster) {
            Some(state) => self.settings.for_keys(&state.keys),
            None => self.settings.for_keys(&[cluster.to_string()]),
        }
    }

    // ----- Leases -----

    /// Keeps `cluster`'s stream with the server-side `filter` going (history from `window`
    /// back) while the lease lives. Starts detection and the backend when needed.
    pub fn lease(
        &mut self,
        cluster: &ClusterId,
        filter: &FlowFilter,
        window: Duration,
        env: &Env,
        host: &mut dyn Host<Self>,
    ) -> FlowLease {
        self.sync(env);
        let settings = self.settings.clone();
        self.ensure(cluster, env, host);
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
                retry: None,
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
            stream.retry = None;
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
            self.start_stream(cluster, &key, host);
        }
        lease
    }

    /// Keeps `cluster` detected and its backend connected while the lease lives (a view before
    /// it knows its stream).
    pub fn watch(
        &mut self,
        cluster: &ClusterId,
        env: &Env,
        host: &mut dyn Host<Self>,
    ) -> FlowLease {
        self.sync(env);
        self.ensure(cluster, env, host);
        let state = self.clusters.get_mut(cluster).expect("ensured");
        state.idle_since = None;
        FlowLease {
            _cluster: state.lease.clone(),
            _stream: Rc::new(()),
        }
    }

    /// Starts tracking a cluster (detection, then the backend).
    fn ensure(&mut self, cluster: &ClusterId, env: &Env, host: &mut dyn Host<Self>) {
        if !self.clusters.contains_key(cluster) {
            self.clusters
                .insert(cluster.clone(), ClusterFlows::new(cluster));
            self.sync(env);
            self.detect(cluster, host);
        } else if matches!(self.clusters[cluster].backend, Backend::None)
            && matches!(self.clusters[cluster].detect, Detect::Done(_))
        {
            // It went idle: connect again.
            self.choose(cluster, host);
        }
    }

    // ----- Detection and connection -----

    /// Detects again when the inputs changed (or `force`).
    fn detect_with(&mut self, cluster: &ClusterId, force: bool, host: &mut dyn Host<Self>) {
        let settings = self.cluster_settings(cluster);
        let keep = self.settings.keep_query_values;
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let inputs = state.connection.as_ref().and_then(|connection| {
            Some(Inputs {
                client: connection.client.clone(),
                served: connection.served.clone()?,
                git_version: connection.git_version.clone(),
                settings,
            })
        });
        let Some(inputs) = inputs else {
            // Disconnected, or discovery unknown: never "no backend" before it's known.
            if !matches!(state.detect, Detect::Waiting) || state.forward.is_some() {
                self.reset(cluster);
            }
            if let Some(state) = self.clusters.get_mut(cluster) {
                state.detect = Detect::Waiting;
                state.detected_with = None;
            }
            host.notify();
            return;
        };
        let key = (inputs.served.clone(), inputs.settings.clone(), keep);
        if !force && state.detected_with.as_ref() == Some(&key) {
            return;
        }
        state.detected_with = Some(key);
        self.reset(cluster);
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.detect = Detect::Running;
        state.generation += 1;
        let generation = state.generation;
        if !self.live {
            return;
        }
        let id = cluster.clone();
        state.detect_task = Some(host.spawn(
            detect::detect(inputs),
            move |this, detection, host| {
                let Some(state) = this.clusters.get_mut(&id) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.detect = Detect::Done(Arc::new(detection));
                this.choose(&id, host);
                host.notify();
            },
        ));
        host.notify();
    }

    fn detect(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        self.detect_with(cluster, false, host);
    }

    /// Looks for backends again (the view's "Look again").
    pub fn redetect(&mut self, cluster: &ClusterId, env: &Env, host: &mut dyn Host<Self>) {
        self.sync(env);
        if self.clusters.contains_key(cluster) {
            self.detect_with(cluster, true, host);
        }
    }

    /// Connects again after a failure (the view's "Try again").
    pub fn reconnect(&mut self, cluster: &ClusterId, env: &Env, host: &mut dyn Host<Self>) {
        self.sync(env);
        if self.clusters.contains_key(cluster) {
            self.reset(cluster);
            self.choose(cluster, host);
        }
    }

    /// Stops streams, the forward and the backend (detection stays).
    fn reset(&mut self, cluster: &ClusterId) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.generation += 1;
        state.backend = Backend::None;
        state.status_read = None;
        state.graphs.clear();
        state.graph_tasks.clear();
        state.pending_loki = None;
        for stream in state.streams.values_mut() {
            stream.tasks.clear();
            stream.retry = None;
            stream.status = StreamStatus::Starting;
            stream.caught_up = false;
            stream.generation += 1;
        }
        state.watcher = None;
        // Dropping the forward stops it.
        state.forward = None;
    }

    /// Picks the backend from detection and settings, then connects.
    fn choose(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let settings = self.cluster_settings(cluster);
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
                host.notify();
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
                        host.notify();
                        return;
                    }
                }
            }
            None => match detection.candidates.first() {
                Some(candidate) => candidate.clone(),
                None => {
                    state.backend = Backend::None;
                    host.notify();
                    return;
                }
            },
        };
        let kind = candidate.kind();
        state.backend = Backend::Connecting(kind);
        let generation = state.generation;
        host.notify();
        if !self.live {
            return;
        }
        let id = cluster.clone();
        let client = state.connection.as_ref().map(|c| c.client.clone());
        let work = connect(
            cluster.clone(),
            candidate,
            client,
            self.settings.keep_query_values,
            self.reach.clone(),
        );
        state.connect_task = Some(host.spawn(work, move |this, result, host| {
            match result {
                Ok(Built::NeedsPrometheus(loki)) => {
                    // The app knows where the cluster's Prometheus is.
                    let Some(state) = this.clusters.get_mut(&id) else {
                        return;
                    };
                    if state.generation == generation {
                        state.pending_loki = Some(loki);
                        host.effect(FlowsEffect::Prometheus {
                            cluster: id,
                            generation,
                        });
                    }
                }
                result => this.connected(&id, generation, kind, result, host),
            }
        }));
    }

    /// The app's answer to [`FlowsEffect::Prometheus`]: the cluster's Prometheus, if it has one.
    pub fn prometheus_found(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        prometheus: Option<PromClient>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if state.generation != generation {
            return;
        }
        let (Backend::Connecting(kind), Some(loki)) = (&state.backend, state.pending_loki.take())
        else {
            return;
        };
        let kind = *kind;
        let netobserv = NetObserv { loki, prometheus };
        let id = cluster.clone();
        state.connect_task = Some(host.spawn(
            async move {
                let status = netobserv.probe().await?;
                Ok(Built::Ready {
                    provider: Arc::new(netobserv),
                    status,
                    forward: None,
                })
            },
            move |this, result, host| this.connected(&id, generation, kind, result, host),
        ));
    }

    /// A connection attempt ended.
    fn connected(
        &mut self,
        cluster: &ClusterId,
        generation: u64,
        kind: BackendKind,
        result: Result<Built, ProviderError>,
        host: &mut dyn Host<Self>,
    ) {
        let current = self
            .clusters
            .get(cluster)
            .is_some_and(|state| state.generation == generation);
        if !current {
            // Rebuilt or rekeyed meanwhile: a forward this connection opened goes with it.
            return;
        }
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        match result {
            Ok(Built::Ready {
                provider,
                status,
                forward,
            }) => {
                state.forward = forward;
                state.backend = Backend::Ready {
                    kind,
                    provider,
                    status,
                };
                state.status_read = Some(Instant::now());
                self.watch_forward(cluster, host);
                let Some(state) = self.clusters.get_mut(cluster) else {
                    return;
                };
                let keys: Vec<String> = state.streams.keys().cloned().collect();
                for key in keys {
                    self.start_stream(cluster, &key, host);
                }
            }
            Ok(Built::NeedsPrometheus(_)) => unreachable!("handled by the caller"),
            Err(error) => state.backend = Backend::Failed(kind, error),
        }
        host.notify();
    }

    /// Notices when the forward is stopped from outside (Active Sessions' stop button, the
    /// cluster disconnecting).
    fn watch_forward(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let generation = state.generation;
        let Some(forward) = state.forward.as_mut() else {
            return;
        };
        let stopped = forward.take_stopped();
        let id = cluster.clone();
        state.watcher = Some(host.spawn(stopped, move |this, (), host| {
            this.forward_stopped(&id, generation, host)
        }));
    }

    /// The forward to Relay or Whisker was stopped from outside.
    fn forward_stopped(&mut self, cluster: &ClusterId, generation: u64, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if state.generation != generation {
            return;
        }
        // This is the watcher's own callback: it must not drop itself.
        if let Some(watcher) = state.watcher.take() {
            watcher.detach();
        }
        state.forward = None;
        let kind = match &state.backend {
            Backend::Ready { kind, .. } | Backend::Connecting(kind) => *kind,
            _ => return,
        };
        self.reset(cluster);
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.backend = Backend::Failed(
                kind,
                ProviderError::Unavailable(format!(
                    "The port-forward to {} was stopped in Active Sessions.",
                    kind.label()
                )),
            );
        }
        host.notify();
    }

    // ----- Streams -----

    fn start_stream(&mut self, cluster: &ClusterId, key: &str, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(provider) = state.provider() else {
            return;
        };
        let Some(stream) = state.streams.get_mut(key) else {
            return;
        };
        stream.tasks.clear();
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
        let (tx, rx) = mpsc::channel::<StreamEvent>(256);
        let (done_tx, done_rx) = oneshot::channel();
        let work = host.spawn(
            async move {
                let result = provider.stream(query, tx).await;
                done_tx.send(result).ok();
            },
            |_, (), _| {},
        );
        // The flows, then how the stream ended: the last batch sees both.
        let items = rx.map(Item::Event).chain(
            stream::once(done_rx).filter_map(|ended| async move { ended.ok().map(Item::Ended) }),
        );
        let id = cluster.clone();
        let key = key.to_string();
        // Batches go to the views at most once a frame.
        let drain = host.batches(items, Pace::throttle(FRAME), move |this, batch, host| {
            let mut events = Vec::new();
            let mut ended = None;
            for item in batch {
                match item {
                    Item::Event(event) => events.push(event),
                    Item::Ended(result) => ended = Some(result),
                }
            }
            if !events.is_empty() && !this.apply(&id, &key, generation, events, host) {
                return Flow::Stop;
            }
            match ended {
                Some(result) => {
                    this.stream_ended(&id, &key, generation, result, host);
                    Flow::Stop
                }
                None => Flow::Continue,
            }
        });
        stream.tasks.push(work);
        stream.tasks.push(drain);
    }

    fn apply(
        &mut self,
        cluster: &ClusterId,
        key: &str,
        generation: u64,
        events: Vec<StreamEvent>,
        host: &mut dyn Host<Self>,
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
        host.notify();
        true
    }

    fn stream_ended(
        &mut self,
        cluster: &ClusterId,
        key: &str,
        generation: u64,
        result: Result<(), ProviderError>,
        host: &mut dyn Host<Self>,
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
        // Its own handle: the drain that called this one must not drop itself.
        stream.retry = Some(host.after(backoff, move |this, host| {
            let current = this
                .clusters
                .get(&id)
                .and_then(|s| s.streams.get(&key))
                .map(|s| s.generation);
            if current == Some(generation) {
                this.start_stream(&id, &key, host);
            }
        }));
        host.notify();
    }

    // ----- Housekeeping -----

    /// Housekeeping at `now`: idle streams and clusters go, flows expire, the status and the
    /// metrics graphs refresh.
    pub fn tick_at(&mut self, now: Instant, host: &mut dyn Host<Self>) {
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
            let graphs = &state.graphs;
            state.graph_tasks.retain(|key, _| graphs.contains_key(key));
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
                self.reset(&id);
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
                let cluster = id.clone();
                state.status_task =
                    Some(host.spawn(provider.probe(), move |this, result, host| {
                        if let Ok(status) = result
                            && let Some(state) = this.clusters.get_mut(&cluster)
                            && state.generation == generation
                            && let Backend::Ready {
                                status: current, ..
                            } = &mut state.backend
                        {
                            *current = status;
                            host.notify();
                        }
                    }));
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
            self.fetch_graph(&id, &key, host);
        }
        // Rates and ages in the header.
        if changed || self.clusters.values().any(|s| !s.streams.is_empty()) {
            host.notify();
        }
    }

    /// A cluster connected, disconnected or learned what it serves.
    pub fn connection_changed(
        &mut self,
        cluster: &ClusterId,
        env: &Env,
        host: &mut dyn Host<Self>,
    ) {
        self.sync(env);
        if self.clusters.contains_key(cluster) {
            self.detect(cluster, host);
        }
        host.notify();
    }

    /// A cluster got another id (its kubeconfig entry changed).
    pub fn rekeyed(
        &mut self,
        from: &ClusterId,
        to: &ClusterId,
        env: &Env,
        host: &mut dyn Host<Self>,
    ) {
        if let Some(state) = self.clusters.remove(from) {
            // The backend holds the old connection's client: build it again. A state already
            // under the new id is replaced: dropping it stops its forward.
            self.clusters.insert(to.clone(), state);
            self.sync(env);
            self.reset(to);
            self.redetect(to, env, host);
        }
        host.notify();
    }

    /// The netflow settings (or a cluster's settings keys) changed.
    pub fn settings_changed(&mut self, env: &Env, host: &mut dyn Host<Self>) {
        self.sync(env);
        let settings = self.settings.clone();
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
            self.detect(&id, host);
        }
    }

    // ----- For views -----

    /// What a view shows for `cluster`; `connected`: the cluster has a client.
    pub fn flow_state(&self, cluster: &ClusterId, connected: bool) -> FlowState {
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
        host: &mut dyn Host<Self>,
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
            self.fetch_graph(cluster, &key, host);
        }
        Some(snapshot)
    }

    fn fetch_graph(&mut self, cluster: &ClusterId, key: &str, host: &mut dyn Host<Self>) {
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
        let id = cluster.clone();
        let graph_key = key.to_string();
        let task = host.spawn(
            provider.graph(window, zoom, filter),
            move |this, result, host| {
                let Some(state) = this.clusters.get_mut(&id) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                if let Some(graph) = state.graphs.get_mut(&graph_key) {
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
                host.notify();
            },
        );
        state.graph_tasks.insert(key.to_string(), task);
    }

    // ----- Seeding (tests of the service and of the views on it) -----

    /// Puts a ready backend (and the `forward` it came with) in place without detecting or
    /// connecting.
    pub fn seed_ready(
        &mut self,
        cluster: &ClusterId,
        provider: Arc<dyn FlowProvider>,
        detection: Detection,
        forward: Option<Reached>,
        host: &mut dyn Host<Self>,
    ) {
        let mut state = ClusterFlows::new(cluster);
        state.detect = Detect::Done(Arc::new(detection));
        state.backend = Backend::Ready {
            kind: provider.kind(),
            provider,
            status: BackendStatus::default(),
        };
        state.forward = forward;
        self.clusters.insert(cluster.clone(), state);
        self.watch_forward(cluster, host);
        host.notify();
    }

    /// Adds flows to a stream, as if its backend sent them.
    pub fn push_flows(
        &mut self,
        cluster: &ClusterId,
        filter: &FlowFilter,
        flows: Vec<crate::model::Flow>,
        host: &mut dyn Host<Self>,
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
                host,
            );
        }
    }

    /// Whether a cluster's forward and streams are gone.
    pub fn is_idle(&self, cluster: &ClusterId) -> bool {
        self.clusters
            .get(cluster)
            .is_none_or(|s| s.forward.is_none() && s.streams.is_empty())
    }
}

/// What the drain of a stream sees: flows, then how the stream ended.
enum Item {
    Event(StreamEvent),
    Ended(Result<(), ProviderError>),
}

/// What connecting built.
enum Built {
    Ready {
        provider: Arc<dyn FlowProvider>,
        status: BackendStatus,
        /// The forward the provider talks through; dropping it stops the forward.
        forward: Option<Reached>,
    },
    /// NetObserv's Loki is ready; its topology needs the cluster's Prometheus from the app.
    NeedsPrometheus(LokiAccess),
}

/// Whether the user may `verb` `resource` (`subresource`) in `namespace`. An access review that
/// fails isn't a denial: the call itself will tell.
async fn can(
    client: &kube::Client,
    verb: &'static str,
    resource: &'static str,
    subresource: Option<&'static str>,
    namespace: &str,
) -> bool {
    let mut query = AccessQuery::new(verb, &Gvr::new("", "v1", resource), Some(namespace));
    if let Some(sub) = subresource {
        query = query.subresource(sub);
    }
    access::check(client.clone(), query).await.unwrap_or(true)
}

/// Connects to a candidate: checks permissions, opens the forward (Hubble, Whisker), builds the
/// provider and probes it.
async fn connect(
    cluster: ClusterId,
    candidate: Candidate,
    client: Option<kube::Client>,
    keep_query_values: bool,
    reach: Arc<dyn Reach>,
) -> Result<Built, ProviderError> {
    let client =
        client.ok_or_else(|| ProviderError::Unavailable("The cluster isn't connected.".into()))?;
    match candidate {
        Candidate::Hubble {
            namespace,
            service: name,
            port,
            tls,
        } => {
            let tls = relay_tls(tls)?;
            if !can(&client, "create", "pods", Some("portforward"), &namespace).await {
                return Err(ProviderError::forbidden(
                    "create",
                    "pods/portforward",
                    Some(&namespace),
                ));
            }
            let forward = open_forward(&*reach, &cluster, &client, &namespace, &name, port).await?;
            let hubble = Hubble {
                target: HubbleTarget {
                    namespace,
                    service: name,
                    port,
                    tls,
                },
                local_port: forward.local_port,
                keep_query_values,
            };
            // A failed probe drops the forward with it.
            let status = hubble.probe().await?;
            Ok(Built::Ready {
                provider: Arc::new(hubble),
                status,
                forward: Some(forward),
            })
        }
        Candidate::Whisker(target) => {
            // A forward, not the service proxy: Calico's own policy only lets Whisker be
            // reached from inside its pod (see backends::whisker).
            if !can(
                &client,
                "create",
                "pods",
                Some("portforward"),
                &target.namespace,
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
            let forward = open_forward(
                &*reach,
                &cluster,
                &client,
                &target.namespace,
                &target.service,
                port,
            )
            .await?;
            let whisker = Whisker {
                client,
                target,
                local_port: forward.local_port,
            };
            let status = whisker.probe().await?;
            Ok(Built::Ready {
                provider: Arc::new(whisker),
                status,
                forward: Some(forward),
            })
        }
        Candidate::NetObserv { loki, prometheus } => {
            let loki = match loki {
                LokiTarget::Service(target) => {
                    if can(&client, "get", "services", Some("proxy"), &target.namespace).await {
                        NetObserv::loki_target(client.clone(), &target)
                    } else {
                        LokiAccess::Missing(format!(
                            "No single flows: reading NetObserv's Loki ({}) needs get services/proxy in {}.",
                            target.label(),
                            target.namespace
                        ))
                    }
                }
                LokiTarget::Url(url) => {
                    match kubyl_metrics_core::transport::Transport::external(&url, None, &Default::default()) {
                        Ok(transport) => LokiAccess::Ready {
                            transport,
                            label: format!("Loki {url}"),
                        },
                        Err(err) => LokiAccess::Missing(format!("NetObserv's Loki URL {url}: {err}")),
                    }
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
                PromTarget::Service(target) => Some(PromClient::new(
                    client.clone(),
                    Target::Service {
                        namespace: target.namespace,
                        service: target.service,
                        port: target.port,
                        scheme: target.scheme,
                        path: target.path,
                    },
                )),
                // The app knows the cluster's Prometheus.
                PromTarget::Cluster => return Ok(Built::NeedsPrometheus(loki)),
            };
            let netobserv = NetObserv { loki, prometheus };
            let status = netobserv.probe().await?;
            Ok(Built::Ready {
                provider: Arc::new(netobserv),
                status,
                forward: None,
            })
        }
    }
}

/// Opens the temporary forward to Relay or Whisker (shown in Active Sessions) and waits until
/// it listens.
async fn open_forward(
    reach: &dyn Reach,
    cluster: &ClusterId,
    client: &kube::Client,
    namespace: &str,
    service: &str,
    port: u16,
) -> Result<Reached, ProviderError> {
    reach
        .reach(ReachRequest {
            client: client.clone(),
            cluster: cluster.clone(),
            namespace: namespace.to_string(),
            service: service.to_string(),
            port: ReachPort::Number(port),
            title: format!("Network flows · svc/{service}"),
            https: false,
            timeout: FORWARD_TIMEOUT,
            reconnect_grace: Some(RECONNECT_GRACE),
        })
        .await
        .map_err(|error| reach_error(&error, namespace, service))
}

/// Why a forward could not be opened, in the words the view shows.
fn reach_error(error: &str, namespace: &str, service: &str) -> ProviderError {
    if error.to_lowercase().contains("forbidden") {
        // A target that doesn't resolve because a read was denied.
        ProviderError::forbidden("get", "pods", Some(namespace))
    } else if error.starts_with("the port-forward didn't start") {
        ProviderError::Unavailable(format!(
            "The port-forward to {namespace}/{service} didn't start in {} s.",
            FORWARD_TIMEOUT.as_secs()
        ))
    } else if error == "the port-forward stopped" {
        ProviderError::Unavailable("The port-forward stopped.".into())
    } else {
        let error = error.strip_prefix("port-forward: ").unwrap_or(error);
        ProviderError::Unavailable(format!(
            "The port-forward to {namespace}/{service} failed: {error}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;
    use kubyl_base::host::TestHost;
    use kubyl_portforward_core::reach::FakeReached;

    use super::*;
    use crate::model::Flow as FlowRecord;
    use crate::provider::{Capabilities, FlowSink, ProviderFuture};

    /// Streams one flow and the end of its history, then waits.
    struct Streams;

    impl FlowProvider for Streams {
        fn kind(&self) -> BackendKind {
            BackendKind::Hubble
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }

        fn pushdown(&self, _: &FlowFilter) -> FlowFilter {
            FlowFilter::default()
        }

        fn probe(&self) -> ProviderFuture<Result<BackendStatus, ProviderError>> {
            Box::pin(async { Ok(BackendStatus::default()) })
        }

        fn stream(
            &self,
            _: StreamQuery,
            mut sink: FlowSink,
        ) -> ProviderFuture<Result<(), ProviderError>> {
            Box::pin(async move {
                use futures::SinkExt as _;
                sink.send(StreamEvent::Flows(vec![FlowRecord::new(Timestamp::now())]))
                    .await
                    .ok();
                sink.send(StreamEvent::CaughtUp).await.ok();
                futures::future::pending().await
            })
        }
    }

    struct Fixture {
        host: TestHost<FlowsCore>,
        core: FlowsCore,
        cluster: ClusterId,
        forward: FakeReached,
    }

    /// A cluster with a ready backend and the forward it came with.
    fn ready() -> Fixture {
        let cluster = ClusterId::new("c");
        let mut host = TestHost::new();
        let mut core = FlowsCore::new(true, Arc::new(Refuses));
        let (reached, forward) = Reached::fake(4000);
        core.seed_ready(
            &cluster,
            Arc::new(Streams),
            Detection::default(),
            Some(reached),
            &mut host,
        );
        Fixture {
            host,
            core,
            cluster,
            forward,
        }
    }

    /// No forwards in these tests.
    struct Refuses;

    impl Reach for Refuses {
        fn reach(
            &self,
            _: kubyl_portforward_core::reach::ReachRequest,
        ) -> futures::future::BoxFuture<'static, Result<Reached, String>> {
            Box::pin(async { Err("no forwards".to_string()) })
        }
    }

    fn env() -> Env {
        Env::default()
    }

    #[test]
    fn a_lease_streams_flows_into_the_buffer() {
        let mut f = ready();
        let filter = FlowFilter::default();

        let lease = f.core.lease(
            &f.cluster,
            &filter,
            Duration::from_secs(900),
            &env(),
            &mut f.host,
        );
        let cluster = f.cluster.clone();
        f.host.run_until(&mut f.core, |core, _| {
            core.stream(&cluster, &filter)
                .is_some_and(|s| s.buffer.len() == 1 && s.caught_up)
        });

        let stream = f.core.stream(&f.cluster, &filter).unwrap();
        assert_eq!(stream.status, StreamStatus::Live);
        drop(lease);
    }

    #[test]
    fn an_unwatched_cluster_stops_its_streams_and_its_forward() {
        let mut f = ready();
        let filter = FlowFilter::default();
        let lease = f.core.lease(
            &f.cluster,
            &filter,
            Duration::from_secs(900),
            &env(),
            &mut f.host,
        );
        let cluster = f.cluster.clone();
        f.host.run_until(&mut f.core, |core, _| {
            core.stream(&cluster, &filter).is_some_and(|s| s.caught_up)
        });
        assert!(!f.core.is_idle(&f.cluster));
        assert!(!f.forward.released());

        // Noticed, then gone after the grace period.
        drop(lease);
        let now = Instant::now();
        f.core.tick_at(now, &mut f.host);
        f.core
            .tick_at(now + GRACE + Duration::from_millis(1), &mut f.host);
        f.core
            .tick_at(now + 2 * GRACE + Duration::from_millis(2), &mut f.host);

        assert!(f.core.is_idle(&f.cluster));
        assert!(f.forward.released());
    }

    #[test]
    fn a_forward_stopped_from_outside_fails_the_backend() {
        let mut f = ready();

        f.forward.end();
        let cluster = f.cluster.clone();
        f.host.run_until(&mut f.core, |core, _| {
            matches!(core.flow_state(&cluster, true), FlowState::Failed { .. })
        });

        match f.core.flow_state(&f.cluster, true) {
            FlowState::Failed { kind, error, .. } => {
                assert_eq!(kind, BackendKind::Hubble);
                assert!(error.to_string().contains("stopped in Active Sessions"));
            }
            _ => panic!("expected a failure"),
        }
        assert!(f.core.is_idle(&f.cluster));
    }
}
