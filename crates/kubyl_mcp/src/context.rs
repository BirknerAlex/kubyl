//! What the tools of a cluster can reach right now, read from the app's services.

use std::sync::Arc;

use gpui::App;
use kubyl_alerts::AlertsService;
use kubyl_alerts::model::Alert;
use kubyl_core::{ActiveContext, ClusterId};
use kubyl_kube::ConnectionManager;
use kubyl_mcp_core::tools::{AlertSummary, ToolContext};
use kubyl_metrics::MetricsService;
use kubyl_resources::ResourceSelection;

fn alert_summary(alert: &Alert) -> AlertSummary {
    AlertSummary {
        name: alert.name.clone(),
        severity: alert.severity.label().to_string(),
        state: alert.state.label().to_string(),
        since: alert.since(),
        summary: alert.summary().to_string(),
        labels: alert.labels.clone(),
    }
}

/// What `cluster`'s tools can reach right now.
pub fn tool_context(cluster: &ClusterId, cx: &App) -> ToolContext {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return ToolContext::default();
    };
    let manager = manager.read(cx);
    let selection = ResourceSelection::global(cx)
        .primary()
        .filter(|s| &s.target.cluster == cluster);
    let active = ActiveContext::global(cx);
    let is_active = active.cluster.as_ref().is_some_and(|c| &c.id == cluster);
    let namespace = is_active
        .then(|| active.namespace.as_ref().map(|n| n.to_string()))
        .flatten();
    let prometheus = MetricsService::global(cx).and_then(|m| m.read(cx).prometheus(cluster));
    let alerts = AlertsService::global(cx).and_then(|service| {
        let service = service.read(cx);
        let state = service.cluster(cluster, kubyl_alerts::service::Pace::Background)?;
        Some(Arc::new(state.alerts.iter().map(alert_summary).collect()))
    });
    ToolContext {
        cluster: Some(cluster.clone()),
        cluster_name: manager.display_name(cluster).to_string(),
        client: manager.client(cluster),
        discovery: manager.discovery(cluster),
        caps: manager.caps(cluster),
        namespace,
        selection: selection.map(|s| s.target.clone()),
        selection_kind: selection.map(|s| s.kind.clone()),
        prometheus,
        alerts,
        max_output: 0,
        log_lines: 0,
        active: is_active,
        status: manager.state(cluster).label(),
    }
}

/// The contexts of every cluster the user didn't hide, for the outside MCP server.
pub fn all_tool_contexts(cx: &App) -> Vec<ToolContext> {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return Vec::new();
    };
    let ids: Vec<ClusterId> = manager.read(cx).contexts().map(|c| c.id.clone()).collect();
    ids.iter().map(|id| tool_context(id, cx)).collect()
}
