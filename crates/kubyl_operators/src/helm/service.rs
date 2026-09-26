//! [`Helm`]: the Helm releases of each cluster a view shows.
//!
//! Releases come from **metadata-only** watches of the Secrets and ConfigMaps labelled
//! `owner=helm`: names, revisions, status and times are labels, so the list needs no Secret
//! data. What the labels don't say (chart, versions, description) comes from the latest
//! revision's object, fetched and decoded on Tokio; only that summary is kept. A release's
//! values and manifest are loaded by its tab ([`load`]) and live there.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use jiff::Timestamp;
use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use kube::Api;
use kubyl_core::{ActiveContext, ClusterId, Gvr, ResourceRef, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey, StoreStatus};
use serde_json::Value;

use super::decode::{self, Driver, Release, Summary};

const KEEP: Duration = Duration::from_secs(120);
const SWEEP: Duration = Duration::from_secs(30);
/// Summaries fetched at once per cluster.
const PARALLEL: usize = 6;

/// The label selector of Helm's storage objects.
pub const SELECTOR: &str = "owner=helm";

/// One revision of a release, from the storage object's labels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub revision: u32,
    pub status: String,
    pub modified: Option<Timestamp>,
    /// The storage object (`sh.helm.release.v1.<name>.v<revision>`).
    pub object: String,
}

/// The summary of a release's latest revision.
#[derive(Clone, Debug)]
pub enum SummaryState {
    Loading,
    Ready(Arc<Summary>),
    Failed(String),
}

/// One release (its latest revision) and its history.
#[derive(Clone, Debug)]
pub struct ReleaseRow {
    pub namespace: String,
    pub name: String,
    pub driver: Driver,
    /// Newest first.
    pub revisions: Vec<Revision>,
    pub summary: SummaryState,
}

impl ReleaseRow {
    pub fn latest(&self) -> &Revision {
        &self.revisions[0]
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    pub fn summary(&self) -> Option<&Arc<Summary>> {
        match &self.summary {
            SummaryState::Ready(s) => Some(s),
            _ => None,
        }
    }

    /// The storage object of the latest revision.
    pub fn object_ref(&self, cluster: &ClusterId) -> ResourceRef {
        object_ref(cluster, self.driver, &self.namespace, &self.latest().object)
    }
}

/// The ref of a release's storage object (what a release tab is opened for).
pub fn object_ref(
    cluster: &ClusterId,
    driver: Driver,
    namespace: &str,
    object: &str,
) -> ResourceRef {
    let resource = match driver {
        Driver::Secret => "secrets",
        Driver::ConfigMap => "configmaps",
    };
    ResourceRef::object(
        cluster.clone(),
        Gvr::new("", "v1", resource),
        Some(namespace.to_string()),
        object.to_string(),
    )
}

/// What the Helm tab shows for a cluster.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub releases: Vec<ReleaseRow>,
    pub loading: bool,
    /// Why releases are missing or limited (403 on Secrets…).
    pub problem: Option<String>,
    /// The namespace the releases are limited to, when listing cluster-wide isn't allowed.
    pub scope: Option<String>,
}

/// Keeps a cluster's Helm watches while held.
#[derive(Clone)]
pub struct HelmLease(#[allow(dead_code)] Rc<()>);

type ObjectId = (Driver, String, String);

struct ClusterHelm {
    lease: Rc<()>,
    wanted_until: Instant,
    scope: Option<String>,
    secrets: StoreHandle,
    config_maps: StoreHandle,
    /// Summaries by storage object and its resourceVersion.
    summaries: HashMap<ObjectId, (String, SummaryState)>,
    queue: VecDeque<(ObjectId, String)>,
    /// Objects being fetched, and the fetches (finished ones are pruned, never dropped from
    /// inside: a task must not drop itself).
    in_flight: HashSet<ObjectId>,
    tasks: Vec<Task<()>>,
    snapshot: RefCell<Option<(u64, Arc<Snapshot>)>>,
    revision: u64,
    _observers: Vec<gpui::Subscription>,
}

pub struct Helm {
    clusters: HashMap<ClusterId, ClusterHelm>,
    _sweep: Option<Task<()>>,
    _subscriptions: Vec<gpui::Subscription>,
}

struct GlobalHelm(Entity<Helm>);

impl Global for GlobalHelm {}

fn label<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object
        .pointer("/metadata/labels")
        .and_then(|l| l.get(key))
        .and_then(Value::as_str)
}

/// Groups storage objects (metadata) into releases, newest revision first.
pub fn group(
    objects: &[(Driver, Arc<Value>)],
) -> Vec<(String, String, Driver, Vec<Revision>, String)> {
    let mut releases: BTreeMap<(String, String, Driver), Vec<(Revision, String)>> = BTreeMap::new();
    for (driver, object) in objects {
        let Some(name) = label(object, "name") else {
            continue;
        };
        let Some(revision) = label(object, "version").and_then(|v| v.parse::<u32>().ok()) else {
            continue;
        };
        let namespace = object
            .pointer("/metadata/namespace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let object_name = object
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let resource_version = object
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let modified = label(object, "modifiedAt")
            .and_then(|t| t.parse::<i64>().ok())
            .and_then(|t| Timestamp::from_second(t).ok())
            .or_else(|| {
                object
                    .pointer("/metadata/creationTimestamp")
                    .and_then(Value::as_str)
                    .and_then(|t| t.parse().ok())
            });
        releases
            .entry((namespace, name.to_string(), *driver))
            .or_default()
            .push((
                Revision {
                    revision,
                    status: label(object, "status").unwrap_or_default().to_string(),
                    modified,
                    object: object_name,
                },
                resource_version,
            ));
    }
    releases
        .into_iter()
        .map(|((namespace, name, driver), mut revisions)| {
            revisions.sort_by_key(|r| std::cmp::Reverse(r.0.revision));
            let resource_version = revisions[0].1.clone();
            (
                namespace,
                name,
                driver,
                revisions.into_iter().map(|(r, _)| r).collect(),
                resource_version,
            )
        })
        .collect()
}

impl Helm {
    pub fn install(sweep: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this: &mut Self, _, event, cx| {
                    match event {
                        ConnectionEvent::Rekeyed { from, .. } => {
                            // Start over under the new id (the stores carry the old one).
                            this.clusters.remove(from);
                            cx.notify();
                        }
                        ConnectionEvent::StateChanged(_) => cx.notify(),
                        _ => {}
                    }
                }));
            }
            let sweep = sweep.then(|| {
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(SWEEP).await;
                        if this.update(cx, |this, _| this.sweep()).is_err() {
                            break;
                        }
                    }
                })
            });
            Self {
                clusters: HashMap::new(),
                _sweep: sweep,
                _subscriptions: subscriptions,
            }
        });
        cx.set_global(GlobalHelm(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalHelm>().map(|g| g.0.clone())
    }

    /// Keeps `cluster`'s Helm watches while the returned lease lives.
    pub fn watch(cluster: &ClusterId, cx: &mut App) -> Option<HelmLease> {
        let helm = Self::global(cx)?;
        Some(helm.update(cx, |helm, cx| {
            helm.ensure(cluster, None, cx);
            HelmLease(helm.clusters[cluster].lease.clone())
        }))
    }

    fn sweep(&mut self) {
        let now = Instant::now();
        self.clusters
            .retain(|_, state| Rc::strong_count(&state.lease) > 1 || state.wanted_until > now);
    }

    fn ensure(&mut self, cluster: &ClusterId, scope: Option<String>, cx: &mut Context<Self>) {
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.wanted_until = Instant::now() + KEEP;
            return;
        }
        let key = |resource: &str| {
            StoreKey::new(cluster.clone(), Gvr::new("", "v1", resource), scope.clone())
                .labels(SELECTOR)
                .metadata()
        };
        let secrets = ResourceStores::acquire(cx, key("secrets"));
        let config_maps = ResourceStores::acquire(cx, key("configmaps"));
        let observers = [&secrets, &config_maps]
            .into_iter()
            .map(|handle| {
                let cluster = cluster.clone();
                cx.observe(handle.entity(), move |this: &mut Self, _, cx| {
                    this.changed(&cluster, cx)
                })
            })
            .collect();
        self.clusters.insert(
            cluster.clone(),
            ClusterHelm {
                lease: Rc::new(()),
                wanted_until: Instant::now() + KEEP,
                scope,
                secrets,
                config_maps,
                summaries: HashMap::new(),
                queue: VecDeque::new(),
                in_flight: HashSet::new(),
                tasks: Vec::new(),
                snapshot: RefCell::new(None),
                revision: 0,
                _observers: observers,
            },
        );
        self.changed(cluster, cx);
    }

    /// Lists only `namespace` (when listing Secrets cluster-wide isn't allowed), or every
    /// namespace again (`None`).
    pub fn set_scope(
        &mut self,
        cluster: &ClusterId,
        scope: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(old) = self.clusters.remove(cluster) else {
            return;
        };
        let lease = old.lease.clone();
        self.ensure(cluster, scope, cx);
        if let Some(state) = self.clusters.get_mut(cluster) {
            state.lease = lease;
        }
        cx.notify();
    }

    fn changed(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.revision += 1;
        // Listing cluster-wide is forbidden: fall back to the active namespace.
        if state.scope.is_none() && *state.secrets.read(cx).status() == StoreStatus::Forbidden {
            let namespace = fallback_namespace(cluster, cx);
            if let Some(namespace) = namespace {
                let lease = state.lease.clone();
                self.clusters.remove(cluster);
                self.ensure(cluster, Some(namespace), cx);
                if let Some(state) = self.clusters.get_mut(cluster) {
                    state.lease = lease;
                }
                cx.notify();
                return;
            }
        }
        let objects: Vec<(Driver, Arc<Value>)> = state
            .secrets
            .read(cx)
            .objects()
            .values()
            .map(|o| (Driver::Secret, o.clone()))
            .chain(
                state
                    .config_maps
                    .read(cx)
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
        self.pump(cluster, cx);
        cx.notify();
    }

    /// Starts queued summary fetches, a few at a time.
    fn pump(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster))
        else {
            return;
        };
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        state.tasks.retain(|task| !task.is_ready());
        while state.in_flight.len() < PARALLEL {
            let Some((id, resource_version)) = state.queue.pop_front() else {
                break;
            };
            let (driver, namespace, object) = id.clone();
            let fetch = spawn_kube(cx, fetch_summary(client.clone(), driver, namespace, object));
            let task_cluster = cluster.clone();
            let task_id = id.clone();
            let task = cx.spawn(async move |this, cx| {
                let result = fetch.await;
                this.update(cx, |this, cx| {
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
                    this.pump(&task_cluster, cx);
                    cx.notify();
                })
                .ok();
            });
            state.in_flight.insert(id);
            state.tasks.push(task);
        }
    }

    /// The releases of `cluster`, if its watches run ([`Self::watch`]).
    pub fn snapshot(&self, cluster: &ClusterId, cx: &App) -> Option<Arc<Snapshot>> {
        let state = self.clusters.get(cluster)?;
        let generation = state.revision * 1_000_003
            + state.secrets.read(cx).generation() * 31
            + state.config_maps.read(cx).generation();
        if let Some((g, snapshot)) = state.snapshot.borrow().as_ref()
            && *g == generation
        {
            return Some(snapshot.clone());
        }
        let secrets = state.secrets.read(cx);
        let config_maps = state.config_maps.read(cx);
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
        let problem = match (secrets.status(), &state.scope) {
            (StoreStatus::Forbidden, Some(ns)) => Some(format!(
                "Helm stores releases in Secrets, and you can't list secrets in {ns} either. Ask for a role with `list` and `watch` on `secrets`."
            )),
            (StoreStatus::Forbidden, None) => Some(
                "Helm stores releases in Secrets, and you can't list secrets cluster-wide. Ask for a role with `list` and `watch` on `secrets`, or pick a namespace you can read."
                    .to_string(),
            ),
            (_, Some(ns)) => Some(format!(
                "You can't list secrets cluster-wide: showing the releases in {ns} only."
            )),
            (StoreStatus::Error(err), _) => Some(err.clone()),
            _ => None,
        };
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

/// The namespace to fall back to: the active one (for this cluster), else the context's.
fn fallback_namespace(cluster: &ClusterId, cx: &App) -> Option<String> {
    let active = ActiveContext::global(cx);
    if active.cluster.as_ref().map(|c| &c.id) == Some(cluster)
        && let Some(ns) = &active.namespace
    {
        return Some(ns.to_string());
    }
    ConnectionManager::try_global(cx)?
        .read(cx)
        .cluster(cluster)?
        .default_namespace()
        .map(str::to_string)
}

/// The raw `data.release` of a storage object.
async fn fetch_encoded(
    client: kube::Client,
    driver: Driver,
    namespace: &str,
    object: &str,
) -> Result<Vec<u8>, String> {
    match driver {
        Driver::Secret => {
            let api: Api<Secret> = Api::namespaced(client, namespace);
            let secret = api
                .get(object)
                .await
                .map_err(|e| crate::errors::describe(&e, "get", "secrets", Some(namespace)))?;
            secret
                .data
                .and_then(|mut d| d.remove("release"))
                .map(|b| b.0)
                .ok_or_else(|| decode::DecodeError::Missing("Secret").to_string())
        }
        Driver::ConfigMap => {
            let api: Api<ConfigMap> = Api::namespaced(client, namespace);
            let config_map = api
                .get(object)
                .await
                .map_err(|e| crate::errors::describe(&e, "get", "configmaps", Some(namespace)))?;
            config_map
                .data
                .and_then(|mut d| d.remove("release"))
                .map(String::into_bytes)
                .ok_or_else(|| decode::DecodeError::Missing("ConfigMap").to_string())
        }
    }
}

async fn fetch_summary(
    client: kube::Client,
    driver: Driver,
    namespace: String,
    object: String,
) -> Result<Summary, String> {
    let encoded = fetch_encoded(client, driver, &namespace, &object).await?;
    decode::decode_summary(&encoded).map_err(|e| e.to_string())
}

/// Loads a whole release revision (values, manifest, notes) for a release tab, on Tokio.
pub async fn load(
    client: kube::Client,
    driver: Driver,
    namespace: String,
    object: String,
) -> Result<Release, String> {
    let encoded = fetch_encoded(client, driver, &namespace, &object).await?;
    decode::decode(&encoded).map_err(|e| e.to_string())
}

/// Loads the summaries of several revisions (the History tab), on Tokio.
pub async fn load_summaries(
    client: kube::Client,
    driver: Driver,
    namespace: String,
    objects: Vec<String>,
) -> Vec<(String, Result<Summary, String>)> {
    let mut out = Vec::new();
    for object in objects {
        let summary =
            fetch_summary(client.clone(), driver, namespace.clone(), object.clone()).await;
        out.push((object, summary));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(ns: &str, name: &str, revision: u32, status: &str, rv: &str) -> Arc<Value> {
        Arc::new(
            json!({"metadata": {"namespace": ns, "name": format!("sh.helm.release.v1.{name}.v{revision}"),
            "resourceVersion": rv,
            "labels": {"owner": "helm", "name": name, "version": revision.to_string(), "status": status, "modifiedAt": "1790338899"}}}),
        )
    }

    #[test]
    fn storage_objects_group_into_releases() {
        let objects = vec![
            (
                Driver::Secret,
                meta("monitoring", "kps", 5, "superseded", "10"),
            ),
            (
                Driver::Secret,
                meta("monitoring", "kps", 6, "deployed", "11"),
            ),
            (
                Driver::Secret,
                meta("kube-system", "metrics-server", 3, "deployed", "12"),
            ),
            (Driver::ConfigMap, meta("shop", "db", 1, "failed", "13")),
            // Not a release (no labels).
            (Driver::Secret, Arc::new(json!({"metadata": {"name": "x"}}))),
        ];
        let releases = group(&objects);
        assert_eq!(releases.len(), 3);
        let kps = releases.iter().find(|r| r.1 == "kps").unwrap();
        assert_eq!(kps.3[0].revision, 6);
        assert_eq!(kps.3[0].status, "deployed");
        assert_eq!(kps.3[0].object, "sh.helm.release.v1.kps.v6");
        assert_eq!(kps.3.len(), 2);
        assert_eq!(kps.4, "11");
        assert!(kps.3[0].modified.is_some());
        let db = releases.iter().find(|r| r.1 == "db").unwrap();
        assert_eq!(db.2, Driver::ConfigMap);
    }
}
