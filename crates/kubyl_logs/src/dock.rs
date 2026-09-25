//! The "Active Sessions" right-dock panel (board 2): log streams, terminals and port-forwards
//! from [`crate::sessions::SessionRegistry`], plus the resource watches of
//! `kubyl_resources::ResourceStores`, each with its status and a stop (or resume) button. Also
//! the status-bar counters, which open the panel.

use std::time::{Duration, Instant};

use gpui::{
    AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use kubyl_core::actions::ActivateDockPanel;
use kubyl_core::{
    ActionRegistry, ActionSpec, DockPanel, DockPosition, StatusBarItem, StatusBarPosition, TabView,
    Tone,
};
use kubyl_resources::{ResourceStores, StoreStatus, WatchInfo};
use kubyl_ui::{ActiveColors, Colors, Icon, IconButton, IconName, h_flex, tone_color, u, v_flex};

use crate::sessions::{SessionInfo, SessionKind, SessionRegistry};

/// [`DockPanel::id`] of the panel, for [`ActivateDockPanel`].
pub const PANEL_ID: &str = "active-sessions";

actions!(
    sessions,
    [
        /// Shows the Active Sessions panel.
        ShowActiveSessions,
    ]
);

pub(crate) fn init(cx: &mut App) {
    ActionRegistry::register(
        cx,
        ActionSpec::new("View: Active Sessions", ShowActiveSessions),
    );
    cx.on_action(|_: &ShowActiveSessions, cx| show_panel(cx));
}

/// Opens the Active Sessions panel in the focused window.
pub fn show_panel(cx: &mut App) {
    cx.defer(|cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window
                .update(cx, |_, window, cx| {
                    window.dispatch_action(Box::new(ActivateDockPanel(PANEL_ID.into())), cx)
                })
                .ok();
        }
    });
}

pub struct ActiveSessionsPanel;

impl DockPanel for ActiveSessionsPanel {
    fn id(&self) -> &'static str {
        PANEL_ID
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

/// Re-renders views that show watches, which change without an event (a store starts or stops
/// inside its own entity).
fn tick<V: 'static>(cx: &mut Context<V>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    })
}

struct ActiveSessionsView {
    focus: FocusHandle,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl ActiveSessionsView {
    fn new(cx: &mut Context<Self>) -> Self {
        let registry = SessionRegistry::global(cx);
        Self {
            focus: cx.focus_handle(),
            _tick: tick(cx),
            _subscriptions: vec![
                cx.observe(&registry, |_, _, cx| cx.notify()),
                cx.observe_global::<ResourceStores>(|_, cx| cx.notify()),
            ],
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

fn session_icon(kind: SessionKind) -> IconName {
    match kind {
        SessionKind::Logs => IconName::List,
        SessionKind::Terminal => IconName::Terminal,
        SessionKind::PortForward => IconName::Link,
    }
}

/// `4m`, `2h`, `3d`.
pub fn age(since: Instant, now: Instant) -> String {
    let secs = now.saturating_duration_since(since).as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

fn watch_status(watch: &WatchInfo, cx: &App) -> (String, Tone, bool) {
    let store = watch.store.read(cx);
    let objects = store.len();
    let views = match watch.users {
        0 => "idle".to_string(),
        1 => "1 view".to_string(),
        n => format!("{n} views"),
    };
    let (state, tone) = match store.status() {
        StoreStatus::Ready => (format!("{objects} objects"), Tone::Good),
        StoreStatus::Loading => ("listing…".into(), Tone::Info),
        StoreStatus::Waiting => ("waiting for the cluster".into(), Tone::Muted),
        StoreStatus::Forbidden => ("forbidden".into(), Tone::Bad),
        StoreStatus::Unsupported => ("not served".into(), Tone::Muted),
        StoreStatus::Error(err) => (format!("retrying: {err}"), Tone::Warning),
        StoreStatus::Paused => (format!("paused · {objects} objects"), Tone::Muted),
    };
    let paused = matches!(store.status(), StoreStatus::Paused);
    (format!("watch · {state} · {views}"), tone, paused)
}

fn row(id: impl Into<gpui::ElementId>, colors: &Colors) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id)
        .px(u(14.0))
        .py(u(9.0))
        .gap(u(10.0))
        .items_start()
        .border_b_1()
        .border_color(colors.border_variant)
}

fn texts(title: SharedString, detail: SharedString, colors: &Colors) -> impl IntoElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .truncate()
                .text_size(u(12.5))
                .text_color(colors.text)
                .child(title),
        )
        .child(
            div()
                .truncate()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child(detail),
        )
}

impl ActiveSessionsView {
    fn render_session(
        &self,
        session: SessionInfo,
        now: Instant,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let id = session.id;
        let mut detail = Vec::new();
        if !session.subtitle.is_empty() {
            detail.push(session.subtitle.to_string());
        }
        detail.push(session.status.to_string());
        detail.push(age(session.started, now));
        row(("session", id.raw() as usize), colors)
            .child(
                Icon::new(session_icon(session.kind))
                    .size(14.0)
                    .color(tone_color(session.tone, colors)),
            )
            .child(texts(
                session.title.clone(),
                detail.join(" · ").into(),
                colors,
            ))
            .children(session.buttons.into_iter().map(|button| {
                let on_click = button.on_click.clone();
                let tooltip = button.tooltip.clone();
                div()
                    .id(SharedString::from(format!(
                        "session-tip-{}-{}",
                        id.raw(),
                        button.id
                    )))
                    .flex_none()
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                    })
                    .child(
                        IconButton::new(
                            SharedString::from(format!("session-{}-{}", id.raw(), button.id)),
                            button.icon,
                        )
                        .icon_size(12.0)
                        .on_click(move |_, window, cx| on_click(window, cx)),
                    )
            }))
            .child(
                IconButton::new(
                    SharedString::from(format!("stop-{}", id.raw())),
                    IconName::X,
                )
                .icon_size(12.0)
                .on_click(cx.listener(move |_, _, _, cx| SessionRegistry::stop(cx, id))),
            )
            .into_any_element()
    }

    fn render_watch(
        &self,
        ix: usize,
        watch: WatchInfo,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let (detail, tone, paused) = watch_status(&watch, cx);
        let namespace = watch
            .key
            .namespace
            .clone()
            .unwrap_or_else(|| "all namespaces".into());
        let mut title = format!("{} · {namespace}", watch.key.gvr.resource);
        if let Some(selector) = &watch.key.label_selector {
            title.push_str(&format!(" · {selector}"));
        }
        let store = watch.store.clone();
        row(("watch", ix), colors)
            .child(
                Icon::new(IconName::Zap)
                    .size(14.0)
                    .color(tone_color(tone, colors)),
            )
            .child(texts(title.into(), detail.into(), colors))
            .child(
                IconButton::new(
                    ("watch-toggle", ix),
                    if paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    },
                )
                .icon_size(12.0)
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| {
                        if paused {
                            store.resume(cx)
                        } else {
                            store.pause(cx)
                        }
                    })
                }),
            )
            .into_any_element()
    }
}

impl Render for ActiveSessionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let registry = SessionRegistry::global(cx);
        let sessions = registry.read(cx).all().to_vec();
        let watches: Vec<WatchInfo> = ResourceStores::watches(cx);
        let now = Instant::now();
        let count = sessions.len() + watches.len();
        let session_rows: Vec<_> = sessions
            .into_iter()
            .map(|session| self.render_session(session, now, &colors, cx))
            .collect();
        let watch_rows: Vec<_> = watches
            .into_iter()
            .enumerate()
            .map(|(ix, watch)| self.render_watch(ix, watch, &colors, cx))
            .collect();

        v_flex()
            .id("active-sessions")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .bg(colors.panel)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(34.0))
                    .px(u(14.0))
                    .gap(u(6.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .text_color(colors.text)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Active sessions"),
                    )
                    .child(
                        div()
                            .font_family(kubyl_ui::fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(count.to_string()),
                    ),
            )
            .when(count == 0, |this| {
                this.child(
                    div()
                        .p(u(16.0))
                        .text_color(colors.text_faint)
                        .child("No active logs, terminals, port-forwards or watches."),
                )
            })
            .children(session_rows)
            .children(watch_rows)
    }
}

/// Status-bar item: `14 watches`, `2 logs`, `1 shell`, `2 forwards`. Clicking it opens the
/// Active Sessions panel.
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
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl SessionsStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let registry: Entity<SessionRegistry> = SessionRegistry::global(cx);
        Self {
            _tick: tick(cx),
            _subscriptions: vec![
                cx.observe(&registry, |_, _, cx| cx.notify()),
                cx.observe_global::<ResourceStores>(|_, cx| cx.notify()),
            ],
        }
    }
}

/// `1 shell`, `3 shells`.
fn counted(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
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
        let watches = ResourceStores::watch_count(cx);
        let item = |icon: IconName, label: String| {
            h_flex()
                .gap(u(5.0))
                .child(Icon::new(icon).size(12.0).color(colors.text_dim))
                .child(label)
        };
        h_flex()
            .id("sessions-status")
            .gap(u(14.0))
            .cursor_pointer()
            .text_color(colors.text_dim)
            .on_click(|_, _, cx| show_panel(cx))
            .when(logs > 0, |this| {
                this.child(item(IconName::List, counted(logs, "log", "logs")))
            })
            .when(terminals > 0, |this| {
                this.child(item(
                    IconName::Terminal,
                    counted(terminals, "shell", "shells"),
                ))
            })
            .when(forwards > 0, |this| {
                this.child(item(
                    IconName::Link,
                    counted(forwards, "forward", "forwards"),
                ))
            })
            .child(item(IconName::Zap, counted(watches, "watch", "watches")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_are_compact() {
        let start = Instant::now();
        assert_eq!(age(start, start + Duration::from_secs(42)), "42s");
        assert_eq!(age(start, start + Duration::from_secs(4 * 60)), "4m");
        assert_eq!(age(start, start + Duration::from_secs(3 * 3600)), "3h");
        assert_eq!(age(start, start + Duration::from_secs(2 * 86400)), "2d");
    }

    #[test]
    fn counts_use_singular_and_plural() {
        assert_eq!(counted(1, "shell", "shells"), "1 shell");
        assert_eq!(counted(3, "shell", "shells"), "3 shells");
        assert_eq!(counted(0, "watch", "watches"), "0 watches");
    }
}
