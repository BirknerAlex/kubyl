//! [`OlmCore`]: the OLM state of each cluster, on any [`Host`]. Per cluster it keeps the
//! watches of the OLM objects while a view (or another feature's check) wants them, and serves a
//! [`Snapshot`] joined from them; it also caches OperatorHub's packages and icons, upgrade
//! reviews, and runs the writes that outlive a dialog (install, approve, uninstall).
//!
//! The service doesn't own watches. The app acquires them with [`OlmStores::acquire`] (through
//! a [`StoreSource`]), hands the leases to [`OlmCore::watch`], and reads snapshots with a
//! [`StoreReader`]. What it reads from a cluster's connection comes in as an [`OlmConn`].

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use futures::channel::oneshot;
use kubyl_base::host::{Flow, Host, HostExt as _, Service, TaskHandle};
use kubyl_base::{ClusterId, Gvr, Notice};
use kubyl_resources_core::source::{StoreLease, StoreReader, StoreSource};
use kubyl_resources_core::store::{ObjectKey, StoreKey, StoreStatus};
use serde_json::Value;

use super::hub::{self, IconFormat, Package};
use super::join;
use super::model::{
    self, COPIED_LABEL, CatalogSource, Csv, InstallPlan, OperatorGroup, Subscription,
};
use super::review::{self, ClusterFacts, Review};
pub use super::snapshot::*;
use super::v1::{self, ClusterCatalog, ClusterExtension};

/// Keeps a cluster's OLM watches running while held (views hold one).
#[derive(Clone)]
pub struct OlmLease(#[allow(dead_code)] Arc<()>);

/// The OLM objects a cluster's watches cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Subscriptions,
    Csvs,
    Plans,
    Catalogs,
    Groups,
    Extensions,
    ClusterCatalogs,
}

impl Kind {
    const ALL: [Kind; 7] = [
        Kind::Subscriptions,
        Kind::Csvs,
        Kind::Plans,
        Kind::Catalogs,
        Kind::Groups,
        Kind::Extensions,
        Kind::ClusterCatalogs,
    ];

    /// The resource, for messages about its watch.
    fn what(self) -> &'static str {
        match self {
            Kind::Subscriptions => "subscriptions",
            Kind::Csvs => "clusterserviceversions",
            Kind::Plans => "installplans",
            Kind::Catalogs => "catalogsources",
            Kind::Groups => "operatorgroups",
            Kind::Extensions => "clusterextensions",
            Kind::ClusterCatalogs => "clustercatalogs",
        }
    }

    /// Whether OLM v1 serves it (else v0).
    fn v1(self) -> bool {
        matches!(self, Kind::Extensions | Kind::ClusterCatalogs)
    }

    fn gvr(self) -> Gvr {
        match self {
            Kind::Subscriptions => model::subscriptions(),
            Kind::Csvs => model::csvs(),
            Kind::Plans => model::install_plans(),
            Kind::Catalogs => model::catalog_sources(),
            Kind::Groups => model::operator_groups(),
            Kind::Extensions => v1::cluster_extensions(),
            Kind::ClusterCatalogs => v1::cluster_catalogs(),
        }
    }

    /// The key of the watch of this kind on `cluster`.
    pub fn key(self, cluster: &ClusterId) -> StoreKey {
        let key = StoreKey::new(cluster.clone(), self.gvr(), None);
        match self {
            // OLM copies each CSV into every namespace its operator watches; the originals are
            // enough (and much smaller on clusters with many namespaces).
            Kind::Csvs => key.labels(format!("!{COPIED_LABEL}")),
            _ => key,
        }
    }
}

/// The watches of one cluster, leased from the app.
pub struct OlmStores {
    leases: Vec<(Kind, StoreLease)>,
}

impl OlmStores {
    /// The kinds a cluster that serves OLM v0 and/or v1 is watched for.
    pub fn kinds(v0: bool, v1: bool) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|kind| if kind.v1() { v1 } else { v0 })
            .collect()
    }

    /// Acquires the watches of a cluster serving OLM v0 and/or v1 from `source`.
    pub fn acquire(cluster: &ClusterId, v0: bool, v1: bool, source: &mut dyn StoreSource) -> Self {
        Self {
            leases: Self::kinds(v0, v1)
                .into_iter()
                .map(|kind| (kind, source.acquire(kind.key(cluster))))
                .collect(),
        }
    }

    /// The keys of the stores, for the app to observe.
    pub fn keys(&self) -> Vec<StoreKey> {
        self.leases
            .iter()
            .map(|(_, lease)| lease.key().clone())
            .collect()
    }
}

/// What the service reads from a cluster's connection.
#[derive(Clone, Default)]
pub struct OlmConn {
    /// `None`: the cluster isn't connected.
    pub client: Option<kube::Client>,
    /// Whether the cluster serves OLM v0 and v1; `None` while discovery hasn't run.
    pub served: Option<(bool, bool)>,
    /// `v1.30.4`.
    pub kube_version: Option<String>,
    /// The cluster is OpenShift.
    pub openshift: bool,
}

struct ClusterOlm {
    stores: OlmStores,
    v0: bool,
    v1: bool,
    /// Whether discovery was known when the watches started: until then the (empty) snapshot is
    /// loading, not a cluster without operators.
    discovered: bool,
    lease: Arc<()>,
    wanted_until: Instant,
    /// The snapshot and the store generations it was built from.
    snapshot: RefCell<Option<(Vec<u64>, Arc<Snapshot>)>>,
    /// Parsed CSVs by object, reused while the object is unchanged (CSVs are big).
    csv_cache: RefCell<CsvCache>,
}

/// A CSV object and what it parsed into.
type CsvCache = HashMap<ObjectKey, (Arc<Value>, Arc<Csv>)>;

/// OperatorHub's packages of a cluster.
#[derive(Default)]
pub struct HubState {
    pub packages: Option<Arc<Vec<Arc<Package>>>>,
    pub fetched_at: Option<Instant>,
    pub error: Option<String>,
    pub loading: bool,
    task: Option<TaskHandle>,
}

/// A package icon: loading, loaded, or none.
#[derive(Clone)]
pub enum IconState {
    Loading,
    Loaded {
        format: IconFormat,
        bytes: Arc<Vec<u8>>,
    },
    None,
}

/// An upgrade review of a plan.
#[derive(Clone)]
pub enum ReviewState {
    Loading,
    Ready(Arc<Review>),
}

struct ReviewEntry {
    state: ReviewState,
    at: Instant,
    /// Stays here (a task must not drop itself).
    _task: Option<TaskHandle>,
}

/// The OLM state of every cluster a view asked about.
pub struct OlmCore {
    clusters: HashMap<ClusterId, ClusterOlm>,
    hub: HashMap<ClusterId, HubState>,
    icons: HashMap<(ClusterId, String), IconState>,
    reviews: HashMap<(ClusterId, String), ReviewEntry>,
    /// Writes in flight, by operator or plan key (the views show them busy).
    busy: HashSet<String>,
    sweep: Option<TaskHandle>,
}

impl Service for OlmCore {
    type Event = Infallible;
    type Effect = Infallible;
}

impl Default for OlmCore {
    fn default() -> Self {
        Self::new()
    }
}

impl OlmCore {
    pub fn new() -> Self {
        Self {
            clusters: HashMap::new(),
            hub: HashMap::new(),
            icons: HashMap::new(),
            reviews: HashMap::new(),
            busy: HashSet::new(),
            sweep: None,
        }
    }

    /// Drops unused watches periodically (off in tests that must not wake timers).
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        self.sweep = Some(host.every(SWEEP, |this, host| {
            this.sweep_at(Instant::now(), host);
            Flow::Continue
        }));
    }

    /// Drops the watches nobody holds or asked for lately, and old reviews.
    pub fn sweep_at(&mut self, now: Instant, host: &mut dyn Host<Self>) {
        let before = self.clusters.len();
        self.clusters
            .retain(|_, state| Arc::strong_count(&state.lease) > 1 || state.wanted_until > now);
        self.reviews
            .retain(|_, r| now.duration_since(r.at) < REVIEW_TTL * 5);
        if self.clusters.len() != before {
            host.notify();
        }
    }

    /// Whether the service follows `cluster` (the app then holds its watches).
    pub fn tracks(&self, cluster: &ClusterId) -> bool {
        self.clusters.contains_key(cluster)
    }

    /// Keeps `cluster`'s watches while the returned lease lives. `stores` are what the app
    /// acquired when [`Self::tracks`] said no (for what `served` says the cluster has);
    /// otherwise `None`.
    pub fn watch(
        &mut self,
        cluster: &ClusterId,
        stores: Option<OlmStores>,
        served: Option<(bool, bool)>,
    ) -> Option<OlmLease> {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.wanted_until = Instant::now() + KEEP;
            return Some(OlmLease(state.lease.clone()));
        }
        let stores = stores?;
        let lease = Arc::new(());
        self.insert(cluster, stores, served, lease.clone());
        Some(OlmLease(lease))
    }

    fn insert(
        &mut self,
        cluster: &ClusterId,
        stores: OlmStores,
        served: Option<(bool, bool)>,
        lease: Arc<()>,
    ) {
        let (v0, v1) = served.unwrap_or_default();
        self.clusters.insert(
            cluster.clone(),
            ClusterOlm {
                stores,
                v0,
                v1,
                discovered: served.is_some(),
                lease,
                wanted_until: Instant::now() + KEEP,
                snapshot: RefCell::new(None),
                csv_cache: RefCell::new(HashMap::new()),
            },
        );
    }

    /// Whether the watches of `cluster` no longer fit what it serves (OLM was installed or
    /// removed): the app then acquires new ones for [`Self::rewatch`].
    pub fn needs_rewatch(&self, cluster: &ClusterId, served: Option<(bool, bool)>) -> bool {
        self.clusters.get(cluster).is_some_and(|state| {
            served.is_some() != state.discovered
                || served.unwrap_or_default() != (state.v0, state.v1)
        })
    }

    /// Replaces the watches of `cluster` (keeping its lease).
    pub fn rewatch(
        &mut self,
        cluster: &ClusterId,
        stores: OlmStores,
        served: Option<(bool, bool)>,
        host: &mut dyn Host<Self>,
    ) {
        if let Some(old) = self.clusters.remove(cluster) {
            self.insert(cluster, stores, served, old.lease);
        }
        host.notify();
    }

    /// [`Self::watch`] or [`Self::rewatch`], whichever fits (tests of views fill stores by hand).
    pub fn rewatch_or_watch(
        &mut self,
        cluster: &ClusterId,
        stores: OlmStores,
        served: Option<(bool, bool)>,
        host: &mut dyn Host<Self>,
    ) {
        if self.tracks(cluster) {
            self.rewatch(cluster, stores, served, host);
        } else {
            self.watch(cluster, Some(stores), served);
            host.notify();
        }
    }

    /// Fills a cluster's OperatorHub packages by hand (tests of views: nothing is fetched).
    pub fn seed_hub(&mut self, cluster: &ClusterId, packages: Vec<Package>) {
        let state = self.hub.entry(cluster.clone()).or_default();
        state.packages = Some(Arc::new(packages.into_iter().map(Arc::new).collect()));
        state.fetched_at = Some(Instant::now());
    }

    /// A cluster disconnected: what was fetched for it goes.
    pub fn disconnected(&mut self, cluster: &ClusterId, host: &mut dyn Host<Self>) {
        self.hub.remove(cluster);
        self.reviews.retain(|(c, _), _| c != cluster);
        host.notify();
    }

    /// A cluster's id changed. Returns whether it was followed: the stores carry the old id, so
    /// the app acquires new ones for [`Self::rewatch`].
    pub fn rekeyed(&mut self, from: &ClusterId, to: &ClusterId, host: &mut dyn Host<Self>) -> bool {
        let followed = match self.clusters.remove(from) {
            Some(state) => {
                self.clusters.insert(to.clone(), state);
                true
            }
            None => false,
        };
        rekey_hub(&mut self.hub, from, to);
        host.notify();
        followed
    }

    /// A watch of a followed cluster changed (views re-render).
    pub fn changed(&mut self, host: &mut dyn Host<Self>) {
        host.notify();
    }

    /// What `cluster` has of OLM. `conn` is `None` without connections (tests): only
    /// filled-in clusters exist then.
    pub fn availability(
        &self,
        cluster: &ClusterId,
        conn: Option<&OlmConn>,
        stores: &dyn StoreReader,
    ) -> Availability {
        match conn {
            Some(conn) => {
                if conn.client.is_none() {
                    return Availability::NotConnected;
                }
                match conn.served {
                    None => return Availability::Loading,
                    Some((false, false)) => return Availability::NoOlm,
                    Some(_) => {}
                }
            }
            None if !self.clusters.contains_key(cluster) => return Availability::NotConnected,
            None => {}
        }
        match self.snapshot(cluster, stores) {
            Some(s) if !s.loading => Availability::Ready,
            _ => Availability::Loading,
        }
    }

    /// The joined state of `cluster`, if its watches run ([`Self::watch`]). Built again only
    /// when a watch changed.
    pub fn snapshot(&self, cluster: &ClusterId, stores: &dyn StoreReader) -> Option<Arc<Snapshot>> {
        let state = self.clusters.get(cluster)?;
        let generations: Vec<u64> = state
            .stores
            .leases
            .iter()
            .map(|(kind, _)| match stores.read(&kind.key(cluster)) {
                Some(store) => store.generation() * 16 + status_code(store.status()),
                None => 0,
            })
            .collect();
        if let Some((gens, snapshot)) = state.snapshot.borrow().as_ref()
            && *gens == generations
        {
            return Some(snapshot.clone());
        }
        let snapshot = Arc::new(build(cluster, state, stores));
        *state.snapshot.borrow_mut() = Some((generations, snapshot.clone()));
        Some(snapshot)
    }

    // ----- OperatorHub -----

    /// OperatorHub's packages of `cluster`, fetched when missing or older than [`HUB_REFRESH`]
    /// (or when `force`).
    pub fn hub(
        &mut self,
        cluster: &ClusterId,
        force: bool,
        conn: &OlmConn,
        host: &mut dyn Host<Self>,
    ) -> &HubState {
        let state = self.hub.entry(cluster.clone()).or_default();
        let stale = state.fetched_at.is_none_or(|at| at.elapsed() > HUB_REFRESH);
        if (stale || force)
            && !state.loading
            && let Some(client) = conn.client.clone()
        {
            state.loading = true;
            state.error = None;
            let cluster = cluster.clone();
            state.task = Some(host.spawn(hub::fetch(client), {
                let cluster = cluster.clone();
                move |this, result, host| {
                    let state = this.hub.entry(cluster).or_default();
                    state.loading = false;
                    state.fetched_at = Some(Instant::now());
                    match result {
                        Ok(packages) => {
                            state.packages = Some(Arc::new(packages));
                            state.error = None;
                        }
                        Err(err) => state.error = Some(err),
                    }
                    host.notify();
                }
            }));
        }
        &self.hub[cluster]
    }

    /// The cached hub state, without fetching.
    pub fn hub_state(&self, cluster: &ClusterId) -> Option<&HubState> {
        self.hub.get(cluster)
    }

    /// A package's icon from the packageserver; loads it on first ask.
    pub fn icon(
        &mut self,
        cluster: &ClusterId,
        package: &Package,
        conn: &OlmConn,
        host: &mut dyn Host<Self>,
    ) -> IconState {
        let key = (cluster.clone(), package.key());
        if let Some(icon) = self.icons.get(&key) {
            return icon.clone();
        }
        let Some(client) = conn.client.clone() else {
            return IconState::None;
        };
        self.icons.insert(key.clone(), IconState::Loading);
        // A fetch for a cluster that went away still reports; it only fills the cache.
        host.spawn(
            hub::fetch_icon(client, package.namespace.clone(), package.name.clone()),
            move |this, result, host| {
                let icon = match result {
                    Some((format, bytes)) => IconState::Loaded {
                        format,
                        bytes: Arc::new(bytes),
                    },
                    None => IconState::None,
                };
                this.icons.insert(key, icon);
                host.notify();
            },
        )
        .detach();
        IconState::Loading
    }

    // ----- Reviews -----

    /// The review of a pending plan, built on first ask.
    pub fn review(
        &mut self,
        cluster: &ClusterId,
        plan: &Arc<InstallPlan>,
        installed: Option<Arc<Csv>>,
        conn: &OlmConn,
        host: &mut dyn Host<Self>,
    ) -> ReviewState {
        let key = (
            cluster.clone(),
            format!(
                "{}/{}/{}",
                plan.namespace,
                plan.name,
                plan.csv_names.join(",")
            ),
        );
        if let Some(entry) = self.reviews.get(&key)
            && entry.at.elapsed() < REVIEW_TTL
        {
            return entry.state.clone();
        }
        let Some(client) = conn.client.clone() else {
            return ReviewState::Loading;
        };
        let facts = ClusterFacts {
            kube_version: conn.kube_version.clone(),
            openshift_version: None,
        };
        let task_key = key.clone();
        let task = host.spawn(
            review::review(
                client,
                (**plan).clone(),
                installed.map(|c| (*c).clone()),
                facts,
                conn.openshift,
            ),
            move |this, result, host| {
                // The task stays in the entry (a task must not drop itself).
                if let Some(entry) = this.reviews.get_mut(&task_key) {
                    entry.state = ReviewState::Ready(Arc::new(result));
                }
                host.notify();
            },
        );
        self.reviews.insert(
            key,
            ReviewEntry {
                state: ReviewState::Loading,
                at: Instant::now(),
                _task: Some(task),
            },
        );
        ReviewState::Loading
    }

    // ----- Writes -----

    pub fn is_busy(&self, key: &str) -> bool {
        self.busy.contains(key)
    }

    /// Runs a write on Tokio, marks `key` busy meanwhile and reports the outcome as a toast.
    /// The write belongs to the service: closing the dialog doesn't cancel it. The answer
    /// arrives on the returned channel.
    pub fn run(
        &mut self,
        key: String,
        work: impl Future<Output = Result<(), String>> + Send + 'static,
        success: String,
        host: &mut dyn Host<Self>,
    ) -> oneshot::Receiver<Result<(), String>> {
        self.busy.insert(key.clone());
        host.notify();
        let (done, answer) = oneshot::channel();
        host.spawn(work, move |this, result, host| {
            this.busy.remove(&key);
            match &result {
                Ok(()) => host.toast(Notice::success(success)),
                Err(err) => host.toast(Notice::error(err.clone())),
            }
            host.notify();
            done.send(result).ok();
        })
        .detach();
        answer
    }
}

/// Moves a finished hub fetch to the new id. One in flight is dropped (with its task, which
/// would report under the old id): the next ask fetches again.
fn rekey_hub(hub: &mut HashMap<ClusterId, HubState>, from: &ClusterId, to: &ClusterId) {
    if let Some(state) = hub.remove(from)
        && !state.loading
    {
        hub.insert(to.clone(), state);
    }
}

fn build(cluster: &ClusterId, state: &ClusterOlm, stores: &dyn StoreReader) -> Snapshot {
    let mut snapshot = Snapshot {
        v0: state.v0,
        v1: state.v1,
        loading: !state.discovered,
        ..Default::default()
    };
    let held = |kind: Kind| state.stores.leases.iter().any(|(k, _)| *k == kind);
    let read = |kind: Kind| {
        held(kind)
            .then(|| stores.read(&kind.key(cluster)))
            .flatten()
    };
    let watches: Vec<(&str, StoreStatus, bool)> = Kind::ALL
        .into_iter()
        .filter_map(|kind| {
            let store = read(kind)?;
            Some((
                kind.what(),
                store.status().clone(),
                store.objects().is_empty(),
            ))
        })
        .collect();
    let watched = watch_problems(&watches);
    snapshot.loading |= watched.loading;
    snapshot.problems = watched.problems;
    snapshot.operators_problem = watched.operators;
    let values = |kind: Kind| -> Vec<Arc<Value>> {
        read(kind)
            .map(|store| store.objects().values().cloned().collect())
            .unwrap_or_default()
    };
    snapshot.subscriptions = values(Kind::Subscriptions)
        .iter()
        .filter_map(|v| Subscription::parse(v).map(Arc::new))
        .collect();
    {
        let mut cache = state.csv_cache.borrow_mut();
        let mut next = HashMap::new();
        if let Some(store) = read(Kind::Csvs) {
            for (key, value) in store.objects() {
                let parsed = match cache.get(key) {
                    Some((old, parsed)) if Arc::ptr_eq(old, value) => Some(parsed.clone()),
                    _ => Csv::parse(value).map(Arc::new),
                };
                if let Some(parsed) = parsed {
                    next.insert(key.clone(), (value.clone(), parsed));
                }
            }
        }
        snapshot.csvs = next.values().map(|(_, c)| c.clone()).collect();
        *cache = next;
    }
    snapshot
        .csvs
        .sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    snapshot.plans = values(Kind::Plans)
        .iter()
        .filter_map(|v| InstallPlan::parse(v).map(Arc::new))
        .collect();
    snapshot.plans.sort_by(|a, b| {
        b.needs_approval()
            .cmp(&a.needs_approval())
            .then(b.created.cmp(&a.created))
            .then(a.name.cmp(&b.name))
    });
    snapshot.catalogs = values(Kind::Catalogs)
        .iter()
        .filter_map(|v| CatalogSource::parse(v).map(Arc::new))
        .collect();
    snapshot
        .catalogs
        .sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    snapshot.groups = values(Kind::Groups)
        .iter()
        .filter_map(|v| OperatorGroup::parse(v))
        .collect();
    snapshot
        .subscriptions
        .sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    snapshot.operators = join::join(&snapshot.subscriptions, &snapshot.csvs, &snapshot.plans);
    snapshot.extensions = values(Kind::Extensions)
        .iter()
        .filter_map(|v| ClusterExtension::parse(v))
        .collect();
    snapshot.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    snapshot.cluster_catalogs = values(Kind::ClusterCatalogs)
        .iter()
        .filter_map(|v| ClusterCatalog::parse(v))
        .collect();
    snapshot
        .cluster_catalogs
        .sort_by(|a, b| a.name.cmp(&b.name));
    snapshot
}

#[cfg(test)]
mod tests {
    use kubyl_base::host::TestHost;
    use kubyl_resources_core::source::MemoryStores;
    use serde_json::json;

    use super::*;

    fn subscription(name: &str) -> Value {
        json!({"metadata": {"namespace": "operators", "name": name},
            "spec": {"name": name, "source": "community", "sourceNamespace": "olm",
                "channel": "stable", "installPlanApproval": "Automatic"}})
    }

    struct Fixture {
        core: OlmCore,
        host: TestHost<OlmCore>,
        stores: MemoryStores,
        cluster: ClusterId,
    }

    fn fixture() -> Fixture {
        Fixture {
            core: OlmCore::new(),
            host: TestHost::new(),
            stores: MemoryStores::default(),
            cluster: ClusterId::new("c"),
        }
    }

    impl Fixture {
        /// A cluster serving OLM v0 whose subscriptions are `names`.
        fn watched(&mut self, names: &[&str]) -> OlmLease {
            self.stores.set(
                Kind::Subscriptions.key(&self.cluster),
                names.iter().map(|n| subscription(n)),
            );
            for kind in [Kind::Csvs, Kind::Plans, Kind::Catalogs, Kind::Groups] {
                self.stores.set(kind.key(&self.cluster), []);
            }
            let served = Some((true, false));
            let stores = OlmStores::acquire(&self.cluster, true, false, &mut self.stores);
            self.core
                .watch(&self.cluster, Some(stores), served)
                .expect("a lease")
        }
    }

    #[test]
    fn a_followed_cluster_serves_the_snapshot_of_its_watches() {
        let mut f = fixture();
        assert!(f.core.snapshot(&f.cluster, &f.stores).is_none());

        let lease = f.watched(&["cert-manager", "cnpg"]);
        assert!(f.core.tracks(&f.cluster));
        let snapshot = f.core.snapshot(&f.cluster, &f.stores).unwrap();
        assert_eq!(snapshot.subscriptions.len(), 2);
        assert!(!snapshot.loading);
        assert!(snapshot.v0 && !snapshot.v1);

        // The same generation gives the same snapshot; a change builds a new one.
        let again = f.core.snapshot(&f.cluster, &f.stores).unwrap();
        assert!(Arc::ptr_eq(&snapshot, &again));
        f.stores
            .set(Kind::Subscriptions.key(&f.cluster), [subscription("cnpg")]);
        assert_eq!(
            f.core
                .snapshot(&f.cluster, &f.stores)
                .unwrap()
                .subscriptions
                .len(),
            1
        );

        let conn = OlmConn {
            client: None,
            served: Some((true, false)),
            ..Default::default()
        };
        assert_eq!(
            f.core.availability(&f.cluster, Some(&conn), &f.stores),
            Availability::NotConnected
        );
        assert_eq!(
            f.core.availability(&f.cluster, None, &f.stores),
            Availability::Ready
        );
        drop(lease);
    }

    #[test]
    fn watches_follow_what_the_cluster_serves() {
        let mut f = fixture();
        let _lease = f.watched(&[]);
        assert!(!f.core.needs_rewatch(&f.cluster, Some((true, false))));
        // OLM v1 was installed too.
        assert!(f.core.needs_rewatch(&f.cluster, Some((true, true))));

        for kind in [Kind::Extensions, Kind::ClusterCatalogs] {
            f.stores.set(kind.key(&f.cluster), []);
        }
        let stores = OlmStores::acquire(&f.cluster, true, true, &mut f.stores);
        f.core
            .rewatch(&f.cluster, stores, Some((true, true)), &mut f.host);
        assert!(!f.core.needs_rewatch(&f.cluster, Some((true, true))));
        assert!(f.core.snapshot(&f.cluster, &f.stores).unwrap().v1);
    }

    #[test]
    fn a_rekeyed_cluster_moves_its_state_and_asks_for_new_watches() {
        let mut f = fixture();
        let _lease = f.watched(&[]);
        let to = ClusterId::new("renamed");
        assert!(f.core.rekeyed(&f.cluster, &to, &mut f.host));
        assert!(f.core.tracks(&to) && !f.core.tracks(&f.cluster));
        assert!(!f.core.rekeyed(&f.cluster, &to, &mut f.host));
    }

    #[test]
    fn unheld_clusters_and_old_reviews_are_swept() {
        let mut f = fixture();
        drop(f.watched(&[]));
        f.core.sweep_at(Instant::now(), &mut f.host);
        assert!(f.core.tracks(&f.cluster), "wanted for a while");
        f.core.sweep_at(Instant::now() + KEEP * 2, &mut f.host);
        assert!(!f.core.tracks(&f.cluster));
    }

    #[test]
    fn a_write_is_busy_until_it_finishes_and_tells_the_result() {
        let mut f = fixture();

        let mut ok = f.core.run(
            "uninstall:sub:operators/cnpg".into(),
            async { Ok(()) },
            "Uninstalled cnpg".into(),
            &mut f.host,
        );
        assert!(f.core.is_busy("uninstall:sub:operators/cnpg"));
        f.host.run_until_idle(&mut f.core);
        assert!(!f.core.is_busy("uninstall:sub:operators/cnpg"));
        assert_eq!(ok.try_recv().unwrap(), Some(Ok(())));

        let mut failed = f.core.run(
            "plan:x".into(),
            async { Err("forbidden".to_string()) },
            "Approved".into(),
            &mut f.host,
        );
        f.host.run_until_idle(&mut f.core);
        assert_eq!(
            failed.try_recv().unwrap(),
            Some(Err("forbidden".to_string()))
        );
        let messages: Vec<&str> = f.host.notices.iter().map(|n| n.message.as_str()).collect();
        assert_eq!(messages, ["Uninstalled cnpg", "forbidden"]);
    }

    #[test]
    fn rekey_moves_finished_hub_and_drops_one_in_flight() {
        let (from, to) = (ClusterId::new("a"), ClusterId::new("b"));
        let mut hub = HashMap::new();
        hub.insert(
            from.clone(),
            HubState {
                fetched_at: Some(Instant::now()),
                ..Default::default()
            },
        );
        rekey_hub(&mut hub, &from, &to);
        assert!(!hub.contains_key(&from) && hub[&to].fetched_at.is_some());

        let mut hub = HashMap::new();
        hub.insert(
            from.clone(),
            HubState {
                loading: true,
                ..Default::default()
            },
        );
        rekey_hub(&mut hub, &from, &to);
        assert!(
            hub.is_empty(),
            "a fetch in flight must restart, not stay loading"
        );
    }
}
