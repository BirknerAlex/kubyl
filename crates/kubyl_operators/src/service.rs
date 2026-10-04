//! [`Olm`]: the app-wide OLM state ([`OlmCore`] from `kubyl_operators_core`) in an entity. Per
//! cluster the core keeps the watches of the OLM objects while a view (or phase 13's check)
//! wants them, and serves a [`Snapshot`] joined from them; it also caches OperatorHub's
//! packages and icons, upgrade reviews, and runs the writes that outlive a dialog (install,
//! approve, uninstall).
//!
//! The core doesn't own watches: this acquires them from `ResourceStores` (and acquires others
//! when the cluster starts or stops serving OLM), and hands the core the cluster's connection.

use std::collections::HashMap;
use std::convert::Infallible;
use std::ops::Deref;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Image, Task};
use kubyl_core::ClusterId;
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::ResourceStores;
use kubyl_resources::store::{AppStores, AppStoresRef};

pub use kubyl_operators_core::olm::service::{
    Availability, HubState, OlmConn, OlmCore, OlmLease, OlmStores, ReviewState, Snapshot,
    WatchProblems, watch_problems,
};

use crate::olm::hub::{IconFormat, Package};
use crate::olm::model::{Csv, InstallPlan};
use crate::olm::v1;

/// A package icon: loading, loaded, or none.
#[derive(Clone)]
pub enum Icon {
    Loading,
    Loaded(Arc<Image>),
    None,
}

/// The OLM state of every cluster a view asked about. Reads go to the [`OlmCore`]; observe the
/// entity to re-render.
pub struct Olm {
    core: OlmCore,
    /// Observers of each cluster's watches.
    observers: HashMap<ClusterId, Vec<gpui::Subscription>>,
    /// Decoded icons, kept so an icon is the same image every frame.
    images: HashMap<(ClusterId, String), Arc<Image>>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl Deref for Olm {
    type Target = OlmCore;

    fn deref(&self) -> &OlmCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for Olm {}

impl Hosts<OlmCore> for Olm {
    fn service(&mut self) -> &mut OlmCore {
        &mut self.core
    }

    fn apply(&mut self, effect: Infallible, _cx: &mut Context<Self>) {
        match effect {}
    }
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
            let mut this = Self {
                core: OlmCore::new(),
                observers: HashMap::new(),
                images: HashMap::new(),
                _subscriptions: subscriptions,
            };
            if sweep {
                hosted(&mut this, cx, |core, host| core.start(host));
            }
            this
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
                if self.core.needs_rewatch(id, served(id, cx)) {
                    self.rewatch(id, cx);
                }
                cx.notify();
            }
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                if connected {
                    cx.notify();
                } else {
                    self.images.retain(|(c, _), _| c != id);
                    hosted(self, cx, |core, host| core.disconnected(id, host));
                }
            }
            ConnectionEvent::Rekeyed { from, to } => {
                // The stores carry the old id: start over under the new one.
                self.observers.remove(from);
                self.images.retain(|(c, _), _| c != from);
                if hosted(self, cx, |core, host| core.rekeyed(from, to, host)) {
                    self.rewatch(to, cx);
                }
            }
            _ => {}
        }
    }

    /// Keeps `cluster`'s watches while the returned lease lives.
    pub fn watch(cluster: &ClusterId, cx: &mut App) -> Option<OlmLease> {
        let olm = Self::global(cx)?;
        Some(olm.update(cx, |olm, cx| olm.ensure(cluster, cx)))
    }

    /// Starts the watches of `cluster` (if the cluster serves OLM) and marks it wanted.
    pub(crate) fn ensure(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) -> OlmLease {
        let served = served(cluster, cx);
        let stores = (!self.core.tracks(cluster)).then(|| {
            let (v0, v1) = served.unwrap_or_default();
            let stores = OlmStores::acquire(cluster, v0, v1, &mut AppStores(cx));
            self.observe(cluster, &stores, cx);
            stores
        });
        self.core
            .watch(cluster, stores, served)
            .expect("the watches were acquired")
    }

    /// Replaces the watches of `cluster` with ones that fit what it serves.
    fn rewatch(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let served = served(cluster, cx);
        let (v0, v1) = served.unwrap_or_default();
        let stores = OlmStores::acquire(cluster, v0, v1, &mut AppStores(cx));
        self.observe(cluster, &stores, cx);
        hosted(self, cx, |core, host| {
            core.rewatch(cluster, stores, served, host)
        });
    }

    /// Re-renders when one of the cluster's watches changes.
    fn observe(&mut self, cluster: &ClusterId, stores: &OlmStores, cx: &mut Context<Self>) {
        let entities: Vec<_> = stores
            .keys()
            .iter()
            .filter_map(|key| ResourceStores::peek(cx, key))
            .collect();
        let observers = entities
            .into_iter()
            .map(|store| cx.observe(&store, |_, _, cx| cx.notify()))
            .collect();
        self.observers.insert(cluster.clone(), observers);
    }

    /// What `cluster` has of OLM.
    pub fn availability(&self, cluster: &ClusterId, cx: &App) -> Availability {
        let conn = ConnectionManager::try_global(cx).map(|_| conn(cluster, cx));
        self.core
            .availability(cluster, conn.as_ref(), &AppStoresRef(cx))
    }

    /// The joined state of `cluster`, if its watches run ([`Self::watch`]). Built again only
    /// when a watch changed.
    pub fn snapshot(&self, cluster: &ClusterId, cx: &App) -> Option<Arc<Snapshot>> {
        self.core.snapshot(cluster, &AppStoresRef(cx))
    }

    /// Fills a cluster's OperatorHub packages by hand (GPUI tests: nothing is fetched).
    #[cfg(test)]
    pub(crate) fn insert_hub_for_test(&mut self, cluster: &ClusterId, packages: Vec<Package>) {
        self.core.seed_hub(cluster, packages);
    }

    /// Fills a cluster's watches by hand (GPUI tests: no cluster, no network).
    #[cfg(test)]
    pub(crate) fn insert_for_test(
        &mut self,
        cluster: &ClusterId,
        subscriptions: Vec<serde_json::Value>,
        csvs: Vec<serde_json::Value>,
        plans: Vec<serde_json::Value>,
        cx: &mut Context<Self>,
    ) {
        use kubyl_operators_core::olm::service::Kind;
        use kubyl_resources::{ResourceStore, StoreKey};
        let mut fill = |kind: Kind, objects: Vec<serde_json::Value>| {
            let key: StoreKey = kind.key(cluster);
            let store = cx.new(|_| ResourceStore::from_objects(key.clone(), objects));
            ResourceStores::insert(cx, key, store);
        };
        fill(Kind::Subscriptions, subscriptions);
        fill(Kind::Csvs, csvs);
        fill(Kind::Plans, plans);
        fill(Kind::Catalogs, Vec::new());
        fill(Kind::Groups, Vec::new());
        let stores = OlmStores::acquire(cluster, true, false, &mut AppStores(cx));
        self.observe(cluster, &stores, cx);
        hosted(self, cx, |core, host| {
            core.rewatch_or_watch(cluster, stores, Some((true, false)), host)
        });
    }

    // ----- OperatorHub -----

    /// OperatorHub's packages of `cluster`, fetched when missing or older than
    /// [`kubyl_operators_core::olm::snapshot::HUB_REFRESH`] (or when `force`).
    pub fn hub(&mut self, cluster: &ClusterId, force: bool, cx: &mut Context<Self>) -> &HubState {
        let conn = conn(cluster, cx);
        hosted(self, cx, |core, host| {
            core.hub(cluster, force, &conn, host);
        });
        self.core
            .hub_state(cluster)
            .expect("the hub entry was made")
    }

    /// A package's icon from the packageserver; loads it on first ask.
    pub fn icon(&mut self, cluster: &ClusterId, package: &Package, cx: &mut Context<Self>) -> Icon {
        let conn = conn(cluster, cx);
        let state = hosted(self, cx, |core, host| {
            core.icon(cluster, package, &conn, host)
        });
        match state {
            kubyl_operators_core::olm::service::IconState::Loading => Icon::Loading,
            kubyl_operators_core::olm::service::IconState::None => Icon::None,
            kubyl_operators_core::olm::service::IconState::Loaded { format, bytes } => {
                let key = (cluster.clone(), package.key());
                let image = self.images.entry(key).or_insert_with(|| {
                    let format = match format {
                        IconFormat::Svg => gpui::ImageFormat::Svg,
                        IconFormat::Png => gpui::ImageFormat::Png,
                        IconFormat::Jpeg => gpui::ImageFormat::Jpeg,
                        IconFormat::Gif => gpui::ImageFormat::Gif,
                    };
                    Arc::new(Image::from_bytes(format, bytes.to_vec()))
                });
                Icon::Loaded(image.clone())
            }
        }
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
        let conn = conn(cluster, cx);
        hosted(self, cx, |core, host| {
            core.review(cluster, plan, installed, &conn, host)
        })
    }

    // ----- Writes -----

    /// Runs a write on Tokio, marks `key` busy meanwhile and reports the outcome as a toast.
    /// The write belongs to the service: closing the dialog doesn't cancel it.
    pub fn run(
        &mut self,
        key: String,
        work: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
        success: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let answer = hosted(self, cx, |core, host| core.run(key, work, success, host));
        cx.background_executor().spawn(async move {
            answer
                .await
                .unwrap_or_else(|_| Err("Cancelled.".to_string()))
        })
    }
}

/// Whether the cluster serves OLM v0 and v1; `None` while discovery hasn't run.
fn served(cluster: &ClusterId, cx: &App) -> Option<(bool, bool)> {
    ConnectionManager::try_global(cx)
        .and_then(|m| m.read(cx).discovery(cluster))
        .map(|d| {
            (
                d.has_group(crate::olm::model::GROUP),
                d.has_group(v1::GROUP),
            )
        })
}

/// What the core reads from the cluster's connection.
fn conn(cluster: &ClusterId, cx: &App) -> OlmConn {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return OlmConn::default();
    };
    let manager = manager.read(cx);
    OlmConn {
        client: manager.client(cluster),
        served: served(cluster, cx),
        kube_version: manager
            .cluster(cluster)
            .and_then(|c| c.info.as_ref())
            .map(|i| i.version.clone()),
        openshift: manager.caps(cluster).openshift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_resources::StoreStatus;

    /// A watch the installed operators don't come from (OLM v1, catalogs) is a problem to show,
    /// not one that hides the operators; a failing Subscription or CSV watch is.
    #[test]
    fn only_subscription_and_csv_watches_block_the_operators() {
        let ready = |what| (what, StoreStatus::Ready, false);
        let watched = watch_problems(&[
            ready("subscriptions"),
            ready("clusterserviceversions"),
            ("clusterextensions", StoreStatus::Forbidden, true),
            ("catalogsources", StoreStatus::Loading, true),
        ]);
        assert!(watched.loading);
        assert_eq!(watched.problems.len(), 1);
        assert!(watched.problems[0].contains("clusterextensions"));
        assert_eq!(watched.operators, None);

        let watched = watch_problems(&[
            ("subscriptions", StoreStatus::Error("timeout".into()), true),
            ("clusterserviceversions", StoreStatus::Forbidden, true),
        ]);
        assert_eq!(watched.operators.as_deref(), Some("subscriptions: timeout"));
        assert_eq!(watched.problems.len(), 2);

        // An error after the list was read (the watch retries) keeps what was read.
        let watched = watch_problems(&[(
            "clusterserviceversions",
            StoreStatus::Error("stream reset".into()),
            false,
        )]);
        assert_eq!(watched, WatchProblems::default());
    }
}
