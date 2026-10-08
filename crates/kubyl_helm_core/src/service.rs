//! [`HelmCore`]: the Helm releases of each cluster a view shows, on any [`Host`].
//!
//! Releases come from **metadata-only** watches of the Secrets and ConfigMaps labelled
//! `owner=helm`: names, revisions, status and times are labels, so the list needs no Secret
//! data. What the labels don't say (chart, versions, description) comes from the latest
//! revision's object, fetched and decoded on Tokio; only that summary is kept. A release's
//! values and manifest are loaded by its tab and live there.
//!
//! The service doesn't own watches. The app acquires the two stores with
//! [`HelmStores::acquire`] (through a [`StoreSource`]) and tells the service when they changed,
//! with copies of their contents ([`HelmInputs`]).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kubyl_base::host::{Flow, Host, HostExt as _, Service, TaskHandle};
use kubyl_base::{ClusterId, Gvr};
use kubyl_resources_core::source::{StoreCopies, StoreLease, StoreReader, StoreSource};
use kubyl_resources_core::store::{StoreKey, StoreStatus};
use serde_json::Value;

use super::decode::Driver;
use super::release::*;

const KEEP: Duration = Duration::from_secs(120);
const SWEEP: Duration = Duration::from_secs(30);
/// Summaries fetched at once per cluster.
const PARALLEL: usize = 6;

/// Keeps a cluster's Helm watches while held.
#[derive(Clone)]
pub struct HelmLease(#[allow(dead_code)] Arc<()>);

type ObjectId = (Driver, String, String);

/// The two watches a cluster's releases come from.
pub struct HelmStores {
    scope: Option<String>,
    /// Held so the watches live as long as the cluster is followed.
    _secrets: StoreLease,
    _config_maps: StoreLease,
}

impl HelmStores {
    /// The keys of the Secrets and ConfigMaps watches, limited to `scope` when listing
    /// cluster-wide isn't allowed.
    pub fn keys(cluster: &ClusterId, scope: Option<&str>) -> [StoreKey; 2] {
        let key = |resource: &str| {
            StoreKey::new(
                cluster.clone(),
                Gvr::new("", "v1", resource),
                scope.map(str::to_string),
            )
            .labels(SELECTOR)
            .metadata()
        };
        [key("secrets"), key("configmaps")]
    }

    /// Acquires the watches of `cluster` from `source`.
    pub fn acquire(
        cluster: &ClusterId,
        scope: Option<String>,
        source: &mut dyn StoreSource,
    ) -> Self {
        let [secrets, config_maps] = Self::keys(cluster, scope.as_deref());
        Self {
            scope,
            _secrets: source.acquire(secrets),
            _config_maps: source.acquire(config_maps),
        }
    }
}

/// What the service reads from the app when a watch changed.
#[derive(Default)]
pub struct HelmInputs {
    /// The cluster's client, `None` while it is disconnected (summaries wait).
    pub client: Option<kube::Client>,
    /// The namespace to list when listing cluster-wide is forbidden: the active one for this
    /// cluster, else the context's.
    pub fallback_namespace: Option<String>,
    /// The cluster's stores ([`HelmStores::keys`] of its scope).
    pub stores: StoreCopies,
}

/// What the service asks the app to do.
#[derive(Debug)]
pub enum HelmEffect {
    /// Listing cluster-wide is forbidden: list `namespace` only (call [`HelmCore::rescope`]
    /// with stores acquired for it).
    Rescope {
        cluster: ClusterId,
        namespace: String,
    },
}

struct ClusterHelm {
    lease: Arc<()>,
    wanted_until: Instant,
    scope: Option<String>,
    _stores: HelmStores,
    client: Option<kube::Client>,
    /// Summaries by storage object and its resourceVersion.
    summaries: HashMap<ObjectId, (String, SummaryState)>,
    queue: VecDeque<(ObjectId, String)>,
    /// Objects being fetched.
    in_flight: HashSet<ObjectId>,
    snapshot: RefCell<Option<(u64, Arc<Snapshot>)>>,
    revision: u64,
}

/// The Helm releases of every cluster a view asked about.
pub struct HelmCore {
    clusters: HashMap<ClusterId, ClusterHelm>,
    sweep: Option<TaskHandle>,
}

impl Service for HelmCore {
    type Event = Infallible;
    type Effect = HelmEffect;
}

impl Default for HelmCore {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmCore {
    pub fn new() -> Self {
        Self {
            clusters: HashMap::new(),
            sweep: None,
        }
    }

    /// Drops the watches nobody holds, every 30 s (off in tests that must not wake timers).
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        self.sweep = Some(host.every(SWEEP, |this, _| {
            this.sweep();
            Flow::Continue
        }));
    }

    fn sweep(&mut self) {
        let now = Instant::now();
        self.clusters
            .retain(|_, state| Arc::strong_count(&state.lease) > 1 || state.wanted_until > now);
    }

    /// Whether the service follows `cluster` (the app then has its watches).
    pub fn tracks(&self, cluster: &ClusterId) -> bool {
        self.clusters.contains_key(cluster)
    }

    /// The namespace `cluster`'s list is limited to, if it is.
    pub fn scope(&self, cluster: &ClusterId) -> Option<&str> {
        self.clusters.get(cluster)?.scope.as_deref()
    }

    /// The clusters the service follows.
    pub fn tracked(&self) -> Vec<ClusterId> {
        self.clusters.keys().cloned().collect()
    }

    /// Keeps `cluster`'s Helm watches while the returned lease lives. `stores` are acquired by
    /// the app when [`Self::tracks`] says no; `None` for a cluster the service follows.
    pub fn watch(
        &mut self,
        cluster: &ClusterId,
        stores: Option<HelmStores>,
        inputs: &HelmInputs,
        host: &mut dyn Host<Self>,
    ) -> Option<HelmLease> {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.wanted_until = Instant::now() + KEEP;
            return Some(HelmLease(state.lease.clone()));
        }
        let stores = stores?;
        let lease = Arc::new(());
        self.insert(cluster, stores, lease.clone());
        self.changed(cluster, inputs, host);
        Some(HelmLease(lease))
    }

    fn insert(&mut self, cluster: &ClusterId, stores: HelmStores, lease: Arc<()>) {
        self.clusters.insert(
            cluster.clone(),
            ClusterHelm {
                lease,
                wanted_until: Instant::now() + KEEP,
                scope: stores.scope.clone(),
                _stores: stores,
                client: None,
                summaries: HashMap::new(),
                queue: VecDeque::new(),
                in_flight: HashSet::new(),
                snapshot: RefCell::new(None),
                revision: 0,
            },
        );
    }

    /// Lists only `scope` (when listing Secrets cluster-wide isn't allowed), or every
    /// namespace again (`None`). `stores` are acquired for the new scope.
    pub fn rescope(
        &mut self,
        cluster: &ClusterId,
        stores: HelmStores,
        inputs: &HelmInputs,
        host: &mut dyn Host<Self>,
    ) {
        let Some(old) = self.clusters.remove(cluster) else {
            return;
        };
        self.insert(cluster, stores, old.lease);
        self.changed(cluster, inputs, host);
        host.notify();
    }

    /// A cluster's id changed: start over under the new one (the stores carry the old one).
    pub fn rekeyed(&mut self, from: &ClusterId, host: &mut dyn Host<Self>) {
        self.clusters.remove(from);
        host.notify();
    }

    /// A cluster connected or disconnected.
    pub fn connection_changed(&mut self, host: &mut dyn Host<Self>) {
        host.notify();
    }

    /// One of `cluster`'s watches changed.
    pub fn changed(&mut self, cluster: &ClusterId, inputs: &HelmInputs, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.revision += 1;
        state.client = inputs.client.clone();
        let [secrets_key, config_maps_key] = HelmStores::keys(cluster, state.scope.as_deref());
        let (Some(secrets), Some(config_maps)) = (
            inputs.stores.read(&secrets_key),
            inputs.stores.read(&config_maps_key),
        ) else {
            host.notify();
            return;
        };
        // Listing cluster-wide is forbidden: fall back to the active namespace.
        if state.scope.is_none()
            && *secrets.status() == StoreStatus::Forbidden
            && let Some(namespace) = inputs.fallback_namespace.clone()
        {
            host.effect(HelmEffect::Rescope {
                cluster: cluster.clone(),
                namespace,
            });
            return;
        }
        let objects: Vec<(Driver, Arc<Value>)> = secrets
            .objects()
            .values()
            .map(|o| (Driver::Secret, o.clone()))
            .chain(
                config_maps
                    .objects()
                    .values()
                    .map(|o| (Driver::ConfigMap, o.clone())),
            )
            .collect();
        let releases = group(&objects);
        let mut wanted = HashMap::new();
        for (namespace, _, driver, revisions, resource_version) in &releases {
            let id: ObjectId = (*driver, namespace.clone(), revisions[0].object.clone());
            wanted.insert(id, resource_version.clone());
        }
        // Forget summaries of objects that are gone; queue new or changed ones.
        state.summaries.retain(|id, _| wanted.contains_key(id));
        state.in_flight.retain(|id| wanted.contains_key(id));
        state.queue.retain(|(id, _)| wanted.contains_key(id));
        for (id, resource_version) in wanted {
            let current = state.summaries.get(&id).map(|(rv, _)| rv);
            if current != Some(&resource_version)
                && !state
                    .queue
                    .iter()
                    .any(|(q, rv)| q == &id && rv == &resource_version)
            {
                state.summaries.insert(
                    id.clone(),
                    (resource_version.clone(), SummaryState::Loading),
                );
                state.in_flight.remove(&id);
                state.queue.push_back((id, resource_version));
            }
        }
        self.pump(cluster, host);
        host.notify();
    }

    /// Starts queued summary fetches, a few at a time.
    fn pump(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        let Some(client) = state.client.clone() else {
            return;
        };
        while state.in_flight.len() < PARALLEL {
            let Some((id, resource_version)) = state.queue.pop_front() else {
                break;
            };
            let (driver, namespace, object) = id.clone();
            let task_cluster = cluster.clone();
            let task_id = id.clone();
            // A fetch for a cluster that went away finds nothing to report to: let it finish.
            host.spawn(
                fetch_summary(client.clone(), driver, namespace, object),
                move |this, result, host| {
                    if let Some(state) = this.clusters.get_mut(&task_cluster) {
                        state.in_flight.remove(&task_id);
                        if let Some(entry) = state.summaries.get_mut(&task_id)
                            && entry.0 == resource_version
                        {
                            entry.1 = match result {
                                Ok(summary) => SummaryState::Ready(Arc::new(summary)),
                                Err(err) => SummaryState::Failed(err),
                            };
                        }
                        state.revision += 1;
                    }
                    this.pump(&task_cluster, host);
                    host.notify();
                },
            )
            .detach();
            state.in_flight.insert(id);
        }
    }

    /// The releases of `cluster`, if its watches run ([`Self::watch`]). Built again only when a
    /// watch changed.
    pub fn snapshot(&self, cluster: &ClusterId, stores: &dyn StoreReader) -> Option<Arc<Snapshot>> {
        let state = self.clusters.get(cluster)?;
        let [secrets_key, config_maps_key] = HelmStores::keys(cluster, state.scope.as_deref());
        let secrets = stores.read(&secrets_key)?;
        let config_maps = stores.read(&config_maps_key)?;
        let generation =
            state.revision * 1_000_003 + secrets.generation() * 31 + config_maps.generation();
        if let Some((g, snapshot)) = state.snapshot.borrow().as_ref()
            && *g == generation
        {
            return Some(snapshot.clone());
        }
        let objects: Vec<(Driver, Arc<Value>)> = secrets
            .objects()
            .values()
            .map(|o| (Driver::Secret, o.clone()))
            .chain(
                config_maps
                    .objects()
                    .values()
                    .map(|o| (Driver::ConfigMap, o.clone())),
            )
            .collect();
        let releases = group(&objects)
            .into_iter()
            .map(|(namespace, name, driver, revisions, _)| {
                let id = (driver, namespace.clone(), revisions[0].object.clone());
                ReleaseRow {
                    summary: state
                        .summaries
                        .get(&id)
                        .map(|(_, s)| s.clone())
                        .unwrap_or(SummaryState::Loading),
                    namespace,
                    name,
                    driver,
                    revisions,
                }
            })
            .collect();
        let problem = problem(secrets.status(), state.scope.as_deref());
        let snapshot = Arc::new(Snapshot {
            releases,
            loading: !secrets.status().is_settled(),
            problem,
            scope: state.scope.clone(),
        });
        *state.snapshot.borrow_mut() = Some((generation, snapshot.clone()));
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use kubyl_base::host::TestHost;
    use kubyl_resources_core::source::MemoryStores;
    use serde_json::json;

    use super::*;

    fn release(ns: &str, name: &str, revision: u32, status: &str, rv: &str) -> Value {
        json!({"metadata": {"namespace": ns, "name": format!("sh.helm.release.v1.{name}.v{revision}"),
            "resourceVersion": rv,
            "labels": {"owner": "helm", "name": name, "version": revision.to_string(),
                "status": status, "modifiedAt": "1790338899"}}})
    }

    struct Fixture {
        core: HelmCore,
        host: TestHost<HelmCore>,
        stores: MemoryStores,
        cluster: ClusterId,
    }

    fn fixture() -> Fixture {
        Fixture {
            core: HelmCore::new(),
            host: TestHost::new(),
            stores: MemoryStores::default(),
            cluster: ClusterId::new("c"),
        }
    }

    impl Fixture {
        fn fill(&mut self, scope: Option<&str>, secrets: Vec<Value>) {
            let [secrets_key, config_maps_key] = HelmStores::keys(&self.cluster, scope);
            self.stores.set(secrets_key, secrets);
            self.stores.set(config_maps_key, []);
        }

        fn inputs(&self, scope: Option<&str>, fallback: Option<&str>) -> HelmInputs {
            HelmInputs {
                client: None,
                fallback_namespace: fallback.map(str::to_string),
                stores: StoreCopies::of(&self.stores, &HelmStores::keys(&self.cluster, scope)),
            }
        }

        fn watch(&mut self, scope: Option<&str>, fallback: Option<&str>) -> Option<HelmLease> {
            let stores = (!self.core.tracks(&self.cluster)).then(|| {
                HelmStores::acquire(&self.cluster, scope.map(str::to_string), &mut self.stores)
            });
            let inputs = self.inputs(scope, fallback);
            self.core
                .watch(&self.cluster, stores, &inputs, &mut self.host)
        }
    }

    #[test]
    fn releases_come_from_the_watches_of_a_followed_cluster() {
        let mut f = fixture();
        f.fill(
            None,
            vec![
                release("monitoring", "kps", 5, "superseded", "10"),
                release("monitoring", "kps", 6, "deployed", "11"),
                release("shop", "db", 1, "failed", "12"),
            ],
        );

        // Nothing is followed until a view asks.
        assert!(f.core.snapshot(&f.cluster, &f.stores).is_none());
        let lease = f.watch(None, None).expect("a lease");
        assert!(f.core.tracks(&f.cluster));

        let snapshot = f.core.snapshot(&f.cluster, &f.stores).unwrap();
        assert_eq!(snapshot.releases.len(), 2);
        let kps = snapshot.releases.iter().find(|r| r.name == "kps").unwrap();
        assert_eq!(kps.revisions[0].revision, 6);
        // The summaries need the cluster's client: until then they load.
        assert!(matches!(kps.summary, SummaryState::Loading));
        assert!(!snapshot.loading);
        assert!(f.host.notified > 0);

        // The same generation gives the same snapshot; a change builds a new one.
        let again = f.core.snapshot(&f.cluster, &f.stores).unwrap();
        assert!(Arc::ptr_eq(&snapshot, &again));
        f.fill(None, vec![release("shop", "db", 2, "deployed", "13")]);
        let inputs = f.inputs(None, None);
        f.core.changed(&f.cluster, &inputs, &mut f.host);
        let changed = f.core.snapshot(&f.cluster, &f.stores).unwrap();
        assert_eq!(changed.releases.len(), 1);
        drop(lease);
    }

    #[test]
    fn forbidden_cluster_wide_listing_falls_back_to_a_namespace() {
        let mut f = fixture();
        f.fill(None, vec![]);
        let [secrets_key, _] = HelmStores::keys(&f.cluster, None);
        // The watch was refused: replace the store with a forbidden one.
        let _lease = f.watch(None, Some("shop"));
        f.stores.set_status(&secrets_key, StoreStatus::Forbidden);
        let inputs = f.inputs(None, Some("shop"));
        f.core.changed(&f.cluster, &inputs, &mut f.host);

        assert!(matches!(
            f.host.effects.as_slice(),
            [HelmEffect::Rescope { namespace, .. }] if namespace == "shop"
        ));

        // The app acquires the namespace's watches; the lease carries over.
        f.fill(
            Some("shop"),
            vec![release("shop", "db", 1, "deployed", "1")],
        );
        let stores = HelmStores::acquire(&f.cluster, Some("shop".into()), &mut f.stores);
        let inputs = f.inputs(Some("shop"), None);
        f.core.rescope(&f.cluster, stores, &inputs, &mut f.host);

        assert_eq!(f.core.scope(&f.cluster), Some("shop"));
        let snapshot = f.core.snapshot(&f.cluster, &f.stores).unwrap();
        assert_eq!(snapshot.scope.as_deref(), Some("shop"));
        assert_eq!(snapshot.releases.len(), 1);
    }

    #[test]
    fn a_rekeyed_cluster_starts_over_and_unheld_clusters_are_swept() {
        let mut f = fixture();
        f.fill(None, vec![]);
        let lease = f.watch(None, None);
        f.core.rekeyed(&f.cluster, &mut f.host);
        assert!(!f.core.tracks(&f.cluster));
        drop(lease);

        // A cluster nobody holds goes once its time is up.
        f.fill(None, vec![]);
        drop(f.watch(None, None));
        f.core.sweep();
        assert!(f.core.tracks(&f.cluster), "still wanted for a while");
        f.core.clusters.get_mut(&f.cluster).unwrap().wanted_until = Instant::now();
        f.core.sweep();
        assert!(!f.core.tracks(&f.cluster));
    }
}
