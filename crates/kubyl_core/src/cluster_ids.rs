//! Cluster ids that may be out of date: persisted before contexts were grouped, or an entry's id
//! from before it was re-keyed (phase 15). `kubyl_kube` installs the resolver; views are built
//! with current ids ([`crate::ViewRegistry::build`]) and the workspace rebuilds tabs whose id
//! changed when [`ClusterIds::changed`] is signalled.

use std::sync::Arc;

use gpui::{App, Global};

use crate::registry::ViewRequest;
use crate::types::ClusterId;

type Resolve = Arc<dyn Fn(&ClusterId, &App) -> ClusterId>;

/// Maps cluster ids to the current ones. Observe it (`cx.observe_global::<ClusterIds>`) to
/// learn when ids may resolve differently.
#[derive(Clone, Default)]
pub struct ClusterIds {
    resolve: Option<Resolve>,
    revision: u64,
}

impl Global for ClusterIds {}

impl ClusterIds {
    /// Installs the resolver (`kubyl_kube`).
    pub fn install(cx: &mut App, resolve: impl Fn(&ClusterId, &App) -> ClusterId + 'static) {
        let revision = cx.try_global::<Self>().map_or(0, |ids| ids.revision);
        cx.set_global(Self {
            resolve: Some(Arc::new(resolve)),
            revision,
        });
    }

    /// Ids may resolve differently now (kubeconfigs reloaded, grouping changed). Notifies
    /// observers.
    pub fn changed(cx: &mut App) {
        let mut ids = cx.try_global::<Self>().cloned().unwrap_or_default();
        ids.revision += 1;
        cx.set_global(ids);
    }

    pub fn revision(cx: &App) -> u64 {
        cx.try_global::<Self>().map_or(0, |ids| ids.revision)
    }

    /// The current id of `id` (itself when nothing is installed or it's current).
    pub fn resolve(cx: &App, id: &ClusterId) -> ClusterId {
        match cx.try_global::<Self>().and_then(|ids| ids.resolve.clone()) {
            Some(resolve) => resolve(id, cx),
            None => id.clone(),
        }
    }

    /// `request` with its target's cluster id resolved.
    pub fn normalize(cx: &App, request: &ViewRequest) -> ViewRequest {
        let mut request = request.clone();
        if let Some(target) = request.target.as_mut() {
            target.cluster = Self::resolve(cx, &target.cluster);
        }
        request
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Gvr, ResourceRef, ViewKind};

    #[gpui::test]
    fn resolves_and_signals(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let old = ClusterId::new("ns/c/u@/k");
            assert_eq!(ClusterIds::resolve(cx, &old), old);
            ClusterIds::install(cx, |id, _| {
                if id.as_str() == "ns/c/u@/k" {
                    ClusterId::new("group:c,u@/k/")
                } else {
                    id.clone()
                }
            });
            let request = ViewRequest::for_resource(
                ViewKind::Table,
                ResourceRef::list(old, Gvr::new("", "v1", "pods"), None),
            );
            let normalized = ClusterIds::normalize(cx, &request);
            assert_eq!(normalized.target.unwrap().cluster.as_str(), "group:c,u@/k/");
            let before = ClusterIds::revision(cx);
            ClusterIds::changed(cx);
            assert_eq!(ClusterIds::revision(cx), before + 1);
        });
    }
}
