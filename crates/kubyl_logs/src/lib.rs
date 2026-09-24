//! Log streaming, search and level parsing (board 2 · Live logs).
//!
//! - [`view::LogsView`]: `ViewKind::Logs`, opened for a Pod (all containers) or a
//!   Deployment/StatefulSet/DaemonSet/Job (its pod-template selector, so new/killed pods join
//!   and leave live).
//! - [`stream`]: the kube-side log streaming, with per-container reconnect and a gap marker on
//!   reconnect.
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
use kubyl_core::{ActionRegistry, ActionSpec, ChromeRegistry, ViewKind, ViewRegistry, ViewRequest};
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

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    Settings::register::<LogsSettings>(cx);
    SessionRegistry::install(cx);

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
            .available_when(|target, _| {
                matches!(
                    target.gvr.resource.as_str(),
                    "pods" | "deployments" | "statefulsets" | "daemonsets" | "jobs"
                )
            }),
    );

    cx.on_action(|_: &ShowLogs, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
            .filter(|t| t.is_object())
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
