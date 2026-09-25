//! Cluster overview dashboard and events stream (board 4).
//!
//! - [`overview::OverviewView`]: `ViewKind::Overview`, the cluster dashboard (KPI tiles, charts
//!   by namespace, nodes with cordon/drain), or a namespace's (workload health, top pods, recent
//!   warnings) when the request's target has a namespace.
//! - [`events`]: the live events stream (`events.k8s.io/v1`, falling back to `v1`, plus
//!   `OOMKilled` warnings derived from pod status): the right-dock Events panel and
//!   `ViewKind::Events`, a searchable table.
//! - [`notify`]: opt-in warnings for favorited namespaces.
//! - [`settings`]: the `"overview"` settings section.

pub mod events;
pub mod notify;
pub mod overview;
pub mod settings;

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{App, Window, actions};
use kubyl_core::actions::{ActivateDockPanel, OpenView};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ChromeRegistry, Gvr, ResourceRef, ViewKind,
    ViewRegistry, ViewRequest, new_tab,
};
use kubyl_settings::Settings;

use events::{EventsDock, EventsView, PANEL_ID};
use overview::OverviewView;

actions!(
    overview,
    [
        /// Opens the Overview of the active cluster.
        OpenOverview,
        /// Opens the Events view of the active cluster.
        OpenEvents,
    ]
);

/// When `init` ran; views built right after it are tabs restored from state.json.
static STARTED: OnceLock<Instant> = OnceLock::new();

fn restoring() -> bool {
    STARTED
        .get()
        .is_none_or(|started| started.elapsed() < Duration::from_secs(2))
}

/// Registers the Overview and Events views, the Events dock panel, settings and actions.
pub fn init(cx: &mut App) {
    STARTED.get_or_init(Instant::now);
    Settings::register::<settings::OverviewSettings>(cx);
    events::view::init(cx);
    ChromeRegistry::add_dock_panel(cx, EventsDock);

    ViewRegistry::register(cx, ViewKind::Overview, |request, window, cx| {
        let target = target_or_active(request, cx)?;
        let tab = new_tab(cx, |cx| {
            OverviewView::new(target.cluster.clone(), target.namespace.clone(), cx)
        });
        // Board 4 pairs the overview with the live events in the right dock. Not for tabs
        // restored at startup: the dock stays as the user left it.
        if !restoring() {
            // Dispatched now, while focus is still on an element of the rendered frame (GPUI
            // runs it deferred from there); the new tab isn't rendered yet, and an action routed
            // from it would start at the window root and miss the workspace.
            window.dispatch_action(Box::new(ActivateDockPanel(PANEL_ID.into())), cx);
        }
        Some(tab)
    });
    ViewRegistry::register(cx, ViewKind::Events, |request, window, cx| {
        let target = target_or_active(request, cx)?;
        let target = ResourceRef::list(
            target.cluster,
            Gvr::new("", "v1", "events"),
            target.namespace,
        );
        Some(new_tab(cx, |cx| EventsView::new(target, window, cx)))
    });

    for spec in [
        ActionSpec::new("Cluster: Open Overview", OpenOverview),
        ActionSpec::new("Cluster: Open Events", OpenEvents),
    ] {
        ActionRegistry::register(cx, spec);
    }
    cx.on_action(|_: &OpenOverview, cx| open_for_active(ViewKind::Overview, cx));
    cx.on_action(|_: &OpenEvents, cx| open_for_active(ViewKind::Events, cx));

    notify::WarningNotifier::install(cx);
}

/// The request's target, else the active cluster (palette commands open without a target).
fn target_or_active(request: &ViewRequest, cx: &App) -> Option<ResourceRef> {
    request.target.clone().or_else(|| {
        let cluster = ActiveContext::global(cx).cluster.as_ref()?.id.clone();
        Some(ResourceRef::list(cluster, Gvr::new("", "", ""), None))
    })
}

fn open_for_active(kind: ViewKind, cx: &mut App) {
    let Some(cluster) = ActiveContext::global(cx)
        .cluster
        .as_ref()
        .map(|c| c.id.clone())
    else {
        return;
    };
    // Global handlers run inside the dispatching window's update.
    cx.defer(move |cx| {
        let Some(window) = cx.active_window() else {
            return;
        };
        window
            .update(cx, |_, window: &mut Window, cx| {
                window.dispatch_action(
                    Box::new(OpenView(ViewRequest::for_resource(
                        kind,
                        ResourceRef::list(cluster, Gvr::new("", "", ""), None),
                    ))),
                    cx,
                )
            })
            .ok();
    });
}
