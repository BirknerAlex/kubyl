//! Pieces shared by the Events dock panel and the Events view.

use std::rc::Rc;

use gpui::{App, Entity, FontWeight, IntoElement, SharedString, Window, div, prelude::*};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use jiff::Timestamp;
use kubyl_core::actions::OpenView;
use kubyl_core::{ViewKind, ViewRequest};
use kubyl_kube::ConnectionManager;
use kubyl_resources::format::{human_duration, seconds_since};
use kubyl_ui::{Chip, Colors, Icon, IconButton, IconName, StatusDot, fonts, h_flex, u, v_flex};

use super::feed::{EventsFeed, FeedStatus, object_ref};
use super::model::{EventRow, Filter};

/// Called with the chosen type filter (`Some(true)`: warnings only).
pub type OnType = Rc<dyn Fn(Option<bool>, &mut Window, &mut App)>;
/// Called with the chosen namespace (`None`: all).
pub type OnNamespace = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;

/// `12s`, `4m`, `2d`.
pub fn age(row: &EventRow, now: Timestamp) -> String {
    row.last
        .map(|t| human_duration(seconds_since(t, now)))
        .unwrap_or_default()
}

/// `● live`, `paused · 3 new`, `loading…`.
pub fn live_indicator(status: &FeedStatus, colors: &Colors) -> impl IntoElement {
    let (label, color): (SharedString, _) = match status {
        FeedStatus::Live => ("live".into(), colors.green),
        FeedStatus::Paused { new: 0 } => ("paused".into(), colors.yellow),
        FeedStatus::Paused { new } => (format!("paused · {new} new").into(), colors.yellow),
        FeedStatus::Loading => ("loading…".into(), colors.text_dim),
        FeedStatus::Forbidden => ("forbidden".into(), colors.red),
        FeedStatus::Error(_) => ("reconnecting".into(), colors.red),
        FeedStatus::NoCluster => ("no cluster".into(), colors.text_dim),
    };
    h_flex()
        .flex_none()
        .gap(u(5.0))
        .text_size(u(12.0))
        .text_color(color)
        .child(StatusDot::new(color))
        .child(label)
}

/// The pause/resume button.
pub fn pause_button(feed: &Entity<EventsFeed>, cx: &App) -> impl IntoElement {
    let paused = feed.read(cx).is_paused();
    let feed = feed.clone();
    IconButton::new(
        "events-pause",
        if paused {
            IconName::Play
        } else {
            IconName::Pause
        },
    )
    .icon_size(12.0)
    .toggled(paused)
    .on_click(move |_, _, cx| feed.update(cx, |feed, cx| feed.set_paused(!paused, cx)))
}

/// `Warning 23` · `Normal 212` filter chips. Clicking one shows only that type; clicking it
/// again shows both.
pub fn type_chips(
    rows: &[EventRow],
    filter: &Filter,
    on_change: OnType,
    colors: &Colors,
) -> impl IntoElement {
    let (warnings, normals) = super::model::counts(rows);
    let chip = |id: &'static str, label: String, value: bool, color| {
        let selected = filter.warnings == Some(value);
        let on_change = on_change.clone();
        div()
            .id(id)
            .cursor_pointer()
            .child(Chip::new(label).selected(selected).text_color(color))
            .on_click(move |_, window, cx| {
                on_change((!selected).then_some(value), window, cx);
            })
    };
    h_flex()
        .flex_none()
        .gap(u(4.0))
        .child(chip(
            "chip-warnings",
            format!("Warning {warnings}"),
            true,
            colors.yellow,
        ))
        .child(chip(
            "chip-normal",
            format!("Normal {normals}"),
            false,
            if filter.warnings == Some(false) {
                colors.chip_selected_text
            } else {
                colors.text_muted
            },
        ))
}

/// `all namespaces ▾`: picks the namespace of the stream.
pub fn namespace_chip(
    feed: &Entity<EventsFeed>,
    on_pick: OnNamespace,
    cx: &App,
) -> impl IntoElement {
    let feed = feed.read(cx);
    let current = feed.namespace().map(str::to_string);
    let available = feed
        .cluster()
        .and_then(|c| ConnectionManager::try_global(cx).map(|m| m.read(cx).namespaces(c).names))
        .unwrap_or_default();
    MenuButton::new("events-namespace")
        .ghost()
        .compact()
        .p_0()
        .child(
            Chip::new(current.clone().unwrap_or_else(|| "all namespaces".into()))
                .icon(IconName::ChevronDown),
        )
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.max_h(gpui::px(420.0)).scrollable(true);
            let pick = on_pick.clone();
            menu = menu
                .item(
                    PopupMenuItem::new("All namespaces")
                        .checked(current.is_none())
                        .on_click(move |_, window, cx| pick(None, window, cx)),
                )
                .separator();
            for namespace in &available {
                let pick = on_pick.clone();
                let ns = namespace.clone();
                menu = menu.item(
                    PopupMenuItem::new(namespace.clone())
                        .checked(current.as_deref() == Some(namespace.as_str()))
                        .on_click(move |_, window, cx| pick(Some(ns.clone()), window, cx)),
                );
            }
            menu
        })
}

/// Opens the object an event is about in a Details tab.
pub fn open_object(row: &EventRow, feed: &Entity<EventsFeed>, window: &mut Window, cx: &mut App) {
    let Some(cluster) = feed.read(cx).cluster().cloned() else {
        return;
    };
    if let Some(target) = object_ref(row, &cluster, cx) {
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(
                ViewKind::Details,
                target,
            ))),
            cx,
        );
    }
}

/// One event in the dock (board 4): icon, reason, `×N age`, object link, message.
pub fn event_item(
    index: usize,
    row: &EventRow,
    feed: &Entity<EventsFeed>,
    now: Timestamp,
    colors: &Colors,
) -> impl IntoElement {
    let meta = match row.count {
        0 | 1 => age(row, now),
        n => format!("×{n} {}", age(row, now)),
    };
    let open_row = row.clone();
    let feed = feed.clone();
    h_flex()
        .items_start()
        .gap(u(10.0))
        .px(u(14.0))
        .py(u(10.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(div().pt(u(2.0)).child(if row.warning {
            Icon::new(IconName::TriangleAlert).color(colors.yellow)
        } else {
            Icon::new(IconName::Info).color(colors.text_dim)
        }))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .when(row.warning, |this| this.text_color(colors.yellow))
                                .child(row.reason.clone()),
                        )
                        .when(row.derived, |this| {
                            this.child(
                                div()
                                    .text_size(u(10.5))
                                    .text_color(colors.text_faint)
                                    .child("pod status"),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_none()
                                .font_family(fonts::MONO)
                                .text_size(u(11.0))
                                .text_color(colors.text_dim)
                                .child(meta),
                        ),
                )
                .child(
                    div()
                        .id(("event-object", index))
                        .truncate()
                        .cursor_pointer()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.accent)
                        .hover(|this| this.underline())
                        .child(row.object())
                        .on_click(move |_, window, cx| open_object(&open_row, &feed, window, cx)),
                )
                .child(
                    div()
                        .text_size(u(12.0))
                        .line_height(u(17.0))
                        .text_color(colors.text_muted)
                        .child(row.message.clone()),
                ),
        )
}
