//! Shared watch caches as a service sees them: acquire a store, read its objects, and be told
//! when they changed.
//!
//! A service that needs a few resources (OLM's Subscriptions, Argo CD's Applications, Helm's
//! release Secrets) doesn't own watches. It asks a [`StoreSource`] for a [`StoreLease`] per
//! [`StoreKey`] and reads the objects through [`StoreSource::view`]; the lease keeps the store
//! alive (a watch stops a while after its last lease is dropped). The app that owns the source
//! calls the service when a leased store changed, as it does for any other input:
//!
//! - the desktop's `ResourceStores` (`kubyl_resources::store::AppStores`) observes its store
//!   entities;
//! - [`MemoryStores`] is a source for tests, filled by hand;
//! - another host brings its own source around its [`StoreCore`]s.
//!
//! The source and its leases belong to the host's thread, like the services.

use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::Value;

use crate::store::{ObjectKey, StoreCore, StoreKey, StoreStatus};

/// What a service reads from a watch cache.
pub trait StoreView {
    fn key(&self) -> &StoreKey;
    fn status(&self) -> &StoreStatus;
    /// Bumped on every applied batch; cheap change detection.
    fn generation(&self) -> u64;
    fn objects(&self) -> &HashMap<ObjectKey, Arc<Value>>;

    fn get(&self, key: &str) -> Option<&Arc<Value>> {
        self.objects().get(key)
    }
}

impl StoreView for StoreCore {
    fn key(&self) -> &StoreKey {
        StoreCore::key(self)
    }

    fn status(&self) -> &StoreStatus {
        StoreCore::status(self)
    }

    fn generation(&self) -> u64 {
        StoreCore::generation(self)
    }

    fn objects(&self) -> &HashMap<ObjectKey, Arc<Value>> {
        StoreCore::objects(self)
    }
}

/// Keeps a store alive while any clone exists.
#[derive(Clone)]
pub struct StoreLease {
    key: StoreKey,
    _hold: Rc<dyn Any>,
}

impl StoreLease {
    /// A lease on `key`; `hold` is whatever keeps the source's store alive.
    pub fn new(key: StoreKey, hold: Rc<dyn Any>) -> Self {
        Self { key, _hold: hold }
    }

    pub fn key(&self) -> &StoreKey {
        &self.key
    }
}

/// Where a service gets its watch caches from.
pub trait StoreSource {
    /// The shared store for `key`, created (and connected) on first use.
    fn acquire(&mut self, key: StoreKey) -> StoreLease;

    /// The store a lease (or any key someone holds) refers to, if it exists.
    fn view(&self, key: &StoreKey) -> Option<&dyn StoreView>;

    /// The keys of every store that exists, whoever holds it (to search loaded objects).
    fn keys(&self) -> Vec<StoreKey>;
}

/// Read access to the stores a source holds, for code that only has a shared borrow of the app
/// (a service answering a read from a view). Every [`StoreSource`] is a reader.
pub trait StoreReader {
    /// See [`StoreSource::view`].
    fn read(&self, key: &StoreKey) -> Option<&dyn StoreView>;
}

impl<T: StoreSource + ?Sized> StoreReader for T {
    fn read(&self, key: &StoreKey) -> Option<&dyn StoreView> {
        StoreSource::view(self, key)
    }
}

/// Copies of some stores' contents, for passing what a service reads into a call that can't
/// reach the app (the objects are shared, so a copy is a map of `Arc`s).
#[derive(Default)]
pub struct StoreCopies(HashMap<StoreKey, StoreCopy>);

struct StoreCopy {
    key: StoreKey,
    status: StoreStatus,
    generation: u64,
    objects: HashMap<ObjectKey, Arc<Value>>,
}

impl StoreView for StoreCopy {
    fn key(&self) -> &StoreKey {
        &self.key
    }

    fn status(&self) -> &StoreStatus {
        &self.status
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn objects(&self) -> &HashMap<ObjectKey, Arc<Value>> {
        &self.objects
    }
}

impl StoreCopies {
    /// Copies the stores of `keys` that `reader` has.
    pub fn of(reader: &dyn StoreReader, keys: &[StoreKey]) -> Self {
        Self(
            keys.iter()
                .filter_map(|key| {
                    let store = reader.read(key)?;
                    Some((
                        key.clone(),
                        StoreCopy {
                            key: key.clone(),
                            status: store.status().clone(),
                            generation: store.generation(),
                            objects: store.objects().clone(),
                        },
                    ))
                })
                .collect(),
        )
    }
}

impl StoreReader for StoreCopies {
    fn read(&self, key: &StoreKey) -> Option<&dyn StoreView> {
        self.0.get(key).map(|store| store as &dyn StoreView)
    }
}

/// A [`StoreSource`] with stores filled by hand: nothing watches.
#[derive(Default)]
pub struct MemoryStores {
    stores: HashMap<StoreKey, StoreCore>,
    leases: HashMap<StoreKey, Rc<()>>,
}

impl MemoryStores {
    /// Makes (or replaces) the store for `key` with `objects`, ready.
    pub fn set(&mut self, key: StoreKey, objects: impl IntoIterator<Item = Value>) {
        let mut store = StoreCore::from_objects(key.clone(), objects);
        if let Some(old) = self.stores.get(&key) {
            store.set_generation(old.generation() + 1);
        }
        self.stores.insert(key, store);
    }

    /// Sets the status of the store of `key` (a watch that was refused, say), as a batch would.
    pub fn set_status(&mut self, key: &StoreKey, status: StoreStatus) {
        if let Some(store) = self.stores.get_mut(key) {
            store.apply(
                vec![crate::store::Change::Status(status)],
                &mut kubyl_base::host::TestHost::new(),
            );
        }
    }

    /// Leases currently held on `key`.
    pub fn leases(&self, key: &StoreKey) -> usize {
        self.leases
            .get(key)
            .map_or(0, |lease| Rc::strong_count(lease) - 1)
    }
}

impl StoreSource for MemoryStores {
    fn acquire(&mut self, key: StoreKey) -> StoreLease {
        self.stores
            .entry(key.clone())
            .or_insert_with(|| StoreCore::new(key.clone()));
        let hold = self.leases.entry(key.clone()).or_default().clone();
        StoreLease::new(key, hold)
    }

    fn view(&self, key: &StoreKey) -> Option<&dyn StoreView> {
        self.stores.get(key).map(|store| store as &dyn StoreView)
    }

    fn keys(&self) -> Vec<StoreKey> {
        self.stores.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use kubyl_base::{ClusterId, Gvr};
    use serde_json::json;

    use super::*;
    use crate::store::object_key;

    fn key() -> StoreKey {
        StoreKey::new(ClusterId::new("c"), Gvr::new("", "v1", "configmaps"), None)
    }

    #[test]
    fn services_read_what_the_source_holds_while_they_hold_a_lease() {
        let mut stores = MemoryStores::default();
        let lease = stores.acquire(key());
        // Not filled yet: waiting, nothing to read.
        let view = stores.view(lease.key()).unwrap();
        assert_eq!(view.status(), &StoreStatus::Waiting);
        assert!(view.objects().is_empty());

        stores.set(
            key(),
            [json!({"metadata": {"namespace": "a", "name": "one"}})],
        );
        let view = stores.view(lease.key()).unwrap();
        assert!(view.status().is_ready());
        assert!(view.get(&object_key(Some("a"), "one")).is_some());
        assert_eq!(view.generation(), 1);
        assert_eq!(stores.keys(), [key()]);

        // A reader sees the same stores through a shared borrow.
        let reader: &dyn StoreReader = &stores;
        assert_eq!(reader.read(&key()).unwrap().generation(), 1);

        // A copy keeps what the store held, whatever happens to it later.
        let copies = StoreCopies::of(&stores, &[key()]);
        stores.set(key(), []);
        assert!(
            copies
                .read(&key())
                .unwrap()
                .get(&object_key(Some("a"), "one"))
                .is_some()
        );
        assert!(
            StoreCopies::of(&stores, &[key()])
                .read(&key())
                .unwrap()
                .objects()
                .is_empty()
        );

        assert_eq!(stores.leases(&key()), 1);
        let second = lease.clone();
        drop(lease);
        assert_eq!(stores.leases(&key()), 1);
        drop(second);
        assert_eq!(stores.leases(&key()), 0);
    }
}
