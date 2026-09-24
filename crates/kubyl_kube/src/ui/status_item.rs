//! Status bar item: latency of the active cluster and how many clusters are connected.

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, Render, Subscription, Window, div,
    prelude::*,
};
use kubyl_core::{StatusBarItem, StatusBarPosition};
use kubyl_ui::{ActiveColors, Icon, IconName, h_flex, u};

use super::open_clusters;
use crate::{ConnectionManager, ConnectionState};

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
    _subscription: Subscription,
}

impl ConnectionStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let manager = ConnectionManager::global(cx);
        Self {
            _subscription: cx.observe(&manager, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for ConnectionStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let manager = ConnectionManager::global(cx);
        let manager = manager.read(cx);
        let active = manager.active().map(|id| manager.state(id));
        let connected = manager.connected_count();
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
    }
}
