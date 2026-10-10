//! Cost monitoring (phase 25, board 23): what OpenCost says the namespaces of a cluster cost,
//! with idle capacity, CPU and memory efficiency, a window of 24 hours, 7 or 30 days, a
//! cost-over-time chart and the per-namespace table (CSV export).
//!
//! OpenCost is found by its Service and reached through the API server's service proxy; the
//! address can be set per cluster (`cost.clusters.<cluster>.service`). Costs refresh once a
//! minute while a tab shows them ([`service::CostService`]); the window and the idle choice are
//! remembered per cluster. OpenCost needs Prometheus, which the empty state says.
//!
//! The data (queries, parsing, totals, series, detection, CSV) is in `kubyl_cost_core`.

pub mod service;
pub mod view;

pub use kubyl_cost_core::{detect, fetch, model, settings, summary, table, window};

use std::sync::Arc;

use gpui::{App, AppContext as _, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_explorer::catalog::{self, ViewRow};
use kubyl_settings::Settings;
use kubyl_ui::IconName;

actions!(
    cost,
    [
        /// Opens the Cost view of the active cluster.
        ShowCost,
    ]
);

/// The view id of the tab.
pub const VIEW_KIND: &str = "cost";

/// Registers settings, the service, the view, the sidebar row and the actions.
pub fn init(cx: &mut App) {
    init_with(true, cx);
}

/// Like [`init`]; `live` off keeps GPUI tests free of the refresh loop.
pub fn init_with(live: bool, cx: &mut App) {
    Settings::register::<settings::CostSettings>(cx);
    service::CostService::install(live, cx);
    view::init(cx);
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            Some(Box::new(
                cx.new(|cx| view::CostView::new(target.cluster, window, cx)),
            ))
        },
    );
    catalog::register_view_row(
        cx,
        ViewRow {
            id: "cost",
            after: "prometheus",
            label: "Cost",
            icon: IconName::Cloud,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: None,
            badge: None,
        },
    );
    kubyl_palette::register_view(
        cx,
        kubyl_palette::PaletteView {
            name: "cost",
            aliases: vec!["opencost"],
            detail: "Cost per namespace (OpenCost)",
            icon: IconName::Cloud,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: Arc::new(|_, _| true),
        },
    );
    ActionRegistry::register(cx, ActionSpec::new("Cost: Open Cost View", ShowCost));
    cx.on_action(|_: &ShowCost, cx| {
        let Some(cluster) = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone())
        else {
            NotificationCenter::push(cx, Notification::info("Select a cluster first."));
            return;
        };
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

/// Opens the Cost tab of a cluster.
pub fn open(cluster: &ClusterId, window: &mut gpui::Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            target(cluster),
        ))),
        cx,
    );
}
