//! Watch caches without the UI: [`StoreCore`] holds the objects of one (cluster, GVR, namespace,
//! selectors, mode) watch and keeps them current.
//!
//! The watch runs on the Tokio runtime (list + watch with bookmarks, relist on 410, exponential
//! backoff) and keeps the objects as JSON. Changes arrive as batches: the Tokio side collects
//! events for one frame (16 ms) and sends them together, so thousands of pod updates cause at
//! most ~60 applies (and re-renders) per second. `kubyl_resources` shares stores between views
//! (`ResourceStores`) and keeps each in an entity.

use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{self, UnboundedSender};
use futures::{Stream, StreamExt as _};
use kube::Resource;
use kube::api::{Api, DynamicObject};
use kube::core::ObjectMeta;
use kube::core::PartialObjectMeta;
use kube::discovery::ApiResource;
use kube::runtime::WatchStreamExt as _;
use kube::runtime::watcher::{self, Event};
use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::{ClusterId, Gvr};
use kubyl_kube_core::discovery::{ApiResourceInfo, Discovery};
use serde::Serialize;
use serde_json::Value;

/// How long a store keeps watching after its last handle is dropped.
pub const GRACE: Duration = Duration::from_secs(30);
/// Events are collected for this long before they are sent to the UI (≤ 60 batches/s).
pub const FRAME: Duration = Duration::from_millis(16);

/// `namespace/name`, or `name` for cluster-scoped objects.
pub type ObjectKey = Arc<str>;

/// Builds the key of an object.
pub fn object_key(namespace: Option<&str>, name: &str) -> ObjectKey {
    match namespace {
        Some(ns) if !ns.is_empty() => format!("{ns}/{name}").into(),
        _ => name.into(),
    }
}

/// The key of a JSON object (`metadata.namespace` / `metadata.name`).
pub fn key_of(object: &Value) -> ObjectKey {
    let meta = &object["metadata"];
    object_key(
        meta["namespace"].as_str(),
        meta["name"].as_str().unwrap_or_default(),
    )
}

/// What a store holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoreMode {
    /// Whole objects (minus `metadata.managedFields`).
    Full,
    /// Only `apiVersion`, `kind` and `metadata` (`PartialObjectMetadata`). Much cheaper for big
    /// lists that only need names, labels and counts.
    Metadata,
}

/// Identifies a store. Views asking for equal keys share one watch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StoreKey {
    pub cluster: ClusterId,
    pub gvr: Gvr,
    /// `None`: all namespaces (or a cluster-scoped kind).
    pub namespace: Option<String>,
    pub label_selector: Option<String>,
    pub field_selector: Option<String>,
    pub mode: StoreMode,
}

impl StoreKey {
    pub fn new(cluster: ClusterId, gvr: Gvr, namespace: Option<String>) -> Self {
        Self {
            cluster,
            gvr,
            namespace,
            label_selector: None,
            field_selector: None,
            mode: StoreMode::Full,
        }
    }

    pub fn metadata(mut self) -> Self {
        self.mode = StoreMode::Metadata;
        self
    }

    pub fn labels(mut self, selector: impl Into<String>) -> Self {
        self.label_selector = Some(selector.into()).filter(|s: &String| !s.is_empty());
        self
    }

    pub fn fields(mut self, selector: impl Into<String>) -> Self {
        self.field_selector = Some(selector.into()).filter(|s: &String| !s.is_empty());
        self
    }
}

/// Where a store is in its lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreStatus {
    /// The cluster isn't connected (or discovery hasn't finished) yet.
    Waiting,
    /// Initial list in progress.
    Loading,
    /// Listed and watching.
    Ready,
    /// The user may not list this resource here.
    Forbidden,
    /// The cluster doesn't serve this resource (anymore).
    Unsupported,
    /// The watch failed and is retrying with backoff.
    Error(String),
    /// The user paused the watch (Active Sessions panel); the objects are the last known state.
    Paused,
}

impl StoreStatus {
    pub fn is_ready(&self) -> bool {
        matches!(self, StoreStatus::Ready)
    }

    /// Whether the objects reflect the cluster (ready, or an error after a successful list).
    pub fn is_settled(&self) -> bool {
        !matches!(self, StoreStatus::Waiting | StoreStatus::Loading)
    }
}

/// How many applied batches (and keys in them) a store remembers for
/// [`StoreCore::changes_since`].
const HISTORY_BATCHES: usize = 256;
const HISTORY_KEYS: usize = 20_000;

/// The objects applied batches touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Touched {
    /// Only these objects (changed, added or removed).
    Keys(HashSet<ObjectKey>),
    /// Anything: the store was listed again or its status changed.
    All,
}

/// One change sent from the watch task.
#[derive(Debug)]
pub enum Change {
    /// A (re)list finished: replaces every object.
    Reset(Vec<(ObjectKey, Arc<Value>)>),
    Upsert(ObjectKey, Arc<Value>),
    Delete(ObjectKey),
    Status(StoreStatus),
}

/// The objects of one watch, kept current by a background task.
pub struct StoreCore {
    key: StoreKey,
    objects: HashMap<ObjectKey, Arc<Value>>,
    status: StoreStatus,
    generation: u64,
    /// What the last applied batches touched, newest last (see [`StoreCore::changes_since`]).
    history: VecDeque<(u64, Touched)>,
    running: bool,
    /// Set by [`Self::pause`]; reconnects don't restart the watch until [`Self::resume`].
    paused: bool,
    /// The watch on Tokio and the task applying its batches. Dropping them stops the watch.
    tasks: Vec<TaskHandle>,
}

impl Service for StoreCore {
    type Event = Infallible;
    type Effect = Infallible;
}

/// What a store needs from its cluster's connection to watch.
pub struct Connection {
    pub client: Option<kube::Client>,
    pub discovery: Option<Arc<Discovery>>,
}

impl StoreCore {
    pub fn new(key: StoreKey) -> Self {
        Self {
            key,
            objects: HashMap::new(),
            status: StoreStatus::Waiting,
            generation: 0,
            history: VecDeque::new(),
            running: false,
            paused: false,
            tasks: Vec::new(),
        }
    }

    /// A store filled by hand, for tests and previews. It never watches.
    pub fn from_objects(key: StoreKey, objects: impl IntoIterator<Item = Value>) -> Self {
        let mut store = Self::new(key);
        store.objects = objects
            .into_iter()
            .map(|o| (key_of(&o), Arc::new(o)))
            .collect();
        store.status = StoreStatus::Ready;
        store
    }

    pub fn key(&self) -> &StoreKey {
        &self.key
    }

    pub(crate) fn set_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.history.clear();
    }

    pub fn objects(&self) -> &HashMap<ObjectKey, Arc<Value>> {
        &self.objects
    }

    pub fn get(&self, key: &str) -> Option<&Arc<Value>> {
        self.objects.get(key)
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn status(&self) -> &StoreStatus {
        &self.status
    }

    /// Bumped on every applied batch; cheap change detection for observers.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What changed since `generation` (an earlier value of [`Self::generation`]): the keys of
    /// the objects batches added, changed or removed, or [`Touched::All`] when anything may
    /// have (a relist, a status change). `None` when `generation` is older than the batches
    /// the store still remembers: compare everything.
    pub fn changes_since(&self, generation: u64) -> Option<Touched> {
        if generation >= self.generation {
            return Some(Touched::Keys(HashSet::new()));
        }
        let first = self.history.front()?.0;
        // The batch that produced `first` is in the history; the one before it isn't.
        if generation + 1 < first {
            return None;
        }
        let mut keys = HashSet::new();
        for (after, touched) in &self.history {
            if *after <= generation {
                continue;
            }
            match touched {
                Touched::All => return Some(Touched::All),
                Touched::Keys(batch) => keys.extend(batch.iter().cloned()),
            }
        }
        Some(Touched::Keys(keys))
    }

    /// Moves to the next generation, remembering what it touched.
    fn bump(&mut self, touched: Touched) {
        self.generation += 1;
        self.history.push_back((self.generation, touched));
        let size = |touched: &Touched| match touched {
            Touched::All => 1,
            Touched::Keys(keys) => keys.len().max(1),
        };
        let mut total: usize = self.history.iter().map(|(_, t)| size(t)).sum();
        while self.history.len() > 1
            && (self.history.len() > HISTORY_BATCHES || total > HISTORY_KEYS)
        {
            if let Some((_, oldest)) = self.history.pop_front() {
                total -= size(&oldest);
            }
        }
    }

    /// Whether the watch is running (listing or watching).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Stops the watch but keeps the last known objects, until [`Self::resume`].
    pub fn pause(&mut self, host: &mut dyn Host<Self>) {
        self.paused = true;
        self.stop(StoreStatus::Paused, host);
    }

    /// Allows a watch stopped with [`Self::pause`] to run again. Returns whether it was
    /// paused; then call [`Self::ensure_running`].
    pub fn resume(&mut self, host: &mut dyn Host<Self>) -> bool {
        if !self.paused {
            return false;
        }
        self.paused = false;
        self.status = StoreStatus::Waiting;
        self.bump(Touched::All);
        host.notify();
        true
    }

    /// A CRD may have been installed or removed: an unsupported store tries again on the next
    /// [`Self::ensure_running`].
    pub fn retry_unsupported(&mut self) {
        if self.status == StoreStatus::Unsupported {
            self.running = false;
        }
    }

    pub fn apply(&mut self, changes: Vec<Change>, host: &mut dyn Host<Self>) {
        tracing::trace!(resource = %self.key.gvr, changes = changes.len(), "store batch");
        let mut touched = Touched::Keys(HashSet::new());
        for change in changes {
            match change {
                Change::Reset(objects) => {
                    self.objects = objects.into_iter().collect();
                    self.status = StoreStatus::Ready;
                    touched = Touched::All;
                }
                Change::Upsert(key, object) => {
                    if let Touched::Keys(keys) = &mut touched {
                        keys.insert(key.clone());
                    }
                    self.objects.insert(key, object);
                }
                Change::Delete(key) => {
                    if let Touched::Keys(keys) = &mut touched {
                        keys.insert(key.clone());
                    }
                    self.objects.remove(&key);
                }
                Change::Status(status) => {
                    touched = Touched::All;
                    if status == StoreStatus::Forbidden || status == StoreStatus::Unsupported {
                        self.objects.clear();
                        self.running = false;
                    }
                    self.status = status;
                }
            }
        }
        self.bump(touched);
        host.notify();
    }

    fn stop(&mut self, status: StoreStatus, host: &mut dyn Host<Self>) {
        self.tasks.clear();
        self.running = false;
        if self.status != status {
            self.status = status;
            self.bump(Touched::All);
            host.notify();
        }
    }

    /// Starts watching when the cluster is connected and serves the resource.
    pub fn ensure_running(&mut self, connection: Connection, host: &mut dyn Host<Self>) {
        if self.paused {
            return;
        }
        let Some(client) = connection.client else {
            if self.running {
                self.stop(StoreStatus::Waiting, host);
            }
            return;
        };
        if self.running {
            return;
        }
        let Some(discovery) = connection.discovery else {
            return;
        };
        let Some(info) = find_resource(&discovery.resources, &self.key.gvr) else {
            self.stop(StoreStatus::Unsupported, host);
            return;
        };
        let resource = api_resource(info);
        let namespace = self.key.namespace.clone().filter(|_| info.namespaced);
        let config = watcher::Config {
            label_selector: self.key.label_selector.clone(),
            field_selector: self.key.field_selector.clone(),
            ..Default::default()
        };

        let (tx, rx) = mpsc::unbounded();
        let mode = self.key.mode;
        let watch = host.spawn(
            async move {
                match mode {
                    StoreMode::Full => {
                        let api: Api<DynamicObject> = match &namespace {
                            Some(ns) => Api::namespaced_with(client, ns, &resource),
                            None => Api::all_with(client, &resource),
                        };
                        pump(watcher::watcher(api, config), tx).await
                    }
                    StoreMode::Metadata => {
                        // `Api<PartialObjectMeta<_>>` makes metadata-only requests.
                        let api: Api<PartialObjectMeta<DynamicObject>> = match &namespace {
                            Some(ns) => Api::namespaced_with(client, ns, &resource),
                            None => Api::all_with(client, &resource),
                        };
                        pump(watcher::watcher(api, config), tx).await
                    }
                }
            },
            |_, (), _| {},
        );
        let apply = host.batches(rx, Pace::IMMEDIATE, |this, batches, host| {
            for changes in batches {
                this.apply(changes, host);
            }
            Flow::Continue
        });
        self.tasks = vec![watch, apply];
        self.running = true;
        self.status = StoreStatus::Loading;
        self.bump(Touched::All);
        host.notify();
    }
}

/// The served resource for `gvr`: that exact version, else the group's preferred version.
pub fn find_resource<'a>(
    resources: &'a [ApiResourceInfo],
    gvr: &Gvr,
) -> Option<&'a ApiResourceInfo> {
    resources
        .iter()
        .find(|r| &r.gvr == gvr)
        .or_else(|| {
            resources
                .iter()
                .find(|r| r.preferred && r.gvr.group == gvr.group && r.gvr.resource == gvr.resource)
        })
        .filter(|r| r.is_listable())
}

/// The kube `ApiResource` for a discovered resource (for `Api::<DynamicObject>::*_with`).
pub fn api_resource(info: &ApiResourceInfo) -> ApiResource {
    ApiResource {
        group: info.gvk.group.clone(),
        version: info.gvk.version.clone(),
        api_version: info.gvk.api_version(),
        kind: info.gvk.kind.clone(),
        plural: info.gvr.resource.clone(),
    }
}

fn is_status(err: &watcher::Error, code: u16) -> bool {
    match err {
        watcher::Error::InitialListFailed(kube::Error::Api(status))
        | watcher::Error::WatchStartFailed(kube::Error::Api(status)) => status.code == code,
        watcher::Error::WatchError(status) => status.code == code,
        _ => false,
    }
}

/// Serializes an object for the store, dropping `managedFields` (large and never shown).
pub fn to_json<K: Serialize>(object: &K) -> Option<Value> {
    let mut value = serde_json::to_value(object).ok()?;
    if let Some(meta) = value.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
    }
    Some(value)
}

fn meta_key(meta: &ObjectMeta) -> ObjectKey {
    object_key(
        meta.namespace.as_deref(),
        meta.name.as_deref().unwrap_or_default(),
    )
}

/// Forwards watcher events as batches, one per [`FRAME`].
async fn pump<K, S>(stream: S, tx: UnboundedSender<Vec<Change>>)
where
    K: Resource + Serialize + Send + 'static,
    S: Stream<Item = Result<Event<K>, watcher::Error>> + Send,
{
    let mut stream = std::pin::pin!(stream.default_backoff());
    let mut pending: Vec<Change> = Vec::new();
    let mut initial: Vec<(ObjectKey, Arc<Value>)> = Vec::new();
    let mut failing = false;
    loop {
        let Some(first) = stream.next().await else {
            return;
        };
        let deadline = tokio::time::Instant::now() + FRAME;
        let mut next = Some(first);
        loop {
            if let Some(item) = next.take() {
                match item {
                    Ok(event) => {
                        if std::mem::take(&mut failing) {
                            pending.push(Change::Status(StoreStatus::Ready));
                        }
                        match event {
                            Event::Init => initial.clear(),
                            Event::InitApply(object) => {
                                if let Some(value) = to_json(&object) {
                                    initial.push((meta_key(object.meta()), Arc::new(value)));
                                }
                            }
                            Event::InitDone => {
                                // A relist replaces everything; earlier changes are moot.
                                pending.clear();
                                pending.push(Change::Reset(std::mem::take(&mut initial)));
                            }
                            Event::Apply(object) => {
                                if let Some(value) = to_json(&object) {
                                    pending.push(Change::Upsert(
                                        meta_key(object.meta()),
                                        Arc::new(value),
                                    ));
                                }
                            }
                            Event::Delete(object) => {
                                pending.push(Change::Delete(meta_key(object.meta())))
                            }
                        }
                    }
                    Err(err) if is_status(&err, 403) => {
                        pending.push(Change::Status(StoreStatus::Forbidden));
                        tx.unbounded_send(pending).ok();
                        return;
                    }
                    Err(err) if is_status(&err, 404) => {
                        pending.push(Change::Status(StoreStatus::Unsupported));
                        tx.unbounded_send(pending).ok();
                        return;
                    }
                    Err(err) => {
                        tracing::debug!("watch error: {err}");
                        if !failing {
                            failing = true;
                            pending.push(Change::Status(StoreStatus::Error(err.to_string())));
                        }
                    }
                }
            }
            tokio::select! {
                item = stream.next() => match item {
                    Some(item) => next = Some(item),
                    None => break,
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        if !pending.is_empty() && tx.unbounded_send(std::mem::take(&mut pending)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_base::host::TestHost;
    use serde_json::json;

    fn pod(ns: &str, name: &str) -> Value {
        json!({"metadata": {"namespace": ns, "name": name}})
    }

    #[test]
    fn batches_remember_which_objects_they_touched() {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        let mut host = TestHost::new();
        let mut store = StoreCore::new(key);
        let keys = |names: &[&str]| {
            Some(Touched::Keys(
                names.iter().map(|n| object_key(Some("a"), n)).collect(),
            ))
        };
        let upsert =
            |name: &str| Change::Upsert(object_key(Some("a"), name), Arc::new(pod("a", name)));
        store.apply(vec![Change::Reset(vec![])], &mut host);
        let listed = store.generation();

        store.apply(vec![upsert("one"), upsert("two")], &mut host);
        let first = store.generation();
        store.apply(
            vec![
                Change::Delete(object_key(Some("a"), "one")),
                upsert("three"),
            ],
            &mut host,
        );

        assert_eq!(store.changes_since(store.generation()), keys(&[]));
        assert_eq!(store.changes_since(first), keys(&["one", "three"]));
        assert_eq!(store.changes_since(listed), keys(&["one", "two", "three"]));
        // A relist (or a status change) may have changed anything.
        let before = store.generation();
        store.apply(vec![Change::Reset(vec![])], &mut host);
        assert_eq!(store.changes_since(before), Some(Touched::All));
        assert_eq!(store.changes_since(listed), Some(Touched::All));
    }

    #[test]
    fn only_recent_batches_are_remembered() {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        let mut host = TestHost::new();
        let mut store = StoreCore::new(key);
        let start = store.generation();
        for i in 0..(HISTORY_BATCHES + 10) {
            let name = format!("pod-{i}");
            store.apply(
                vec![Change::Upsert(
                    object_key(Some("a"), &name),
                    Arc::new(pod("a", &name)),
                )],
                &mut host,
            );
        }
        // Too old: compare everything.
        assert_eq!(store.changes_since(start), None);
        let recent = store.generation() - 5;
        let Some(Touched::Keys(keys)) = store.changes_since(recent) else {
            panic!("recent changes are remembered");
        };
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn applies_batches_without_a_ui() {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        let mut host = TestHost::new();
        let mut store = StoreCore::new(key);
        store.apply(
            vec![Change::Reset(vec![
                (key_of(&pod("a", "one")), Arc::new(pod("a", "one"))),
                (key_of(&pod("a", "two")), Arc::new(pod("a", "two"))),
            ])],
            &mut host,
        );
        assert_eq!(store.len(), 2);
        assert!(store.status().is_ready());
        store.apply(vec![Change::Status(StoreStatus::Forbidden)], &mut host);
        assert!(store.is_empty());
        assert_eq!(host.notified, 2);
    }

    #[test]
    fn waits_without_a_client() {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        let mut host = TestHost::new();
        let mut store = StoreCore::new(key);
        let connection = Connection {
            client: None,
            discovery: None,
        };
        store.ensure_running(connection, &mut host);
        assert_eq!(store.status(), &StoreStatus::Waiting);
        assert!(!store.is_running());
    }

    #[test]
    fn strips_managed_fields() {
        let object = json!({"metadata": {"name": "x", "managedFields": [{}]}, "spec": {}});
        let value = to_json(&object).unwrap();
        assert!(value["metadata"].get("managedFields").is_none());
        assert_eq!(key_of(&value).as_ref(), "x");
    }
}
