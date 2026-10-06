//! The status bar item: shows while an agent works or waits for the user; opens the panel.

use gpui::{AnyView, App, AppContext as _, Context, IntoElement, Render, Window, div, prelude::*};
use kubyl_core::{StatusBarItem, StatusBarPosition};
use kubyl_ui::{ActiveColors as _, Icon, IconName, h_flex, u};

use crate::service::{AgentService, ThreadId, ThreadStatus};

pub struct AgentStatusItem;

impl StatusBarItem for AgentStatusItem {
    fn id(&self) -> &'static str {
        "agent"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Right
    }

    fn order(&self) -> i32 {
        -20
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(AgentStatusView::new).into()
    }
}

struct AgentStatusView;

impl AgentStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(service) = AgentService::global(cx) {
            cx.observe(&service, |_, _, cx| cx.notify()).detach();
        }
        Self
    }
}

impl Render for AgentStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(service) = AgentService::global(cx) else {
            return div().into_any_element();
        };
        let service = service.read(cx);
        let waiting = service.threads().iter().any(|t| !t.pending.is_empty());
        let working = service.threads().iter().any(|t| {
            !t.is_unused() && matches!(t.status, ThreadStatus::Running | ThreadStatus::Starting)
        });
        if !waiting && !working {
            return div().into_any_element();
        }
        let (label, color) = if waiting {
            ("Agent needs you", colors.yellow)
        } else {
            ("Agent working…", colors.text_muted)
        };
        h_flex()
            .id("agent-status")
            .gap(u(4.0))
            .px(u(6.0))
            .cursor_pointer()
            .hover(|this| this.bg(colors.hover))
            .on_click(|_, window, cx| show_attention(window, cx))
            .child(Icon::new(IconName::Zap).size(12.0).color(color))
            .child(div().text_size(u(11.5)).text_color(color).child(label))
            .into_any_element()
    }
}

/// The thread that needs the user: the oldest one waiting for an answer, else the newest one
/// at work.
pub(crate) fn attention(service: &crate::service::AgentService) -> Option<ThreadId> {
    let threads = service.threads();
    threads
        .iter()
        .filter(|t| !t.pending.is_empty())
        .min_by_key(|t| t.pending.iter().map(|p| p.id).min())
        .or_else(|| {
            threads.iter().rev().find(|t| {
                !t.is_unused() && matches!(t.status, ThreadStatus::Running | ThreadStatus::Starting)
            })
        })
        .map(|t| t.id)
}

/// Opens the Agent panel on the thread that needs the user, scrolled to what it waits for.
pub(crate) fn show_attention(window: &mut Window, cx: &mut App) {
    let target = AgentService::global(cx).and_then(|s| attention(s.read(cx)));
    if let Some(panel) = crate::panel::show(window, cx)
        && let Some(thread) = target
    {
        panel.update(cx, |panel, cx| panel.show_thread(thread, cx));
    }
}
