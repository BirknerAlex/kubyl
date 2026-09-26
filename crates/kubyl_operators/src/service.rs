//! [`Olm`]: the app-wide OLM state. Per cluster it keeps the watches of the OLM objects while a
//! view (or phase 13's check) wants them, and serves a [`Snapshot`] joined from them; it also
//! caches OperatorHub's packages and icons, upgrade reviews, and runs the writes that outlive a
//! dialog (install, approve, uninstall).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Image, Task};
use kubyl_core::{ClusterId, Notification, NotificationCenter, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::{ObjectKey, ResourceStores, StoreHandle, StoreKey, StoreStatus};
use serde_json::Value;

use crate::olm::hub::{self, Package};
use crate::olm::join::{self, Operator};
use crate::olm::model::{
    self, COPIED_LABEL, CatalogSource, Csv, InstallPlan, OperatorGroup, Subscription,
};
use crate::olm::review::{self, ClusterFacts, Review};
use crate::olm::v1::{self, ClusterCatalog, ClusterExtension};

/// How long a cluster's watches stay after the last lease went and the API stopped asking.
const KEEP: Duration = Duration::from_secs(120);
const SWEEP: Duration = Duration::from_secs(30);
/// OperatorHub's packages are listed again after this while a view shows them.
pub const HUB_REFRESH: Duration = Duration::from_secs(600);
/// Reviews older than this are built again.
const REVIEW_TTL: Duration = Duration::from_secs(60);

/// What a cluster has of OLM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Not connected, or discovery hasn't run yet.
    NotConnected,
    Loading,
    /// Neither `operators.coreos.com` nor `olm.operatorframework.io`.
    NoOlm,
    Ready,
}

/// Everything the views show about one cluster's OLM, joined. Cheap to clone (Arcs).
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// OLM v0 (`operators.coreos.com`) is served.
    pub v0: bool,
    /// OLM v1 (`olm.operatorframework.io`) is served.
    pub v1: bool,
    pub subscriptions: Vec<Arc<Subscription>>,
    pub csvs: Vec<Arc<Csv>>,
    pub plans: Vec<Arc<InstallPlan>>,
    pub catalogs: Vec<Arc<CatalogSource>>,
    pub groups: Vec<OperatorGroup>,
    pub operators: Vec<Operator>,
    pub extensions: Vec<ClusterExtension>,
    pub cluster_catalogs: Vec<ClusterCatalog>,
    /// Some watch hasn't listed yet.
    pub loading: bool,
    /// Watches that can't list (403…): what's missing.
    pub problems: Vec<String>,
}

impl Snapshot {
    /// The operator of a key (`sub:ns/name`, `csv:ns/name`).
    pub fn operator(&self, key: &str) -> Option<&Operator> {
        self.operators.iter().find(|o| o.key == key)
    }

    pub fn plan(&self, namespace: &str, name: &str) -> Option<&Arc<InstallPlan>> {
        self.plans
            .iter()
            .find(|p| p.namespace == namespace && p.name == name)
    }

    /// The installed operator of a package (for OperatorHub's "Installed").
    pub fn installed(&self, package: &str, catalog: &str) -> Option<&Operator> {
        self.operators.iter().find(|o| {
            o.subscription.as_ref().is_some_and(|s| {
                s.package == package && (catalog.is_empty() || s.source == catalog)
            })
        })
    }

    /// Install plans that wait for approval.
    pub fn pending_plans(&self) -> usize {
        self.plans.iter().filter(|p| p.needs_approval()).count()
    }
}

/// Keeps a cluster's OLM watches running while held (views hold one).
#[derive(Clone)]
pub struct OlmLease(#[allow(dead_code)] Rc<()>);

struct Stores {
    subscriptions: Option<StoreHandle>,
    csvs: Option<StoreHandle>,
    plans: Option<StoreHandle>,
    catalogs: Option<StoreHandle>,
    groups: Option<StoreHandle>,
    extensions: Option<StoreHandle>,
    cluster_catalogs: Option<StoreHandle>,
}

impl Stores {
    fn all(&self) -> impl Iterator<Item = &StoreHandle> {
        [
            &self.subscriptions,
            &self.csvs,
            &self.plans,
            &self.catalogs,
            &self.groups,
            &self.extensions,
            &self.cluster_catalogs,
        ]
        .into_iter()
        .flatten()
    }
}

struct ClusterOlm {
    stores: Stores,
    v0: bool,
    v1: bool,
    lease: Rc<()>,
    wanted_until: Instant,
    /// The snapshot and the store generations it was built from.
    snapshot: RefCell<Option<(Vec<u64>, Arc<Snapshot>)>>,
    /// Parsed CSVs by object, reused while the object is unchanged (CSVs are big).
    csv_cache: RefCell<CsvCache>,
    _observers: Vec<gpui::Subscription>,
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
    task: Option<Task<()>>,
}

/// A package icon: loading, loaded, or none.
#[derive(Clone)]
pub enum Icon {
    Loading,
    Loaded(Arc<Image>),
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
    _task: Option<Task<()>>,
}

pub struct Olm {
    clusters: HashMap<ClusterId, ClusterOlm>,
    hub: HashMap<ClusterId, HubState>,
    icons: HashMap<(ClusterId, String), Icon>,
    icon_tasks: HashMap<(ClusterId, String), Task<()>>,
    reviews: HashMap<(ClusterId, String), ReviewEntry>,
    /// Writes in flight, by operator or plan key (the views show them busy).
    busy: HashSet<String>,
    tasks: Vec<Task<()>>,
    _sweep: Option<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
}

struct GlobalOlm(Entity<Olm>);

impl Global for GlobalOlm {}

impl Olm {
    /// Installs the global. `sweep`: drop unused watches periodically (off in GPUI tests).
    pub fn install(sweep: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this: &mut Self, _, event, cx| {
                    this.connection_event(event, cx)
                }));
            }
            let sweep = sweep.then(|| {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(SWEEP).await;
                        if this.update(cx, |this, cx| this.sweep(cx)).is_err() {
                            break;
                        }
                    }
                })
            });
            Self {
                clusters: HashMap::new(),
                hub: HashMap::new(),
                icons: HashMap::new(),
                icon_tasks: HashMap::new(),
                reviews: HashMap::new(),
                busy: HashSet::new(),
                tasks: Vec::new(),
                _sweep: sweep,
                _subscriptions: subscriptions,
            }
        });
        cx.set_global(GlobalOlm(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalOlm>().map(|g| g.0.clone())
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::DiscoveryChanged(id) => {
                // OLM may have been installed or removed: rebuild the watches.
                if let Some(state) = self.clusters.get(id) {
                    let (v0, v1) = served(id, cx);
                    if (v0, v1) != (state.v0, state.v1) {
                        let lease = state.lease.clone();
                        self.clusters.remove(id);
                        self.ensure(id, Some(lease), cx);
                    }
                }
                cx.notify();
            }
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                if !connected {
                    self.hub.remove(id);
                    self.reviews.retain(|(c, _), _| c != id);
                }
                cx.notify();
            }
            ConnectionEvent::Rekeyed { from, to } => {
                // The stores carry the old id: start over under the new one.
                if let Some(state) = self.clusters.remove(from) {
                    self.ensure(to, Some(state.lease), cx);
                }
                if let Some(hub) = self.hub.remove(from) {
                    self.hub.insert(to.clone(), hub);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    /// Keeps `cluster`'s watches while the returned lease lives.
    pub fn watch(cluster: &ClusterId, cx: &mut App) -> Option<OlmLease> {
        let olm = Self::global(cx)?;
        Some(olm.update(cx, |olm, cx| olm.lease(cluster, cx)))
    }

    fn lease(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) -> OlmLease {
        self.ensure(cluster, None, cx);
        OlmLease(self.clusters[cluster].lease.clone())
    }

    /// Starts the watches of `cluster` (if the cluster serves OLM) and marks it wanted.
    pub(crate) fn ensure(
        &mut self,
        cluster: &ClusterId,
        lease: Option<Rc<()>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.wanted_until = Instant::now() + KEEP;
            return;
        }
        let (v0, v1) = served(cluster, cx);
        let key = |gvr: kubyl_core::Gvr| StoreKey::new(cluster.clone(), gvr, None);
        let mut acquire = |key: StoreKey, on: bool| on.then(|| ResourceStores::acquire(cx, key));
        let stores = Stores {
            subscriptions: acquire(key(model::subscriptions()), v0),
            // OLM copies each CSV into every namespace its operator watches; the originals are
            // enough (and much smaller on clusters with many namespaces).
            csvs: acquire(key(model::csvs()).labels(format!("!{COPIED_LABEL}")), v0),
            plans: acquire(key(model::install_plans()), v0),
            catalogs: acquire(key(model::catalog_sources()), v0),
            groups: acquire(key(model::operator_groups()), v0),
            extensions: acquire(key(v1::cluster_extensions()), v1),
            cluster_catalogs: acquire(key(v1::cluster_catalogs()), v1),
        };
        let observers = stores
            .all()
            .map(|handle| cx.observe(handle.entity(), |_, _, cx| cx.notify()))
            .collect();
        self.clusters.insert(
            cluster.clone(),
            ClusterOlm {
                stores,
                v0,
                v1,
                lease: lease.unwrap_or_default(),
                wanted_until: Instant::now() + KEEP,
                snapshot: RefCell::new(None),
                csv_cache: RefCell::new(HashMap::new()),
                _observers: observers,
            },
        );
    }

    fn sweep(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let before = self.clusters.len();
        self.clusters
            .retain(|_, state| Rc::strong_count(&state.lease) > 1 || state.wanted_until > now);
        self.reviews
            .retain(|_, r| now.duration_since(r.at) < REVIEW_TTL * 5);
        if self.clusters.len() != before {
            cx.notify();
        }
    }

    /// What `cluster` has of OLM.
    pub fn availability(&self, cluster: &ClusterId, cx: &App) -> Availability {
        match ConnectionManager::try_global(cx) {
            Some(manager) => {
                let manager = manager.read(cx);
                if manager.client(cluster).is_none() {
                    return Availability::NotConnected;
                }
                let Some(discovery) = manager.discovery(cluster) else {
                    return Availability::Loading;
                };
                if !discovery.has_group(model::GROUP) && !discovery.has_group(v1::GROUP) {
                    return Availability::NoOlm;
                }
            }
            // Without connections (GPUI tests) only filled-in clusters exist.
            None if !self.clusters.contains_key(cluster) => return Availability::NotConnected,
            None => {}
        }
        match self.snapshot(cluster, cx) {
            Some(s) if !s.loading => Availability::Ready,
            _ => Availability::Loading,
        }
    }

    /// The joined state of `cluster`, if its watches run ([`Self::watch`]). Built again only
    /// when a watch changed.
    pub fn snapshot(&self, cluster: &ClusterId, cx: &App) -> Option<Arc<Snapshot>> {
        let state = self.clusters.get(cluster)?;
        let generations: Vec<u64> = state
            .stores
            .all()
            .map(|h| {
                let store = h.read(cx);
                store.generation() * 16 + status_code(store.status())
            })
            .collect();
        if let Some((gens, snapshot)) = state.snapshot.borrow().as_ref()
            && *gens == generations
        {
            return Some(snapshot.clone());
        }
        let snapshot = Arc::new(build(state, cx));
        *state.snapshot.borrow_mut() = Some((generations, snapshot.clone()));
        Some(snapshot)
    }

    /// Fills a cluster's watches by hand (GPUI tests: no cluster, no network).
    #[cfg(test)]
    pub(crate) fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        subscriptions: Vec<Value>,
        csvs: Vec<Value>,
        plans: Vec<Value>,
        cx: &mut Context<Self>,
    ) {
        use kubyl_resources::ResourceStore;
        let mut store = |gvr: kubyl_core::Gvr, objects: Vec<Value>| {
            let key = StoreKey::new(cluster.clone(), gvr, None);
            Some(StoreHandle::detached(
                cx.new(|_| ResourceStore::from_objects(key, objects)),
            ))
        };
        let stores = Stores {
            subscriptions: store(model::subscriptions(), subscriptions),
            csvs: store(model::csvs(), csvs),
            plans: store(model::install_plans(), plans),
            catalogs: store(model::catalog_sources(), Vec::new()),
            groups: store(model::operator_groups(), Vec::new()),
            extensions: None,
            cluster_catalogs: None,
        };
        self.clusters.insert(
            cluster.clone(),
            ClusterOlm {
                stores,
                v0: true,
                v1: false,
                lease: Rc::new(()),
                wanted_until: Instant::now() + KEEP,
                snapshot: RefCell::new(None),
                csv_cache: RefCell::new(HashMap::new()),
                _observers: Vec::new(),
            },
        );
        cx.notify();
    }

    // ----- OperatorHub -----

    /// OperatorHub's packages of `cluster`, fetched when missing or older than
    /// [`HUB_REFRESH`] (or when `force`).
    pub fn hub(&mut self, cluster: &ClusterId, force: bool, cx: &mut Context<Self>) -> &HubState {
        let state = self.hub.entry(cluster.clone()).or_default();
        let stale = state.fetched_at.is_none_or(|at| at.elapsed() > HUB_REFRESH);
        if (stale || force) && !state.loading {
            let client = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster));
            if let Some(client) = client {
                state.loading = true;
                state.error = None;
                let fetch = spawn_kube(cx, hub::fetch(client));
                let cluster = cluster.clone();
                state.task = Some(cx.spawn(async move |this, cx| {
                    let result = fetch.await;
                    this.update(cx, |this, cx| {
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
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        &self.hub[cluster]
    }

    /// The cached hub state, without fetching.
    pub fn hub_state(&self, cluster: &ClusterId) -> Option<&HubState> {
        self.hub.get(cluster)
    }

    /// A package's icon from the packageserver; loads it on first ask.
    pub fn icon(&mut self, cluster: &ClusterId, package: &Package, cx: &mut Context<Self>) -> Icon {
        let key = (cluster.clone(), package.key());
        if let Some(icon) = self.icons.get(&key) {
            return icon.clone();
        }
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster))
        else {
            return Icon::None;
        };
        self.icons.insert(key.clone(), Icon::Loading);
        // Finished fetches (a task never drops itself: that would cancel it).
        self.icon_tasks.retain(|_, task| !task.is_ready());
        let fetch = spawn_kube(
            cx,
            hub::fetch_icon(client, package.namespace.clone(), package.name.clone()),
        );
        let task_key = key.clone();
        self.icon_tasks.insert(
            task_key,
            cx.spawn(async move |this, cx| {
                let result = fetch.await;
                this.update(cx, |this, cx| {
                    let icon = match result {
                        Some((format, bytes)) => {
                            Icon::Loaded(Arc::new(Image::from_bytes(format, bytes)))
                        }
                        None => Icon::None,
                    };
                    this.icons.insert(key.clone(), icon);
                    cx.notify();
                })
                .ok();
            }),
        );
        Icon::Loading
    }

    // ----- Reviews -----

    /// The review of a pending plan, built on first ask.
    pub fn review(
        &mut self,
        cluster: &ClusterId,
        plan: &Arc<InstallPlan>,
        installed: Option<Arc<Csv>>,
        cx: &mut Context<Self>,
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
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return ReviewState::Loading;
        };
        let (client, facts, openshift) = {
            let manager = manager.read(cx);
            let facts = ClusterFacts {
                kube_version: manager
                    .cluster(cluster)
                    .and_then(|c| c.info.as_ref())
                    .map(|i| i.version.clone()),
                openshift_version: None,
            };
            (
                manager.client(cluster),
                facts,
                manager.caps(cluster).openshift,
            )
        };
        let Some(client) = client else {
            return ReviewState::Loading;
        };
        let work = spawn_kube(
            cx,
            review::review(
                client,
                (**plan).clone(),
                installed.map(|c| (*c).clone()),
                facts,
                openshift,
            ),
        );
        let task_key = key.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                // The task stays in the entry (a task must not drop itself).
                if let Some(entry) = this.reviews.get_mut(&task_key) {
                    entry.state = ReviewState::Ready(Arc::new(result));
                }
                cx.notify();
            })
            .ok();
        });
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
    /// The task belongs to the service: closing the dialog doesn't cancel it.
    pub fn run(
        &mut self,
        key: String,
        work: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
        success: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        self.busy.insert(key.clone());
        cx.notify();
        let task = spawn_kube(cx, work);
        let (done_tx, done_rx) = futures::channel::oneshot::channel();
        let runner = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy.remove(&key);
                match &result {
                    Ok(()) => NotificationCenter::push(cx, Notification::success(success)),
                    Err(err) => NotificationCenter::push(cx, Notification::error(err.clone())),
                }
                cx.notify();
            })
            .ok();
            done_tx.send(result).ok();
        });
        self.tasks.retain(|t| !t.is_ready());
        self.tasks.push(runner);
        cx.background_executor().spawn(async move {
            done_rx
                .await
                .unwrap_or_else(|_| Err("Cancelled.".to_string()))
        })
    }
}

fn status_code(status: &StoreStatus) -> u64 {
    match status {
        StoreStatus::Waiting => 0,
        StoreStatus::Loading => 1,
        StoreStatus::Ready => 2,
        StoreStatus::Forbidden => 3,
        StoreStatus::Unsupported => 4,
        StoreStatus::Error(_) => 5,
        StoreStatus::Paused => 6,
    }
}

/// Whether the cluster serves OLM v0 and v1.
fn served(cluster: &ClusterId, cx: &App) -> (bool, bool) {
    ConnectionManager::try_global(cx)
        .and_then(|m| m.read(cx).discovery(cluster))
        .map(|d| (d.has_group(model::GROUP), d.has_group(v1::GROUP)))
        .unwrap_or((false, false))
}

fn build(state: &ClusterOlm, cx: &App) -> Snapshot {
    let mut snapshot = Snapshot {
        v0: state.v0,
        v1: state.v1,
        ..Default::default()
    };
    let mut check = |handle: &StoreHandle, what: &str| {
        let store = handle.read(cx);
        match store.status() {
            StoreStatus::Waiting | StoreStatus::Loading => snapshot.loading = true,
            StoreStatus::Forbidden => snapshot.problems.push(format!(
                "Forbidden: you can't list {what} cluster-wide. Ask for a role with `list` and `watch` on `{what}`."
            )),
            StoreStatus::Error(err) if !store.status().is_settled() || store.is_empty() => {
                snapshot.problems.push(format!("{what}: {err}"))
            }
            _ => {}
        }
    };
    let s = &state.stores;
    for (handle, what) in [
        (&s.subscriptions, "subscriptions"),
        (&s.csvs, "clusterserviceversions"),
        (&s.plans, "installplans"),
        (&s.catalogs, "catalogsources"),
        (&s.groups, "operatorgroups"),
        (&s.extensions, "clusterextensions"),
        (&s.cluster_catalogs, "clustercatalogs"),
    ] {
        if let Some(handle) = handle {
            check(handle, what);
        }
    }
    let values = |handle: &Option<StoreHandle>| -> Vec<Arc<Value>> {
        handle
            .as_ref()
            .map(|h| h.read(cx).objects().values().cloned().collect())
            .unwrap_or_default()
    };
    snapshot.subscriptions = values(&s.subscriptions)
        .iter()
        .filter_map(|v| Subscription::parse(v).map(Arc::new))
        .collect();
    {
        let mut cache = state.csv_cache.borrow_mut();
        let mut next = HashMap::new();
        if let Some(handle) = &s.csvs {
            for (key, value) in handle.read(cx).objects() {
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
    snapshot.plans = values(&s.plans)
        .iter()
        .filter_map(|v| InstallPlan::parse(v).map(Arc::new))
        .collect();
    snapshot.plans.sort_by(|a, b| {
        b.needs_approval()
            .cmp(&a.needs_approval())
            .then(b.created.cmp(&a.created))
            .then(a.name.cmp(&b.name))
    });
    snapshot.catalogs = values(&s.catalogs)
        .iter()
        .filter_map(|v| CatalogSource::parse(v).map(Arc::new))
        .collect();
    snapshot
        .catalogs
        .sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    snapshot.groups = values(&s.groups)
        .iter()
        .filter_map(|v| OperatorGroup::parse(v))
        .collect();
    snapshot
        .subscriptions
        .sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    snapshot.operators = join::join(&snapshot.subscriptions, &snapshot.csvs, &snapshot.plans);
    snapshot.extensions = values(&s.extensions)
        .iter()
        .filter_map(|v| ClusterExtension::parse(v))
        .collect();
    snapshot.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    snapshot.cluster_catalogs = values(&s.cluster_catalogs)
        .iter()
        .filter_map(|v| ClusterCatalog::parse(v))
        .collect();
    snapshot
        .cluster_catalogs
        .sort_by(|a, b| a.name.cmp(&b.name));
    snapshot
}
