//! Status bar item: latency of the active cluster, how many clusters are connected and how
//! many API server connections are open.

use std::time::Duration;

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, Render, Subscription, Task, Window, div,
    prelude::*,
};
use gpui_component::tooltip::Tooltip;
use kubyl_core::{StatusBarItem, StatusBarPosition};
use kubyl_ui::{ActiveColors, Icon, IconName, h_flex, u, v_flex};

use super::open_clusters;
use crate::transport::{self, OpenConnections};
use crate::{ConnectionManager, ConnectionState};

/// How often the connection and open-file counts are refreshed.
const REFRESH: Duration = Duration::from_secs(2);

pub struct ConnectionStatusItem;

impl StatusBarItem for ConnectionStatusItem {
    fn id(&self) -> &'static str {
        "kube-connections"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Left
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(ConnectionStatusView::new).into()
    }
}

struct ConnectionStatusView {
    connections: OpenConnections,
    /// Open file descriptors and the soft limit.
    files: Option<(usize, Option<u64>)>,
    _subscription: Subscription,
    _refresh: Task<()>,
}

impl ConnectionStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let manager = ConnectionManager::global(cx);
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                let files = cx
                    .background_executor()
                    .spawn(async { transport::open_files() })
                    .await;
                let connections = transport::open_connections();
                let updated = this.update(cx, |this, cx| {
                    if this.connections != connections || this.files != files {
                        this.connections = connections;
                        this.files = files;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        Self {
            connections: OpenConnections::default(),
            files: None,
            _subscription: cx.observe(&manager, |_, _, cx| cx.notify()),
            _refresh: refresh,
        }
    }
}

/// `3 HTTP/2, 1 HTTP/1.1`, leaving out protocols without connections.
fn by_protocol(open: &OpenConnections) -> String {
    let mut parts = Vec::new();
    if open.http2 > 0 {
        parts.push(format!("{} HTTP/2", open.http2));
    }
    if open.http1 > 0 {
        parts.push(format!("{} HTTP/1.1", open.http1));
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

/// The tooltip: connections per cluster and the process's open files.
fn tooltip_lines(
    total: &OpenConnections,
    clusters: &[(gpui::SharedString, OpenConnections)],
    files: Option<(usize, Option<u64>)>,
) -> Vec<String> {
    let noun = if total.total() == 1 {
        "connection"
    } else {
        "connections"
    };
    let mut lines = vec![format!(
        "{} API server {noun} ({})",
        total.total(),
        by_protocol(total)
    )];
    lines.extend(
        clusters
            .iter()
            .map(|(name, open)| format!("{name}: {}", by_protocol(open))),
    );
    let listed: usize = clusters.iter().map(|(_, open)| open.total()).sum();
    if total.total() > listed {
        lines.push(format!("{} closing", total.total() - listed));
    }
    match files {
        Some((open, Some(limit))) => lines.push(format!("Open files: {open} of {limit}")),
        Some((open, None)) => lines.push(format!("Open files: {open}")),
        None => {}
    }
    lines
}

impl Render for ConnectionStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        let active = manager.active().map(|id| manager.state(id));
        let connected = manager.connected_count();
        let total = self.connections;
        let files = self.files;
        let (text, color) = match &active {
            Some(ConnectionState::Connected { latency, .. }) => (
                format!("{} ms", latency.as_millis().max(1)),
                colors.text_dim,
            ),
            Some(ConnectionState::Connecting) => ("connecting…".to_string(), colors.yellow),
            Some(state @ ConnectionState::AuthRequired { .. }) => {
                (state.label().to_lowercase(), colors.yellow)
            }
            Some(ConnectionState::Unreachable {
                retry_in: Some(delay),
                ..
            }) => (
                format!("unreachable · retry in {}s", delay.as_secs()),
                colors.red,
            ),
            Some(state @ (ConnectionState::Unreachable { .. } | ConnectionState::Forbidden(_))) => {
                (state.label().to_lowercase(), colors.red)
            }
            _ => (String::new(), colors.text_dim),
        };
        h_flex()
            .id("kube-connections")
            .gap(u(5.0))
            .cursor_pointer()
            .on_click(|_, window, cx| open_clusters(window, cx))
            .when(!text.is_empty(), |this| {
                this.child(Icon::new(IconName::Activity).size(12.0).color(color))
                    .child(div().text_color(color).child(text))
            })
            .when(connected > 1, |this| {
                this.child(
                    div()
                        .text_color(colors.text_dim)
                        .child(format!("· {connected} clusters connected")),
                )
            })
            .when(total.total() > 0, |this| {
                let noun = if total.total() == 1 {
                    "connection"
                } else {
                    "connections"
                };
                this.child(
                    div()
                        .text_color(colors.text_dim)
                        .child(format!("· {} {noun}", total.total())),
                )
                .tooltip(move |window, cx| {
                    Tooltip::element(move |_, cx| {
                        let clusters = ConnectionManager::global(cx).read(cx).open_connections();
                        v_flex().children(
                            tooltip_lines(&transport::open_connections(), &clusters, files)
                                .into_iter()
                                .map(|line| div().child(line)),
                        )
                    })
                    .build(window, cx)
                })
            })
    }
}
