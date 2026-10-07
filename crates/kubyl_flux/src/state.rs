//! [`Flux`]: the app-wide entity that knows, per cluster, which Flux kinds are served (from
//! `ClusterCaps`, so it follows installs and uninstalls live) and where the controllers run. The
//! state machine is [`FluxCore`] from `kubyl_flux_core`.
//!
//! While a cluster serves Kustomizations or HelmReleases, it also watches them in every
//! namespace (shared watches, cheap) to keep the [`FluxIndex`]: which Flux objects exist on
//! which cluster and their state, for the "Flux" column of workload lists.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::FutureExt as _;
use gpui::{App, AppContext as _, Context, Entity, EntityId, EventEmitter, Global, Subscription};
use kube::discovery::ApiResource;
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ClusterCaps, ClusterId, FluxCaps, Gvr};
use kubyl_flux_core::detect;
use kubyl_flux_core::kinds::FluxKind;
use kubyl_flux_core::model::{FluxObject, State};
use kubyl_flux_core::service::{Detector, FluxCore, FluxEffect};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::store;
use kubyl_resources::{ResourceStore, ResourceStores, StoreHandle, StoreKey};
use parking_lot::RwLock;

/// A Flux object's kind, namespace and name.
type IndexKey = (FluxKind, String, String);
/// The objects of one cluster: their resourceVersion and state.
type ClusterIndex = HashMap<IndexKey, (Option<String>, State)>;

/// Flux objects by (kind, namespace, name) per cluster, with their state, for the "Flux" column
/// of workload lists (which can't read stores themselves).
#[derive(Default)]
pub struct FluxIndex {
    /// cluster → object → (resourceVersion, state).
    clusters: RwLock<HashMap<ClusterId, ClusterIndex>>,
    /// Some cluster serves Flux.
    any: AtomicBool,
    /// The clusters that serve Flux (cheap for tables, which ask per cell).
    serving: RwLock<HashSet<ClusterId>>,
}

impl FluxIndex {
    /// The state of a Flux object on `cluster`.
    pub fn state(
        &self,
        cluster: &ClusterId,
        kind: FluxKind,
        namespace: &str,
        name: &str,
    ) -> Option<State> {
        self.clusters
            .read()
            .get(cluster)?
            .get(&(kind, namespace.to_string(), name.to_string()))
            .map(|(_, s)| *s)
    }

    pub fn any(&self) -> bool {
        self.any.load(Ordering::Relaxed)
    }

    /// Whether `cluster` serves any Flux kind (as last synced from its discovery).
    pub fn serves(&self, cluster: &ClusterId) -> bool {
        self.serving.read().contains(cluster)
    }

    pub(crate) fn set_serving(&self, cluster: &ClusterId, serves: bool) {
        let mut serving = self.serving.write();
        if serves {
            serving.insert(cluster.clone());
        } else {
            serving.remove(cluster);
        }
        self.any.store(!serving.is_empty(), Ordering::Relaxed);
    }

    /// Replaces `cluster`'s objects; only objects whose resourceVersion changed are parsed.
    pub(crate) fn replace<'a>(
        &self,
        cluster: &ClusterId,
        objects: impl IntoIterator<Item = (FluxKind, &'a Arc<serde_json::Value>)>,
    ) {
        let previous = self.clusters.write().remove(cluster).unwrap_or_default();
        let mut next = HashMap::with_capacity(previous.len());
        for (kind, object) in objects {
            let meta = object.get("metadata");
            let text = |key: &str| {
                meta.and_then(|m| m.get(key))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            let (Some(name), namespace) = (text("name"), text("namespace").unwrap_or_default())
            else {
                continue;
            };
            let version = text("resourceVersion");
            let key = (kind, namespace, name);
            let state = match previous.get(&key) {
                Some((v, state)) if version.is_some() && *v == version => *state,
                _ => match FluxObject::parse_as(kind, object) {
                    Some(parsed) => parsed.state(),
                    None => continue,
                },
            };
            next.insert(key, (version, state));
        }
        if !next.is_empty() {
            self.clusters.write().insert(cluster.clone(), next);
        }
    }

    pub(crate) fn forget(&self, cluster: &ClusterId) {
        self.clusters.write().remove(cluster);
    }
}

/// Flux state of every cluster. One per app: [`Flux::global`].
pub struct Flux {
    core: FluxCore,
    index: Arc<FluxIndex>,
    /// Kustomizations and HelmReleases of each Flux cluster (all namespaces).
    watches: HashMap<ClusterId, Vec<(FluxKind, StoreHandle)>>,
    _watch_observers: HashMap<ClusterId, Vec<Subscription>>,
    _subscriptions: Vec<Subscription>,
}

impl Deref for Flux {
    type Target = FluxCore;

    fn deref(&self) -> &FluxCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for Flux {}

impl Hosts<FluxCore> for Flux {
    fn service(&mut self) -> &mut FluxCore {
        &mut self.core
    }

    fn apply(&mut self, effect: FluxEffect, cx: &mut Context<Self>) {
        match effect {
            FluxEffect::TreeGroupsChanged => kubyl_explorer::catalog::tree_groups_changed(cx),
        }
    }
}

struct GlobalFlux(Entity<Flux>);

impl Global for GlobalFlux {}

impl Flux {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let index = Arc::new(FluxIndex::default());
        let entity = cx.new(|cx| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(
                    &manager,
                    |this: &mut Flux, _, event: &ConnectionEvent, cx| match event {
                        ConnectionEvent::DiscoveryChanged(id)
                        | ConnectionEvent::StateChanged(id) => this.sync_cluster(id, cx),
                        ConnectionEvent::ContextsChanged => {
                            for id in this.core.clusters() {
                                this.sync_cluster(&id, cx);
                            }
                        }
                        _ => {}
                    },
                ));
            }
            Self {
                core: FluxCore::new(),
                index: index.clone(),
                watches: HashMap::new(),
                _watch_observers: HashMap::new(),
                _subscriptions: subscriptions,
            }
        });
        cx.set_global(GlobalFlux(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalFlux>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalFlux>().map(|g| g.0.clone())
    }

    pub fn index(&self) -> Arc<FluxIndex> {
        self.index.clone()
    }

    /// Which Flux kinds `cluster` serves (live, from discovery).
    pub fn caps(cluster: &ClusterId, cx: &App) -> FluxCaps {
        #[cfg(test)]
        if let Some(caps) = cx.try_global::<TestCaps>() {
            return caps.0.flux;
        }
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(cluster).flux)
            .unwrap_or_default()
    }

    /// Follows a cluster's caps and connection.
    fn sync_cluster(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let (caps, client) = {
            let m = manager.read(cx);
            let connected = m.state(cluster).is_connected();
            (
                m.caps(cluster).flux,
                m.client(cluster).filter(|_| connected),
            )
        };
        let detector: Option<Detector> = client.map(|client| {
            Arc::new(move || {
                let client = client.clone();
                async move { detect::detect(client).await }.boxed()
            }) as Detector
        });
        let watch = detector.is_some() && (caps.kustomizations || caps.helm_releases);
        hosted(self, cx, |core, host| {
            core.sync_cluster(cluster, caps, detector, host)
        });
        if watch {
            self.watch_cluster(cluster, cx);
        } else if self.watches.remove(cluster).is_some() {
            self._watch_observers.remove(cluster);
            self.index.forget(cluster);
        }
        self.index.set_serving(cluster, caps.any());
        cx.notify();
    }

    fn watch_cluster(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let kinds: Vec<(FluxKind, Gvr)> = [FluxKind::Kustomization, FluxKind::HelmRelease]
            .into_iter()
            .filter_map(|kind| Some((kind, gvr(cluster, kind, cx)?)))
            .collect();
        let current: Vec<(FluxKind, Gvr)> = self
            .watches
            .get(cluster)
            .map(|w| {
                w.iter()
                    .map(|(k, h)| (*k, h.read(cx).key().gvr.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if current == kinds {
            return;
        }
        let handles: Vec<(FluxKind, StoreHandle)> = kinds
            .into_iter()
            .map(|(kind, gvr)| (kind, ResourceStores::acquire(cx, all_key(cluster, &gvr))))
            .collect();
        let observers = handles
            .iter()
            .map(|(_, handle)| {
                let cluster = cluster.clone();
                cx.observe(handle.entity(), move |this, _, cx| {
                    this.reindex(&cluster, cx);
                })
            })
            .collect();
        self.watches.insert(cluster.clone(), handles);
        self._watch_observers.insert(cluster.clone(), observers);
        self.reindex(cluster, cx);
    }

    /// Keeps the index current. Doesn't notify: the index is read when tables render, and
    /// the views that show Flux objects watch their own stores.
    fn reindex(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(watches) = self.watches.get(cluster) else {
            return;
        };
        let stores: Vec<(FluxKind, &ResourceStore)> = watches
            .iter()
            .map(|(kind, h)| (*kind, h.read(cx)))
            .collect();
        self.index.replace(
            cluster,
            stores
                .iter()
                .flat_map(|(kind, store)| store.objects().values().map(move |o| (*kind, o))),
        );
    }

    /// Detects the controllers again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.redetect(cluster, host));
    }

    /// The Flux version of `cluster`, for the sidebar group's badge.
    pub fn version(&self, cluster: &ClusterId) -> Option<String> {
        self.core.install(cluster)?.version()
    }
}

/// Caps for GPUI tests (no cluster connects there).
#[cfg(test)]
pub(crate) struct TestCaps(pub ClusterCaps);

#[cfg(test)]
impl Global for TestCaps {}

/// The capabilities of `cluster` (read-only and PROD flags included).
pub fn cluster_caps(cluster: &ClusterId, cx: &App) -> ClusterCaps {
    #[cfg(test)]
    if let Some(caps) = cx.try_global::<TestCaps>() {
        return caps.0.clone();
    }
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).caps(cluster))
        .unwrap_or_default()
}

/// Whether actions are hidden on `cluster` (read-only in Kubyl).
pub fn read_only(cluster: &ClusterId, cx: &App) -> bool {
    cluster_caps(cluster, cx).read_only
}

/// The served GVR of a Flux kind (its preferred version).
pub fn gvr(cluster: &ClusterId, kind: FluxKind, cx: &App) -> Option<Gvr> {
    if let Some((gvr, _)) = resource(cluster, kind, cx) {
        return Some(gvr);
    }
    // GPUI tests have no discovery: the GA versions.
    #[cfg(test)]
    if cx.has_global::<TestCaps>() {
        return Some(Gvr::new(kind.group(), ga_version(kind), kind.plural()));
    }
    None
}

/// The GA (or latest) version of each kind in Flux 2.x.
pub fn ga_version(kind: FluxKind) -> &'static str {
    match kind {
        FluxKind::HelmRelease => "v2",
        FluxKind::Alert | FluxKind::Provider => "v1beta3",
        _ => "v1",
    }
}

/// The served resource of a Flux kind: its GVR (preferred version) and kube `ApiResource`.
pub fn resource(cluster: &ClusterId, kind: FluxKind, cx: &App) -> Option<(Gvr, ApiResource)> {
    let discovery = ConnectionManager::try_global(cx)?
        .read(cx)
        .discovery(cluster)?;
    let info = store::find_resource(
        &discovery.resources,
        &Gvr::new(kind.group(), "", kind.plural()),
    )?;
    Some((info.gvr.clone(), store::api_resource(info)))
}

/// The watch of a kind in every namespace.
pub fn all_key(cluster: &ClusterId, gvr: &Gvr) -> StoreKey {
    StoreKey::new(cluster.clone(), gvr.clone(), None)
}

/// The objects of `kind` in a loaded store.
pub fn objects_of(kind: FluxKind, store: &ResourceStore) -> Vec<FluxObject> {
    store
        .objects()
        .values()
        .filter_map(|o| FluxObject::parse_as(kind, o))
        .collect()
}

/// Parsed Flux objects per store, re-parsed only when the store changed (its generation or
/// size): views read them while rendering, which happens far more often than Flux objects
/// change. One per view.
#[derive(Default)]
pub struct ParsedObjects(RefCell<HashMap<EntityId, Parsed>>);

/// A store's generation and size, and its objects parsed then.
type Parsed = (u64, usize, Arc<Vec<FluxObject>>);

impl ParsedObjects {
    /// The objects of `kind` in `store`.
    pub fn of(
        &self,
        kind: FluxKind,
        store: &Entity<ResourceStore>,
        cx: &App,
    ) -> Arc<Vec<FluxObject>> {
        let read = store.read(cx);
        let stamp = (read.generation(), read.len());
        let id = store.entity_id();
        if let Some((generation, len, objects)) = self.0.borrow().get(&id)
            && (*generation, *len) == stamp
        {
            return objects.clone();
        }
        let objects = Arc::new(objects_of(kind, read));
        let mut cache = self.0.borrow_mut();
        // Stores a view stopped reading go with the next miss.
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert(id, (stamp.0, stamp.1, objects.clone()));
        objects
    }
}

/// A Flux object from any running watch of its cluster (all namespaces, its namespace, or one
/// for its name).
pub fn find(
    cluster: &ClusterId,
    kind: FluxKind,
    namespace: &str,
    name: &str,
    cx: &App,
) -> Option<FluxObject> {
    let gvr = gvr(cluster, kind, cx)?;
    let key = kubyl_resources::object_key(Some(namespace), name);
    [
        all_key(cluster, &gvr),
        StoreKey::new(cluster.clone(), gvr.clone(), Some(namespace.to_string())),
        StoreKey::new(cluster.clone(), gvr.clone(), Some(namespace.to_string()))
            .fields(format!("metadata.name={name}")),
    ]
    .iter()
    .filter_map(|k| ResourceStores::peek(cx, k))
    .find_map(|store| store.read(cx).get(&key).cloned())
    .and_then(|o| FluxObject::parse_as(kind, &o))
}

/// The served GVR (preferred version) of any kind by group and kind name (inventory entries).
pub fn served_gvr(cluster: &ClusterId, group: &str, kind: &str, cx: &App) -> Option<Gvr> {
    let discovery = ConnectionManager::try_global(cx)?
        .read(cx)
        .discovery(cluster)?;
    discovery
        .resources
        .iter()
        .find(|r| r.gvk.group == group && r.gvk.kind == kind && r.preferred)
        .or_else(|| {
            discovery
                .resources
                .iter()
                .find(|r| r.gvk.group == group && r.gvk.kind == kind)
        })
        .map(|r| r.gvr.clone())
}

/// An object from any running watch of its kind (its namespace or all namespaces, full or
/// metadata only): no new watch is started.
pub fn peek_object(
    cluster: &ClusterId,
    gvr: &Gvr,
    namespace: Option<&str>,
    name: &str,
    cx: &App,
) -> Option<Arc<serde_json::Value>> {
    let key = kubyl_resources::object_key(namespace, name);
    let mut keys = vec![StoreKey::new(cluster.clone(), gvr.clone(), None)];
    if let Some(namespace) = namespace {
        keys.push(StoreKey::new(
            cluster.clone(),
            gvr.clone(),
            Some(namespace.to_string()),
        ));
    }
    let keys: Vec<StoreKey> = keys
        .into_iter()
        .flat_map(|k| [k.clone().metadata(), k])
        .collect();
    keys.iter()
        .filter_map(|k| ResourceStores::peek(cx, k))
        .find_map(|store| store.read(cx).get(&key).cloned())
}
