//! The status bar item: the MCP server is running, failed, or a change waits for the user.

use gpui::{AnyView, App, AppContext as _, Context, IntoElement, Render, Window, div, prelude::*};
use kubyl_core::{StatusBarItem, StatusBarPosition};
use kubyl_mcp_core::hub::ServerState;
use kubyl_ui::{ActiveColors as _, Icon, IconName, h_flex, u};

use crate::service::McpService;

pub struct McpStatusItem;

impl StatusBarItem for McpStatusItem {
    fn id(&self) -> &'static str {
        "mcp"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Right
    }

    fn order(&self) -> i32 {
        -10
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(McpStatusView::new).into()
    }
}

struct McpStatusView;

impl McpStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(service) = McpService::global(cx) {
            cx.observe(&service, |_, _, cx| cx.notify()).detach();
        }
        Self
    }
}

impl Render for McpStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(service) = McpService::global(cx) else {
            return div().into_any_element();
        };
        let service = service.read(cx);
        let (label, color) = if !service.pending().is_empty() {
            ("MCP: change waiting".to_string(), colors.yellow)
        } else {
            match service.state() {
                ServerState::Off | ServerState::Starting => return div().into_any_element(),
                ServerState::Running { port } => (format!("MCP :{port}"), colors.text_muted),
                ServerState::Failed(_) => ("MCP failed".to_string(), colors.red),
            }
        };
        h_flex()
            .id("mcp-status")
            .gap(u(4.0))
            .px(u(6.0))
            .cursor_pointer()
            .hover(|this| this.bg(colors.hover))
            .on_click(|_, window, cx| crate::open_status(window, cx))
            .child(Icon::new(IconName::Server).size(12.0).color(color))
            .child(div().text_size(u(11.5)).text_color(color).child(label))
            .into_any_element()
    }
}
