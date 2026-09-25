//! The right-dock Events panel (board 4): the live stream of the active cluster.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Window, div, prelude::*,
};
use kubyl_core::{ActiveContext, DockPanel, DockPosition, TabHandle, TabView};
use kubyl_ui::{ActiveColors, IconName, h_flex, u, v_flex};

use super::feed::EventsFeed;
use super::model::Filter;
use super::ui;

/// Id of the panel, for `ActivateDockPanel`.
pub const PANEL_ID: &str = "events";
/// Rows rendered at most (the Events view shows everything).
const MAX_ROWS: usize = 200;

pub struct EventsDock;

impl DockPanel for EventsDock {
    fn id(&self) -> &'static str {
        PANEL_ID
    }

    fn position(&self) -> DockPosition {
        DockPosition::Right
    }

    fn order(&self) -> i32 {
        10
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> Box<dyn TabHandle> {
        Box::new(cx.new(EventsPanel::new))
    }
}

pub struct EventsPanel {
    feed: Entity<EventsFeed>,
    filter: Filter,
    /// Namespace picked in the chip (`Some(None)` = all); `None` follows the active namespace.
    picked: Option<Option<String>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventsPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let feed = cx.new(EventsFeed::new);
        let subscriptions = vec![
            cx.observe(&feed, |_, _, cx| cx.notify()),
            cx.observe_global::<ActiveContext>(|this, cx| this.follow(cx)),
        ];
        let mut this = Self {
            feed,
            filter: Filter::default(),
            picked: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.follow(cx);
        this
    }

    /// Follows the active cluster (and namespace, unless one was picked in the chip).
    fn follow(&mut self, cx: &mut Context<Self>) {
        let active = ActiveContext::global(cx).clone();
        let cluster = active.cluster.map(|c| c.id);
        if cluster.as_ref() != self.feed.read(cx).cluster() {
            self.picked = None;
        }
        let namespace = match &self.picked {
            Some(picked) => picked.clone(),
            None => active.namespace.map(|n| n.to_string()),
        };
        self.feed
            .update(cx, |feed, cx| feed.set_scope(cluster, namespace, cx));
    }
}

impl Focusable for EventsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for EventsPanel {
    fn tab_title(&self, _: &App) -> SharedString {
        "Events".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Bell.path())
    }
}

impl Render for EventsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let feed = self.feed.read(cx);
        let rows = feed.rows();
        let status = feed.status(cx);
        let now = jiff::Timestamp::now();
        let weak = cx.weak_entity();
        let on_type = {
            let weak = weak.clone();
            Rc::new(move |value: Option<bool>, _: &mut Window, cx: &mut App| {
                weak.update(cx, |this, cx| {
                    this.filter.warnings = value;
                    cx.notify();
                })
                .ok();
            })
        };
        let on_namespace = Rc::new(move |ns: Option<String>, _: &mut Window, cx: &mut App| {
            weak.update(cx, |this, cx| {
                this.picked = Some(ns);
                this.follow(cx);
            })
            .ok();
        });
        let visible: Vec<_> = rows
            .iter()
            .filter(|r| self.filter.matches(r))
            .take(MAX_ROWS)
            .enumerate()
            .map(|(i, row)| ui::event_item(i, row, &self.feed, now, &colors))
            .collect();
        let empty = visible.is_empty();
        v_flex()
            .track_focus(&self.focus)
            .size_full()
            .text_color(colors.text)
            .child(
                h_flex()
                    .flex_none()
                    .gap(u(4.0))
                    .px(u(14.0))
                    .py(u(8.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(ui::type_chips(&rows, &self.filter, on_type, &colors))
                    .child(ui::namespace_chip(&self.feed, on_namespace, cx))
                    .child(div().flex_1())
                    .child(ui::live_indicator(&status, &colors))
                    .child(ui::pause_button(&self.feed, cx)),
            )
            .child(
                div()
                    .id("events-stream")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(visible)
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p(u(14.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child("No events."),
                        )
                    }),
            )
    }
}
