//! The Security Center (phase 25, board 23): what Trivy Operator found, as three views over
//! its report CRDs: **Images** (vulnerabilities by image, with the exposed secrets in them),
//! **Resources** (config audits by workload) and **Roles** (RBAC assessments). Each has a
//! severity summary, a filter, a details panel with the findings and a CSV export.
//!
//! Lists come from the reports' metadata and the API server's printer columns; a report's
//! findings are fetched when it is opened. Exposed secrets are listed by rule and file, never
//! their text. Where the operator isn't installed the tab offers to install it through the Helm
//! flow (hidden on read-only clusters; PROD asks for the cluster's name there) or shows the
//! command.
//!
//! - [`feed`]: the watches and tables. [`view`]: the tab. [`service`]: the sidebar badge.
//! - The logic (rows, aggregation, details parsing, CSV) is in `kubyl_security_core`.

pub mod feed;
pub mod overview;
pub mod service;
pub mod view;

pub use kubyl_security_core::{aggregate, details, install, kinds, model, table};

use std::sync::Arc;

use gpui::{App, AppContext as _, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, Notification, NotificationCenter,
    ResourceRef, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_explorer::catalog::{self, RowBadge, ViewRow};
use kubyl_ui::IconName;

actions!(
    security,
    [
        /// Opens the Security Center of the active cluster.
        ShowSecurity,
    ]
);

/// The view id of the tab.
pub const VIEW_KIND: &str = "security";

/// Registers the view, the sidebar row, the palette entries and the actions.
pub fn init(cx: &mut App) {
    service::SecurityService::install(cx);
    kubyl_core::ChromeRegistry::add_overview_section(cx, overview::SecurityOverview);
    view::init(cx);
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.clone()?;
            let tab = kinds::View::from_id(&target.gvr.resource);
            Some(Box::new(cx.new(|cx| {
                view::SecurityView::new(
                    target.cluster,
                    tab.unwrap_or(kinds::View::Images),
                    window,
                    cx,
                )
            })))
        },
    );
    catalog::register_view_row(
        cx,
        ViewRow {
            id: "security",
            // Under Alerts in the sidebar.
            after: "alerts",
            label: "Security",
            icon: IconName::Shield,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: None,
            badge: Some(Arc::new(row_badge)),
        },
    );
    kubyl_palette::register_view(
        cx,
        kubyl_palette::PaletteView {
            name: "security",
            aliases: vec!["trivy", "vulnerabilities"],
            detail: "Security Center (Trivy Operator reports)",
            icon: IconName::Shield,
            kind: ViewKind::Custom(VIEW_KIND.into()),
            visible: Arc::new(|_, _| true),
        },
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Security: Open Security Center", ShowSecurity),
    );
    cx.on_action(|_: &ShowSecurity, cx| {
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
                    .update(cx, |_, window, cx| {
                        open(&cluster, kinds::View::Images, window, cx)
                    })
                    .ok();
            }
        });
    });
}

/// The ref a tab is opened for; the resource names the view (`images`, `resources`, `roles`).
pub fn target(cluster: &ClusterId, view: kinds::View) -> ResourceRef {
    ResourceRef::list(cluster.clone(), Gvr::new("", "", view.id()), None)
}

/// Opens the Security Center of a cluster on a view.
pub fn open(cluster: &ClusterId, view: kinds::View, window: &mut gpui::Window, cx: &mut App) {
    window.dispatch_action(
        Box::new(OpenView(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            target(cluster, view),
        ))),
        cx,
    );
}

/// The critical findings in red, nothing while there are none (or no Trivy to ask).
pub fn badge(criticals: Option<u32>) -> Option<RowBadge> {
    let count = criticals.filter(|c| *c > 0)?;
    Some(RowBadge::Count {
        text: count.to_string().into(),
        tone: kubyl_core::Tone::Bad,
    })
}

fn row_badge(cluster: &ClusterId, cx: &App) -> Option<RowBadge> {
    let service = service::SecurityService::global(cx)?;
    badge(service.read(cx).criticals(cluster))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_shows_critical_findings_in_red_only() {
        assert_eq!(badge(None), None);
        assert_eq!(badge(Some(0)), None);
        assert_eq!(
            badge(Some(14)),
            Some(RowBadge::Count {
                text: "14".into(),
                tone: kubyl_core::Tone::Bad
            })
        );
    }
}
