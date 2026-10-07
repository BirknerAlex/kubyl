//! Picks a release of the active cluster for the palette's `Helm: Upgrade Release…`,
//! `Roll Back Release…` and `Uninstall Release…` when no release is focused.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Window, div, prelude::*,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::ClusterId;
use kubyl_ui::{ActiveColors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{footer, header, input_box, open};
use crate::releases::status_tone;
use crate::service::{Helm, HelmLease, ReleaseRow};
use crate::widgets;

type OnPick = Rc<dyn Fn(ReleaseRow, &mut Window, &mut App)>;

/// Opens the picker; `on_pick` runs with the chosen release after the picker closed.
pub fn pick_release(
    cluster: ClusterId,
    title: &'static str,
    on_pick: impl Fn(ReleaseRow, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let on_pick: OnPick = Rc::new(on_pick);
    let view = cx.new(|cx| ReleasePicker::new(cluster, title, on_pick, window, cx));
    let focus = view.read(cx).query.read(cx).focus_handle(cx);
    open(view, 560.0, Some(focus), window, cx);
}

pub struct ReleasePicker {
    cluster: ClusterId,
    title: &'static str,
    query: Entity<InputState>,
    on_pick: OnPick,
    focus: FocusHandle,
    _lease: Option<HelmLease>,
    _subscriptions: Vec<Subscription>,
}

impl ReleasePicker {
    fn new(
        cluster: ClusterId,
        title: &'static str,
        on_pick: OnPick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Filter releases"));
        let mut subscriptions =
            vec![
                cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => cx.notify(),
                        InputEvent::PressEnter { .. } => {
                            if let Some(row) = this.rows(cx).into_iter().next() {
                                this.pick(row, window, cx);
                            }
                        }
                        _ => {}
                    }
                }),
            ];
        if let Some(helm) = Helm::global(cx) {
            subscriptions.push(cx.observe(&helm, |_, _, cx| cx.notify()));
        }
        let lease = Helm::watch(&cluster, cx);
        Self {
            cluster,
            title,
            query,
            on_pick,
            focus: cx.focus_handle(),
            _lease: lease,
            _subscriptions: subscriptions,
        }
    }

    fn rows(&self, cx: &App) -> Vec<ReleaseRow> {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let Some(snapshot) = crate::releases::snapshot(&self.cluster, cx) else {
            return Vec::new();
        };
        let mut rows: Vec<ReleaseRow> = snapshot
            .releases
            .iter()
            .filter(|r| {
                query.is_empty()
                    || query
                        .split_whitespace()
                        .all(|w| format!("{} {}", r.name, r.namespace).contains(w))
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| (&a.name, &a.namespace).cmp(&(&b.name, &b.namespace)));
        rows
    }

    fn pick(&mut self, row: ReleaseRow, window: &mut Window, cx: &mut Context<Self>) {
        let on_pick = self.on_pick.clone();
        window.close_dialog(cx);
        window.defer(cx, move |window, cx| on_pick(row, window, cx));
    }
}

impl Focusable for ReleasePicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ReleasePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let rows = self.rows(cx);
        let list = v_flex()
            .id("release-picker")
            .h(u(320.0))
            .p(u(8.0))
            .gap(u(1.0))
            .overflow_y_scroll()
            .when(rows.is_empty(), |this| {
                this.child(widgets::empty("No Helm releases.", &colors))
            })
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                let status = row.latest().status.clone();
                h_flex()
                    .id(("release-pick", i))
                    .gap(u(8.0))
                    .px(u(10.0))
                    .py(u(6.0))
                    .rounded(u(5.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.hover))
                    .child(Icon::new(IconName::Anchor).size(13.0).color(colors.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.5))
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(row.namespace.clone()),
                    )
                    .child(widgets::pill(status.clone(), status_tone(&status), &colors))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick(row.clone(), window, cx)),
                    )
            }));
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                Icon::new(IconName::Anchor)
                    .size(16.0)
                    .color(colors.accent)
                    .into_any_element(),
                self.title,
                Vec::new(),
                &colors,
            ))
            .child(
                div()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(input_box(&self.query, false, &colors)),
            )
            .child(list)
            .child(footer(None, vec![super::close_button("Cancel")], &colors))
    }
}
