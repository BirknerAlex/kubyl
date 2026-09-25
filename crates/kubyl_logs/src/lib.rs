//! Log streaming, search and level parsing (board 2 · Live logs).
//!
//! - [`view::LogsView`]: `ViewKind::Logs`, opened for a Pod, a workload
//!   (Deployment/StatefulSet/DaemonSet/ReplicaSet/Job) or a Service. Workloads and Services
//!   stream every pod of their selector (editable in the view); pods join and leave live.
//! - [`stream`]: the kube-side log streaming: a pod watch for selector sources, the initial
//!   backlog merged by timestamp, per-container reconnect with backoff.
//! - [`ring`], [`level`], [`search`], [`json`]: the ring buffer, level detection, search and
//!   JSON helpers, all pure and unit-tested without a cluster.
//! - [`sessions`]: the active-sessions registry shared with `kubyl_terminal` and
//!   `kubyl_portforward` (both depend on this crate for it — see the module docs).
//! - [`dock`]: the right-dock "Active Sessions" panel and its status-bar counters.
//! - [`settings`]: the `"logs"` settings section (ring buffer size, default tail/follow/etc).

pub mod dock;
pub mod json;
pub mod level;
pub mod line;
pub mod ring;
pub mod search;
pub mod sessions;
pub mod settings;
pub mod stream;
pub mod view;

use gpui::{App, AppContext as _, Window, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ChromeRegistry, ResourceRef, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_resources::ResourceSelection;
use kubyl_settings::Settings;

use dock::{ActiveSessionsPanel, SessionsStatusItem};
use sessions::SessionRegistry;
use settings::LogsSettings;
use view::LogsView;

actions!(
    logs,
    [
        /// Opens the log view for the selected pod or workload.
        ShowLogs,
    ]
);

/// Kinds the log view can stream: pods, and the workloads and Services that select pods.
pub fn logs_applicable(resource: &str) -> bool {
    matches!(
        resource,
        "pods"
            | "deployments"
            | "statefulsets"
            | "daemonsets"
            | "replicasets"
            | "jobs"
            | "services"
    )
}

/// Opens the log view of `target` with `query` in the search box and only matching lines shown,
/// e.g. the Argo CD application controller's lines about one app.
pub fn open_filtered(target: ResourceRef, query: String, window: &mut Window, cx: &mut App) {
    cx.set_global(view::PendingSearch(Some((
        target.clone(),
        query,
        std::time::Instant::now(),
    ))));
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(ViewKind::Logs, target))),
        cx,
    );
}

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    Settings::register::<LogsSettings>(cx);
    SessionRegistry::install(cx);
    view::init(cx);
    dock::init(cx);

    ViewRegistry::register(cx, ViewKind::Logs, |request, window, cx| {
        let target = request.target.clone();
        Some(Box::new(cx.new(|cx| LogsView::new(target, window, cx))))
    });

    ChromeRegistry::add_dock_panel(cx, ActiveSessionsPanel);
    ChromeRegistry::add_status_item(cx, SessionsStatusItem);

    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Show Logs", ShowLogs)
            .hint("Logs")
            .bind("l", Some("ResourceList"))
            .available_when(|target, _| logs_applicable(&target.gvr.resource)),
    );

    cx.on_action(|_: &ShowLogs, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
            .filter(|t| t.is_object() && logs_applicable(&t.gvr.resource))
        else {
            return;
        };
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window: &mut Window, cx| {
                        window.dispatch_action(
                            Box::new(OpenView(ViewRequest::for_resource(ViewKind::Logs, target))),
                            cx,
                        )
                    })
                    .ok();
            }
        });
    });
}
