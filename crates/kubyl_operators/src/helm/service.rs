//! [`Helm`]: the Helm releases of each cluster a view shows ([`HelmCore`] from
//! `kubyl_operators_core`) in an entity.
//!
//! Releases come from **metadata-only** watches of the Secrets and ConfigMaps labelled
//! `owner=helm`: names, revisions, status and times are labels, so the list needs no Secret
//! data. What the labels don't say (chart, versions, description) comes from the latest
//! revision's object, fetched and decoded on Tokio; only that summary is kept. A release's
//! values and manifest are loaded by its tab ([`load`]) and live there.
//!
//! The core doesn't own watches: this acquires them from `ResourceStores`, tells the core when
//! they changed (with copies of their contents) and lists the namespace it asks for when
//! listing cluster-wide is forbidden.

use std::collections::HashMap;
use std::convert::Infallible;
use std::ops::Deref;
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ActiveContext, ClusterId};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::ResourceStores;
use kubyl_resources::source::StoreCopies;
use kubyl_resources::store::{AppStores, AppStoresRef};

pub use kubyl_operators_core::helm::release::*;
pub use kubyl_operators_core::helm::service::{
    HelmCore, HelmEffect, HelmInputs, HelmLease, HelmStores,
};

/// The Helm releases of every cluster a view asked about. Reads go to the [`HelmCore`]; observe
/// the entity to re-render.
pub struct Helm {
    core: HelmCore,
    /// Observers of each cluster's watches.
    observers: HashMap<ClusterId, Vec<gpui::Subscription>>,
    _subscriptions: Vec<gpui::Subscription>,
}

impl Deref for Helm {
    type Target = HelmCore;

    fn deref(&self) -> &HelmCore {
        &self.core
    }
}

impl EventEmitter<Infallible> for Helm {}

impl Hosts<HelmCore> for Helm {
    fn service(&mut self) -> &mut HelmCore {
        &mut self.core
    }

    fn apply(&mut self, effect: HelmEffect, cx: &mut Context<Self>) {
        match effect {
            HelmEffect::Rescope { cluster, namespace } => {
                self.set_scope(&cluster, Some(namespace), cx)
            }
        }
    }
}

struct GlobalHelm(Entity<Helm>);

impl Global for GlobalHelm {}

impl Helm {
    /// Installs the global. `sweep`: drop unused watches periodically (off in GPUI tests).
    pub fn install(sweep: bool, cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|cx: &mut Context<Self>| {
            let mut subscriptions = Vec::new();
            if let Some(manager) = ConnectionManager::try_global(cx) {
                subscriptions.push(cx.subscribe(&manager, |this: &mut Self, _, event, cx| {
                    match event {
                        ConnectionEvent::Rekeyed { from, .. } => {
                            this.observers.remove(from);
                            hosted(this, cx, |core, host| core.rekeyed(from, host));
                        }
                        ConnectionEvent::StateChanged(_) => {
                            hosted(this, cx, |core, host| core.connection_changed(host));
                        }
                        _ => {}
                    }
                }));
            }
            let mut this = Self {
                core: HelmCore::new(),
                observers: HashMap::new(),
                _subscriptions: subscriptions,
            };
            if sweep {
                hosted(&mut this, cx, |core, host| core.start(host));
            }
            this
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
        helm.update(cx, |helm, cx| {
            let new = !helm.core.tracks(cluster);
            let stores = new.then(|| HelmStores::acquire(cluster, None, &mut AppStores(cx)));
            if new {
                helm.observe(cluster, None, cx);
            }
            let inputs = helm.inputs(cluster, cx);
            hosted(helm, cx, |core, host| {
                core.watch(cluster, stores, &inputs, host)
            })
        })
    }

    /// Lists only `namespace` (when listing Secrets cluster-wide isn't allowed), or every
    /// namespace again (`None`).
    pub fn set_scope(
        &mut self,
        cluster: &ClusterId,
        scope: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.core.tracks(cluster) {
            return;
        }
        let stores = HelmStores::acquire(cluster, scope.clone(), &mut AppStores(cx));
        self.observe(cluster, scope.as_deref(), cx);
        let inputs = self.inputs_for(cluster, scope.as_deref(), cx);
        hosted(self, cx, |core, host| {
            core.rescope(cluster, stores, &inputs, host)
        });
    }

    /// Tells the core when one of the cluster's watches changes.
    fn observe(&mut self, cluster: &ClusterId, scope: Option<&str>, cx: &mut Context<Self>) {
        let stores: Vec<_> = HelmStores::keys(cluster, scope)
            .iter()
            .filter_map(|key| ResourceStores::peek(cx, key))
            .collect();
        let observers = stores
            .into_iter()
            .map(|store| {
                let cluster = cluster.clone();
                cx.observe(&store, move |this: &mut Self, _, cx| {
                    this.changed(&cluster, cx)
                })
            })
            .collect();
        self.observers.insert(cluster.clone(), observers);
    }

    fn changed(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let inputs = self.inputs(cluster, cx);
        hosted(self, cx, |core, host| core.changed(cluster, &inputs, host));
    }

    /// What the core reads from the app for the cluster it follows.
    fn inputs(&self, cluster: &ClusterId, cx: &App) -> HelmInputs {
        self.inputs_for(cluster, self.core.scope(cluster), cx)
    }

    fn inputs_for(&self, cluster: &ClusterId, scope: Option<&str>, cx: &App) -> HelmInputs {
        HelmInputs {
            client: ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(cluster)),
            fallback_namespace: fallback_namespace(cluster, cx),
            stores: StoreCopies::of(&AppStoresRef(cx), &HelmStores::keys(cluster, scope)),
        }
    }

    /// The releases of `cluster`, if its watches run ([`Self::watch`]).
    pub fn snapshot(&self, cluster: &ClusterId, cx: &App) -> Option<Arc<Snapshot>> {
        self.core.snapshot(cluster, &AppStoresRef(cx))
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

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_operators_core::helm::decode::Driver;
    use kubyl_resources::StoreStatus;
    use serde_json::{Value, json};

    fn meta(ns: &str, name: &str, revision: u32, status: &str, rv: &str) -> Arc<Value> {
        Arc::new(
            json!({"metadata": {"namespace": ns, "name": format!("sh.helm.release.v1.{name}.v{revision}"),
            "resourceVersion": rv,
            "labels": {"owner": "helm", "name": name, "version": revision.to_string(), "status": status, "modifiedAt": "1790338899"}}}),
        )
    }

    /// A failing watch shows its error, also when the list is limited to one namespace.
    #[test]
    fn banners_show_watch_errors_before_the_scope() {
        let error = StoreStatus::Error("connection refused".into());
        assert_eq!(
            problem(&error, Some("shop")).as_deref(),
            Some("connection refused")
        );
        assert_eq!(problem(&error, None).as_deref(), Some("connection refused"));
        assert!(
            problem(&StoreStatus::Ready, Some("shop"))
                .unwrap()
                .contains("releases in shop only")
        );
        assert!(
            problem(&StoreStatus::Forbidden, Some("shop"))
                .unwrap()
                .contains("in shop either")
        );
        assert_eq!(problem(&StoreStatus::Ready, None), None);
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
