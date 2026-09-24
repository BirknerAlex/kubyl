//! The "Active Sessions" right-dock panel: log streams, terminals and port-forwards from
//! [`crate::sessions::SessionRegistry`], each with a stop button. Also the status-bar counter.

use gpui::{
    AnyView, App, AppContext as _, Context, FocusHandle, Focusable, IntoElement, SharedString,
    Subscription, Window, div, prelude::*,
};
use kubyl_core::{DockPanel, DockPosition, StatusBarItem, StatusBarPosition, TabView};
use kubyl_ui::{ActiveColors, Icon, IconName, h_flex, tone_color, u, v_flex};

use crate::sessions::{SessionKind, SessionRegistry};

pub struct ActiveSessionsPanel;

impl DockPanel for ActiveSessionsPanel {
    fn id(&self) -> &'static str {
        "active-sessions"
    }

    fn position(&self) -> DockPosition {
        DockPosition::Right
    }

    fn order(&self) -> i32 {
        50
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> Box<dyn kubyl_core::TabHandle> {
        Box::new(cx.new(ActiveSessionsView::new))
    }
}

struct ActiveSessionsView {
    focus: FocusHandle,
    _subscription: Subscription,
}

impl ActiveSessionsView {
    fn new(cx: &mut Context<Self>) -> Self {
        let registry = SessionRegistry::global(cx);
        Self {
            focus: cx.focus_handle(),
            _subscription: cx.observe(&registry, |_, _, cx| cx.notify()),
        }
    }
}

impl Focusable for ActiveSessionsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ActiveSessionsView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Active Sessions".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Activity.path())
    }
}

impl Render for ActiveSessionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let registry = SessionRegistry::global(cx);
        let sessions = registry.read(cx).all().to_vec();

        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.panel)
            .when(sessions.is_empty(), |this| {
                this.items_center().justify_center().child(
                    div()
                        .text_color(colors.text_faint)
                        .child("No active logs, terminals or port-forwards."),
                )
            })
            .children(sessions.into_iter().map(|session| {
                let id = session.id;
                let icon = match session.kind {
                    SessionKind::Logs => IconName::Terminal,
                    SessionKind::Terminal => IconName::Terminal,
                    SessionKind::PortForward => IconName::Network,
                };
                h_flex()
                    .id(("session", id.raw() as usize))
                    .px(u(10.0))
                    .py(u(8.0))
                    .gap(u(8.0))
                    .items_center()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(icon).size(14.0).color(colors.text_dim))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_color(colors.text).child(session.title.clone()))
                            .child(
                                div()
                                    .text_size(u(11.0))
                                    .text_color(colors.text_faint)
                                    .child(session.subtitle.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(u(11.0))
                            .text_color(tone_color(session.tone, &colors))
                            .child(session.status.clone()),
                    )
                    .child(
                        div()
                            .id(("stop", id.raw() as usize))
                            .cursor_pointer()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |_, _, _, cx| SessionRegistry::stop(cx, id)),
                            )
                            .child(Icon::new(IconName::X).size(12.0).color(colors.text_faint)),
                    )
            }))
    }
}

/// Status-bar item: `N watches` (placeholder, `kubyl_resources` doesn't expose a global watch
/// count yet — see the phase 05 handoff log) and `N forwards`.
pub struct SessionsStatusItem;

impl StatusBarItem for SessionsStatusItem {
    fn id(&self) -> &'static str {
        "active-sessions"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Right
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(SessionsStatusView::new).into()
    }
}

struct SessionsStatusView {
    _subscription: Subscription,
}

impl SessionsStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let registry = SessionRegistry::global(cx);
        Self {
            _subscription: cx.observe(&registry, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for SessionsStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let registry = SessionRegistry::global(cx);
        let registry = registry.read(cx);
        let forwards = registry.count(SessionKind::PortForward);
        let terminals = registry.count(SessionKind::Terminal);
        let logs = registry.count(SessionKind::Logs);
        h_flex()
            .id("sessions-status")
            .gap(u(8.0))
            .text_color(colors.text_dim)
            .when(logs + terminals + forwards == 0, |this| this)
            .when(logs > 0, |this| this.child(format!("{logs} logs")))
            .when(terminals > 0, |this| {
                this.child(format!("{terminals} shells"))
            })
            .when(forwards > 0, |this| {
                this.child(format!("{forwards} forwards"))
            })
    }
}
