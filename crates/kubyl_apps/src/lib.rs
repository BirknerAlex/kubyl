//! Applications (phase 25, board 23): the objects of a cluster grouped by their
//! `app.kubernetes.io/instance` label, one row per application with who manages it (Helm,
//! Argo CD, Flux), its version, age and health. A row's details list its objects with links,
//! and its workloads' logs are one click away.
//!
//! - [`view`]: the tab (table, filter, details, CSV export). [`service`]: the sidebar count.
//! - the sidebar row under every cluster, `:apps` in the palette and the
//!   `Applications: Open` action.
//!
//! The grouping, the health rules and the CSV records are in `kubyl_apps_core`.

pub mod service;
pub mod view;

pub use kubyl_apps_core::{Health, Kind, Manager, build, health, kinds, model, table};

use std::sync::Arc;

use gpui::{App, AppContext as _, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_ui::IconName;

actions!(
    applications,
    [
        /// Opens the Applications view of the active cluster.
        ShowApplications,
    ]
);

/// The view id of the tab.
pub const VIEW_KIND: &str = "applications";

/// Registers the view, the sidebar row, the palette entries and the actions.
pub fn init(cx: &mut App) {
    let service = service::AppsService::install(cx);
    // The number of applications next to the row in Workloads.
    kubyl_explorer::catalog::register_view_count(
        cx,
        "applications",
        Arc::new(move |cluster, cx| service.read(cx).count(cluster)),
    );
    view::init(cx);
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| view::AppsView::new(target.cluster, window, cx)),
            ))
        },
    );
    // The sidebar entry is a view of the Workloads group (`kubyl_explorer::catalog`).
    kubyl_palette::register_view(
        cx,
        kubyl_palette::PaletteView {
            name: "apps",
            aliases: vec!["applications"],
            detail: "Applications by app.kubernetes.io labels",
            icon: IconName::Blocks,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: Arc::new(|_, _| true),
        },
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Applications: Open for This Cluster", ShowApplications),
    );
    cx.on_action(|_: &ShowApplications, cx| {
        let Some(cluster) = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone())
        else {
            NotificationCenter::push(cx, Notification::info("Select a cluster first."));
            return;
        };
        // Global actions run inside the dispatching window's update.
        cx.defer(move |cx| {
            if let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) {
                window
                    .update(cx, |_, window, cx| open(&cluster, window, cx))
                    .ok();
            }
        });
    });
}

/// The ref the tab is opened for.
pub fn target(cluster: &ClusterId) -> ResourceRef {
    ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None)
}

/// Opens (or focuses) the Applications tab of a cluster.
pub fn open(cluster: &ClusterId, window: &mut gpui::Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            target(cluster),
        ))),
        cx,
    );
}
