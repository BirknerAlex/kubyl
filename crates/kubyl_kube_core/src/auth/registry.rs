//! The process-wide registries of in-memory tokens (OIDC slots, OpenShift auths), keyed by
//! [`Credentials::key`](super::store::Credentials::key).
//!
//! The default scope's entries stay for the life of the process: the desktop rebuilds clients
//! (reconnects, new kubeconfig contents) and expects a token that only lives in memory, like an
//! ID token too large for the Windows Credential Manager, to survive that. A scoped handle's
//! entries are weak: they go with the last client or sign-in that uses them, so a host that
//! serves many users from one process doesn't keep a dropped user's tokens in memory.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;

enum Entry<T> {
    Strong(Arc<T>),
    Weak(Weak<T>),
}

impl<T> Entry<T> {
    fn live(&self) -> Option<Arc<T>> {
        match self {
            Entry::Strong(value) => Some(value.clone()),
            Entry::Weak(value) => value.upgrade(),
        }
    }
}

pub(crate) struct Registry<T> {
    entries: Mutex<HashMap<String, Entry<T>>>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
        }
    }
}

impl<T> Registry<T> {
    /// The live entry under `key`, if any.
    pub(crate) fn get(&self, key: &str) -> Option<Arc<T>> {
        self.entries.lock().get(key).and_then(Entry::live)
    }

    /// The live entry under `key`, made with `make` when there is none or `stale` says the
    /// existing one must go. `strong` entries stay until replaced; the others only as long as
    /// something else holds them. Dead entries are dropped on the way.
    pub(crate) fn get_or_insert(
        &self,
        key: String,
        strong: bool,
        stale: impl FnOnce(&T) -> bool,
        make: impl FnOnce() -> T,
    ) -> Arc<T> {
        let mut entries = self.entries.lock();
        entries.retain(|_, entry| entry.live().is_some());
        if let Some(existing) = entries.get(&key).and_then(Entry::live)
            && !stale(&existing)
        {
            return existing;
        }
        let value = Arc::new(make());
        let entry = if strong {
            Entry::Strong(value.clone())
        } else {
            Entry::Weak(Arc::downgrade(&value))
        };
        entries.insert(key, entry);
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(registry: &Registry<String>) -> usize {
        registry.entries.lock().len()
    }

    #[test]
    fn scoped_entries_go_with_their_last_holder() {
        let registry = Registry::<String>::default();
        let alice = registry.get_or_insert("scope/alice/k".into(), false, |_| false, || "a".into());
        assert!(Arc::ptr_eq(
            &alice,
            &registry.get_or_insert("scope/alice/k".into(), false, |_| false, || "b".into())
        ));
        assert!(registry.get("scope/alice/k").is_some());
        drop(alice);
        assert!(registry.get("scope/alice/k").is_none());
        // The dead entry is pruned by the next insert.
        registry.get_or_insert("scope/bob/k".into(), false, |_| false, || "b".into());
        assert_eq!(entries(&registry), 1);
    }

    #[test]
    fn default_entries_stay_and_stale_ones_are_replaced() {
        let registry = Registry::<String>::default();
        let first = registry.get_or_insert("k".into(), true, |_| false, || "old".into());
        drop(first);
        assert_eq!(
            registry.get("k").as_deref().map(String::as_str),
            Some("old")
        );
        let replaced = registry.get_or_insert("k".into(), true, |v| v == "old", || "new".into());
        assert_eq!(replaced.as_str(), "new");
        assert_eq!(
            registry.get("k").as_deref().map(String::as_str),
            Some("new")
        );
    }
}
