//! Shared watch caches: one [`ResourceStore`] entity per [`StoreKey`], around a
//! [`StoreCore`](kubyl_resources_core::store::StoreCore) from `kubyl_resources_core`.
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
use std::convert::Infallible;
use std::ops::Deref;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources_core::source::{StoreLease, StoreReader, StoreSource, StoreView};
pub use kubyl_resources_core::store::{
    Change, FRAME, GRACE, ObjectKey, StoreKey, StoreMode, StoreStatus, api_resource, find_resource,
    key_of, object_key, to_json,
};
use kubyl_resources_core::store::{Connection, StoreCore};
use serde_json::Value;

const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// The objects of one watch, kept current by a background task. Reads go to the
/// [`StoreCore`]; observe the entity to re-render on changes.
pub struct ResourceStore {
    core: StoreCore,
}

impl Deref for ResourceStore {
    type Target = StoreCore;

    fn deref(&self) -> &StoreCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for ResourceStore {}

impl Hosts<StoreCore> for ResourceStore {
    fn service(&mut self) -> &mut StoreCore {
        &mut self.core
    }

    fn apply(&mut self, effect: Infallible, _cx: &mut Context<Self>) {
        match effect {}
    }
}

impl ResourceStore {
    fn new(key: StoreKey) -> Self {
        Self {
            core: StoreCore::new(key),
        }
    }

    /// A store filled by hand, for tests and previews. It never watches.
    pub fn from_objects(key: StoreKey, objects: impl IntoIterator<Item = Value>) -> Self {
        Self {
            core: StoreCore::from_objects(key, objects),
        }
    }

    /// Stops the watch but keeps the last known objects, until [`Self::resume`].
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.pause(host));
    }

    /// Restarts a watch stopped with [`Self::pause`].
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if hosted(self, cx, |core, host| core.resume(host)) {
            self.ensure_running(cx);
        }
    }

    pub fn apply(&mut self, changes: Vec<Change>, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.apply(changes, host));
    }

    /// Starts watching when the cluster is connected and serves the resource.
    fn ensure_running(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let connection = {
            let manager = manager.read(cx);
            Connection {
                client: manager.client(&self.key().cluster),
                discovery: manager.discovery(&self.key().cluster),
            }
        };
        hosted(self, cx, |core, host| core.ensure_running(connection, host));
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

    /// Registers a store filled by hand (tests, previews) under `key`, replacing the one there.
    /// It never watches; handles to the replaced store keep it alive.
    pub fn insert(cx: &mut App, key: StoreKey, store: Entity<ResourceStore>) -> StoreHandle {
        let lease = Rc::new(());
        cx.default_global::<Self>().entries.insert(
            key,
            Entry {
                store: store.clone(),
                lease: lease.clone(),
                idle_since: None,
            },
        );
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
                .filter(|e| e.store.read(cx).is_running())
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
                if rediscovered {
                    store.core.retry_unsupported();
                }
                store.ensure_running(cx)
            });
        }
    }
}

/// The desktop's [`StoreSource`]: [`ResourceStores`] for core services. Make one around the
/// `App` for the duration of a call into a service; the service's leases outlive it.
pub struct AppStores<'a>(pub &'a mut App);

impl StoreSource for AppStores<'_> {
    fn acquire(&mut self, key: StoreKey) -> StoreLease {
        let handle = ResourceStores::acquire(self.0, key.clone());
        StoreLease::new(key, Rc::new(handle))
    }

    fn view(&self, key: &StoreKey) -> Option<&dyn StoreView> {
        let store = ResourceStores::peek(self.0, key)?;
        let core: &StoreCore = store.read(self.0);
        Some(core as &dyn StoreView)
    }

    fn keys(&self) -> Vec<StoreKey> {
        ResourceStores::all(self.0)
            .iter()
            .map(|store| store.read(self.0).key().clone())
            .collect()
    }
}

/// A read-only [`StoreReader`] over `ResourceStores`, for answering reads with a shared `&App`.
pub struct AppStoresRef<'a>(pub &'a App);

impl StoreReader for AppStoresRef<'_> {
    fn read(&self, key: &StoreKey) -> Option<&dyn StoreView> {
        let store = ResourceStores::peek(self.0, key)?;
        let core: &StoreCore = store.read(self.0);
        Some(core as &dyn StoreView)
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
    use kubyl_core::{ClusterId, Gvr};
    use serde_json::json;
    use std::sync::Arc;

    fn pod(ns: &str, name: &str) -> Value {
        json!({"metadata": {"namespace": ns, "name": name}})
    }

    #[gpui::test]
    fn core_services_acquire_and_read_stores_through_the_app(cx: &mut gpui::TestAppContext) {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        cx.update(|cx| {
            let mut stores = AppStores(cx);
            let lease = stores.acquire(key.clone());
            let view = stores.view(lease.key()).expect("the store exists");
            assert_eq!(view.status(), &StoreStatus::Waiting);
            assert!(view.objects().is_empty());
            assert_eq!(stores.keys(), std::slice::from_ref(&key));
            assert_eq!(ResourceStores::watches(cx)[0].users, 1);
            drop(lease);
            assert_eq!(ResourceStores::watches(cx)[0].users, 0);
        });
    }

    #[gpui::test]
    fn hand_filled_stores_are_read_through_the_app(cx: &mut gpui::TestAppContext) {
        let key = StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "pods"), None);
        cx.update(|cx| {
            let store = cx.new(|_| ResourceStore::from_objects(key.clone(), [pod("a", "one")]));
            let handle = ResourceStores::insert(cx, key.clone(), store);
            let reader = AppStoresRef(cx);
            let view = reader.read(&key).expect("the store is registered");
            assert!(view.status().is_ready());
            assert!(view.get("a/one").is_some());
            assert_eq!(ResourceStores::watches(cx)[0].users, 1);
            drop(handle);
            assert_eq!(ResourceStores::watches(cx)[0].users, 0);
        });
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
}
