//! Watch caches: one [`ResourceStore`] per (cluster, GVR, namespace, selectors, mode).
//!
//! A store runs a kube `watcher` on the Tokio runtime (list + watch with bookmarks, relist on
//! 410, exponential backoff) and keeps the objects as JSON. Changes reach GPUI as batches: the
//! Tokio side collects events for one frame (16 ms) and sends them together, so thousands of pod
//! updates cause at most ~60 applies (and re-renders) per second.
//!
//! Stores are shared: [`ResourceStores::acquire`] returns a [`StoreHandle`] and every view that
//! asks for the same [`StoreKey`] gets the same store. When the last handle is dropped the watch
//! keeps running for a grace period ([`GRACE`]) so switching tabs back and forth doesn't relist.
//!
//! ```ignore
//! let pods = ResourceStores::acquire(cx, StoreKey::new(cluster, Gvr::new("", "v1", "pods"), Some("payments".into())));
//! cx.observe(pods.entity(), |this, store, cx| { /* rebuild rows */ }).detach();
//! for object in pods.read(cx).objects().values() { /* serde_json::Value */ }
//! ```

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::channel::mpsc::{self, UnboundedSender};
use futures::{Stream, StreamExt as _};
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kube::Resource;
use kube::api::{Api, DynamicObject};
use kube::core::ObjectMeta;
use kube::core::PartialObjectMeta;
use kube::discovery::ApiResource;
use kube::runtime::WatchStreamExt as _;
use kube::runtime::watcher::{self, Event};
use kubyl_core::{ClusterId, Gvr, spawn_kube};
use kubyl_kube::discovery::ApiResourceInfo;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use serde::Serialize;
use serde_json::Value;

/// How long a store keeps watching after its last handle is dropped.
pub const GRACE: Duration = Duration::from_secs(30);
/// Events are collected for this long before they are sent to the UI (≤ 60 batches/s).
pub const FRAME: Duration = Duration::from_millis(16);
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

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

/// One change sent from the watch task.
#[derive(Debug)]
pub(crate) enum Change {
    /// A (re)list finished: replaces every object.
    Reset(Vec<(ObjectKey, Arc<Value>)>),
    Upsert(ObjectKey, Arc<Value>),
    Delete(ObjectKey),
    Status(StoreStatus),
}

/// The objects of one watch, kept current by a background task.
pub struct ResourceStore {
    key: StoreKey,
    objects: HashMap<ObjectKey, Arc<Value>>,
    status: StoreStatus,
    generation: u64,
    running: bool,
    /// Set by [`Self::pause`]; reconnects don't restart the watch until [`Self::resume`].
    paused: bool,
    task: Option<Task<()>>,
}

impl ResourceStore {
    fn new(key: StoreKey) -> Self {
        Self {
            key,
            objects: HashMap::new(),
            status: StoreStatus::Waiting,
            generation: 0,
            running: false,
            paused: false,
            task: None,
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

    /// Whether the watch is running (listing or watching).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Stops the watch but keeps the last known objects, until [`Self::resume`].
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        self.paused = true;
        self.stop(StoreStatus::Paused, cx);
    }

    /// Restarts a watch stopped with [`Self::pause`].
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if !self.paused {
            return;
        }
        self.paused = false;
        self.status = StoreStatus::Waiting;
        self.generation += 1;
        cx.notify();
        self.ensure_running(cx);
    }

    pub(crate) fn apply(&mut self, changes: Vec<Change>, cx: &mut Context<Self>) {
        tracing::trace!(resource = %self.key.gvr, changes = changes.len(), "store batch");
        for change in changes {
            match change {
                Change::Reset(objects) => {
                    self.objects = objects.into_iter().collect();
                    self.status = StoreStatus::Ready;
                }
                Change::Upsert(key, object) => {
                    self.objects.insert(key, object);
                }
                Change::Delete(key) => {
                    self.objects.remove(&key);
                }
                Change::Status(status) => {
                    if status == StoreStatus::Forbidden || status == StoreStatus::Unsupported {
                        self.objects.clear();
                        self.running = false;
                    }
                    self.status = status;
                }
            }
        }
        self.generation += 1;
        cx.notify();
    }

    fn stop(&mut self, status: StoreStatus, cx: &mut Context<Self>) {
        self.task = None;
        self.running = false;
        if self.status != status {
            self.status = status;
            self.generation += 1;
            cx.notify();
        }
    }

    /// Starts watching when the cluster is connected and serves the resource.
    fn ensure_running(&mut self, cx: &mut Context<Self>) {
        if self.paused {
            return;
        }
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let (client, discovery) = {
            let manager = manager.read(cx);
            (
                manager.client(&self.key.cluster),
                manager.discovery(&self.key.cluster),
            )
        };
        let Some(client) = client else {
            if self.running {
                self.stop(StoreStatus::Waiting, cx);
            }
            return;
        };
        if self.running {
            return;
        }
        let Some(discovery) = discovery else {
            return;
        };
        let Some(info) = find_resource(&discovery.resources, &self.key.gvr) else {
            self.stop(StoreStatus::Unsupported, cx);
            return;
        };
        let resource = api_resource(info);
        let namespace = self.key.namespace.clone().filter(|_| info.namespaced);
        let config = watcher::Config {
            label_selector: self.key.label_selector.clone(),
            field_selector: self.key.field_selector.clone(),
            ..Default::default()
        };

        let (tx, mut rx) = mpsc::unbounded();
        let mode = self.key.mode;
        let watch = spawn_kube(cx, async move {
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
        });
        self.running = true;
        self.status = StoreStatus::Loading;
        self.generation += 1;
        self.task = Some(cx.spawn(async move |this, cx| {
            // Dropping `watch` aborts the Tokio task.
            let _watch = watch;
            while let Some(changes) = rx.next().await {
                if this.update(cx, |this, cx| this.apply(changes, cx)).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
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
pub(crate) fn to_json<K: Serialize>(object: &K) -> Option<Value> {
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

struct Entry {
    store: Entity<ResourceStore>,
    lease: Rc<()>,
    idle_since: Option<Instant>,
}

/// One registered watch, for the Active Sessions panel.
#[derive(Clone)]
pub struct WatchInfo {
    pub key: StoreKey,
    pub store: Entity<ResourceStore>,
    /// Views holding a [`StoreHandle`]; `0` = idle, stopped after the grace period.
    pub users: usize,
}

/// Shared stores, ref-counted by [`StoreHandle`]s.
#[derive(Default)]
pub struct ResourceStores {
    entries: HashMap<StoreKey, Entry>,
    sweeping: bool,
}

impl Global for ResourceStores {}

/// Keeps a store alive. Clone it freely; the store stops [`GRACE`] after the last clone drops.
#[derive(Clone)]
pub struct StoreHandle {
    store: Entity<ResourceStore>,
    _lease: Rc<()>,
}

impl StoreHandle {
    pub fn entity(&self) -> &Entity<ResourceStore> {
        &self.store
    }

    pub fn read<'a>(&self, cx: &'a App) -> &'a ResourceStore {
        self.store.read(cx)
    }

    /// A handle to a store that isn't registered (tests, previews).
    pub fn detached(store: Entity<ResourceStore>) -> Self {
        Self {
            store,
            _lease: Rc::new(()),
        }
    }
}

impl ResourceStores {
    /// The shared store for `key`, created (and connected) on first use.
    pub fn acquire(cx: &mut App, key: StoreKey) -> StoreHandle {
        if let Some(entry) = cx.default_global::<Self>().entries.get_mut(&key) {
            entry.idle_since = None;
            return StoreHandle {
                store: entry.store.clone(),
                _lease: entry.lease.clone(),
            };
        }
        let cluster = key.cluster.clone();
        let store = cx.new(|cx| {
            let mut store = ResourceStore::new(key.clone());
            store.ensure_running(cx);
            store
        });
        let lease = Rc::new(());
        cx.global_mut::<Self>().entries.insert(
            key,
            Entry {
                store: store.clone(),
                lease: lease.clone(),
                idle_since: None,
            },
        );
        if let Some(manager) = ConnectionManager::try_global(cx) {
            manager.update(cx, |m, cx| m.ensure_connected(&cluster, cx));
        }
        Self::schedule_sweep(cx);
        StoreHandle {
            store,
            _lease: lease,
        }
    }

    /// An existing store for `key`, without creating one.
    pub fn peek(cx: &App, key: &StoreKey) -> Option<Entity<ResourceStore>> {
        cx.try_global::<Self>()?
            .entries
            .get(key)
            .map(|e| e.store.clone())
    }

    /// Every registered store (running or in its grace period), e.g. to search loaded objects.
    pub fn all(cx: &App) -> Vec<Entity<ResourceStore>> {
        cx.try_global::<Self>().map_or_else(Vec::new, |s| {
            s.entries.values().map(|e| e.store.clone()).collect()
        })
    }

    /// Every registered watch with its number of users, ordered by resource and namespace.
    pub fn watches(cx: &App) -> Vec<WatchInfo> {
        let mut watches: Vec<WatchInfo> = cx.try_global::<Self>().map_or_else(Vec::new, |s| {
            s.entries
                .iter()
                .map(|(key, entry)| WatchInfo {
                    key: key.clone(),
                    store: entry.store.clone(),
                    users: Rc::strong_count(&entry.lease).saturating_sub(1),
                })
                .collect()
        });
        watches.sort_by(|a, b| {
            (&a.key.cluster, &a.key.gvr.resource, &a.key.namespace).cmp(&(
                &b.key.cluster,
                &b.key.gvr.resource,
                &b.key.namespace,
            ))
        });
        watches
    }

    /// Number of running watches (status bar).
    pub fn watch_count(cx: &App) -> usize {
        cx.try_global::<Self>().map_or(0, |s| {
            s.entries
                .values()
                .filter(|e| e.store.read(cx).running)
                .count()
        })
    }

    fn schedule_sweep(cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if std::mem::replace(&mut this.sweeping, true) {
            return;
        }
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(SWEEP_INTERVAL).await;
                let more = cx.update(|cx| Self::sweep(Instant::now(), cx));
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// Drops stores that have been unused for [`GRACE`]. Returns whether stores remain.
    fn sweep(now: Instant, cx: &mut App) -> bool {
        let this = cx.global_mut::<Self>();
        let mut expired = Vec::new();
        for (key, entry) in &mut this.entries {
            if Rc::strong_count(&entry.lease) > 1 {
                entry.idle_since = None;
                continue;
            }
            let since = *entry.idle_since.get_or_insert(now);
            if now.duration_since(since) >= GRACE {
                expired.push(key.clone());
            }
        }
        // Dropping the entity drops its task, which aborts the watch.
        for key in expired {
            this.entries.remove(&key);
        }
        let more = !this.entries.is_empty();
        this.sweeping = more;
        more
    }

    fn connection_changed(event: &ConnectionEvent, cx: &mut App) {
        let cluster = match event {
            ConnectionEvent::StateChanged(id) | ConnectionEvent::DiscoveryChanged(id) => id,
            _ => return,
        };
        let stores: Vec<_> = cx
            .try_global::<Self>()
            .map(|s| {
                s.entries
                    .iter()
                    .filter(|(key, _)| &key.cluster == cluster)
                    .map(|(_, e)| e.store.clone())
                    .collect()
            })
            .unwrap_or_default();
        let rediscovered = matches!(event, ConnectionEvent::DiscoveryChanged(_));
        for store in stores {
            store.update(cx, |store, cx| {
                // A CRD may have been installed or removed: retry unsupported stores.
                if rediscovered && store.status == StoreStatus::Unsupported {
                    store.running = false;
                }
                store.ensure_running(cx)
            });
        }
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<ResourceStores>();
    if let Some(manager) = ConnectionManager::try_global(cx) {
        cx.subscribe(&manager, |_, event: &ConnectionEvent, cx| {
            ResourceStores::connection_changed(event, cx)
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(ns: &str, name: &str) -> Value {
        json!({"metadata": {"namespace": ns, "name": name}})
    }

    #[gpui::test]
    fn applies_batches(cx: &mut gpui::TestAppContext) {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        let store = cx.new(|_| ResourceStore::new(key));
        store.update(cx, |store, cx| {
            store.apply(
                vec![Change::Reset(vec![
                    (key_of(&pod("a", "one")), Arc::new(pod("a", "one"))),
                    (key_of(&pod("a", "two")), Arc::new(pod("a", "two"))),
                ])],
                cx,
            );
            assert_eq!(store.len(), 2);
            assert!(store.status().is_ready());
            store.apply(
                vec![
                    Change::Delete("a/one".into()),
                    Change::Upsert("b/three".into(), Arc::new(pod("b", "three"))),
                ],
                cx,
            );
            assert!(store.get("a/one").is_none());
            assert!(store.get("b/three").is_some());
            assert_eq!(store.generation(), 2);
            store.apply(vec![Change::Status(StoreStatus::Forbidden)], cx);
            assert!(store.is_empty());
        });
    }

    #[gpui::test]
    fn unused_stores_expire_after_the_grace_period(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.default_global::<ResourceStores>();
            let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
            let handle = ResourceStores::acquire(cx, key.clone());
            let again = ResourceStores::acquire(cx, key.clone());
            assert_eq!(handle.entity(), again.entity());
            drop((handle, again));

            let start = Instant::now();
            assert!(ResourceStores::sweep(start, cx));
            assert!(ResourceStores::peek(cx, &key).is_some());
            assert!(!ResourceStores::sweep(start + GRACE, cx));
            assert!(ResourceStores::peek(cx, &key).is_none());
        });
    }

    #[gpui::test]
    fn pausing_keeps_objects_until_resumed(cx: &mut gpui::TestAppContext) {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        let store = cx.new(|_| ResourceStore::from_objects(key, [pod("a", "one")]));
        store.update(cx, |store, cx| {
            store.pause(cx);
            assert_eq!(store.status(), &StoreStatus::Paused);
            assert!(!store.is_running());
            assert_eq!(store.len(), 1);
            store.resume(cx);
            // No connection manager in tests: the store waits for the cluster.
            assert_eq!(store.status(), &StoreStatus::Waiting);
        });
    }

    #[gpui::test]
    fn watches_report_their_users(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.default_global::<ResourceStores>();
            let pods = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
            let nodes = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "nodes"), None);
            let first = ResourceStores::acquire(cx, pods.clone());
            let _second = first.clone();
            let _nodes = ResourceStores::acquire(cx, nodes);
            let watches = ResourceStores::watches(cx);
            assert_eq!(watches.len(), 2);
            assert_eq!(watches[0].key.gvr.resource, "nodes");
            assert_eq!(watches[0].users, 1);
            assert_eq!(watches[1].key, pods);
            assert_eq!(watches[1].users, 2);
        });
    }

    #[test]
    fn strips_managed_fields() {
        let object = json!({"metadata": {"name": "x", "managedFields": [{}]}, "spec": {}});
        let value = to_json(&object).unwrap();
        assert!(value["metadata"].get("managedFields").is_none());
        assert_eq!(key_of(&value).as_ref(), "x");
    }
}
