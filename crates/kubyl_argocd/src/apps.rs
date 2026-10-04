//! Applications as the views show them (rows, filters and sorting from `kubyl_argocd_core`), and
//! finding them in the shared watch caches.

use std::sync::Arc;

use gpui::App;
pub use kubyl_argocd_core::apps::*;
use kubyl_core::{ClusterId, Gvr};
use kubyl_kube::ConnectionManager;
use kubyl_resources::{ResourceStores, object_key};
use serde_json::Value;

use crate::links::KnownCluster;
use crate::model::Application;
use crate::ops::AppTarget;

/// A loaded app (from any running watch of its cluster).
pub fn find_object(
    cluster: &ClusterId,
    gvr: &Gvr,
    app: &AppTarget,
    cx: &App,
) -> Option<Arc<Value>> {
    let key = object_key(Some(&app.namespace), &app.name);
    [
        all_key(cluster, gvr),
        namespace_key(cluster, gvr, &app.namespace),
        namespace_key(cluster, gvr, &app.namespace).fields(format!("metadata.name={}", app.name)),
    ]
    .iter()
    .filter_map(|k| ResourceStores::peek(cx, k))
    .find_map(|store| store.read(cx).get(&key).cloned())
}

pub fn find_app(cluster: &ClusterId, gvr: &Gvr, app: &AppTarget, cx: &App) -> Option<Application> {
    find_object(cluster, gvr, app, cx).and_then(|o| Application::parse(&o))
}

/// Kubyl contexts with their API servers, for destination links.
pub fn known_clusters(cx: &App) -> Vec<KnownCluster> {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return Vec::new();
    };
    manager
        .read(cx)
        .contexts()
        .filter_map(|c| {
            Some(KnownCluster {
                id: c.id.clone(),
                name: c.context.clone(),
                server: c.server.clone()?,
            })
        })
        .collect()
}
