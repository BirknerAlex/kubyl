//! XRay (k9s `:xray`): a tree from a workload or service down to its pods, their containers and
//! the ConfigMaps, Secrets, volume claims and service account they use, with the health of each
//! node. `:xray dp`, `:xray svc kube-system` in the palette open it; Enter on an object opens
//! its page.
//!
//! The tree is built from the explorer's cached objects (Secrets, ConfigMaps and the like by
//! metadata only, so no Secret data is ever fetched); the logic is in `kubyl_xray_core`.

pub mod view;

pub use kubyl_xray_core::{Health, Node, Root, Row, build, flatten};

use gpui::{App, AppContext as _, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ClusterId, Gvr, ResourceRef, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_resources::ResourceSelection;

actions!(
    xray,
    [
        /// Opens the XRay tree of the selected workload or service.
        OpenXray,
    ]
);

/// The view id of the tab.
pub const VIEW_KIND: &str = "xray";

/// Registers the view.
pub fn init(cx: &mut App) {
    view::init(cx);
    init_action(cx);
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| view::XrayView::new(target, window, cx)),
            ))
        },
    );
}

fn init_action(cx: &mut App) {
    // In the row menu and the palette of resource lists, for the kinds XRay starts from.
    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: XRay", OpenXray)
            .hint("XRay")
            .in_context("ResourceList")
            .available_when(|target, _| root_of(&target.gvr).is_some()),
    );
    cx.on_action(|_: &OpenXray, cx| {
        let Some(primary) = ResourceSelection::global(cx).primary().cloned() else {
            return;
        };
        let target = primary.target;
        let Some(root) = root_of(&target.gvr) else {
            return;
        };
        // Named: the tree opens on that object.
        let target = ResourceRef {
            gvr: gvr_of(root),
            ..target
        };
        // Deferred: the action runs while the dispatching window is being updated.
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(
                            Box::new(OpenView(ViewRequest::for_resource(
                                ViewKind::Custom(VIEW_KIND.into()),
                                target,
                            ))),
                            cx,
                        )
                    })
                    .ok();
            }
        });
    });
}

/// The group and plural a root is addressed by in the tab's request.
pub fn gvr_of(root: Root) -> Gvr {
    let (group, resource) = root.api();
    Gvr::new(group, "", resource)
}

/// The root a request's resource names.
pub fn root_of(gvr: &Gvr) -> Option<Root> {
    Root::ALL
        .into_iter()
        .find(|r| gvr_of(*r).resource == gvr.resource)
}

/// The ref the tab is opened for (`namespace: None`: all namespaces).
pub fn target(cluster: &ClusterId, root: Root, namespace: Option<String>) -> ResourceRef {
    ResourceRef::list(cluster.clone(), gvr_of(root), namespace)
}
