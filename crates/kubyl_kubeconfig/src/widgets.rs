//! Small UI pieces shared by the editor, the wizard and the dialogs (board 11).

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, FontWeight, Hsla, IntoElement, SharedString,
    Window, div, prelude::*,
};
use gpui_component::input::{Input, InputState};
use kubyl_ui::{Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::conntest::Status;
use crate::validate::Severity;

/// A titled card (`CONTEXT`, `KUBYL OVERRIDES · settings.json`).
pub fn card(title: impl Into<SharedString>, note: Option<&str>, colors: &Colors) -> gpui::Div {
    let title: SharedString = title.into();
    v_flex()
        .gap(u(10.0))
        .p(u(14.0))
        .rounded(u(8.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.panel)
        .child(
            h_flex()
                .gap(u(6.0))
                .child(
                    div()
                        .text_size(u(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_dim)
                        .child(title.to_uppercase()),
                )
                .children(note.map(|n| {
                    div()
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(format!("· {n}"))
                })),
        )
}

/// A form row: a label column and the control.
pub fn row(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    colors: &Colors,
) -> gpui::Div {
    h_flex()
        .items_center()
        .gap(u(12.0))
        .min_h(u(30.0))
        .child(
            div()
                .w(u(150.0))
                .flex_none()
                .text_size(u(12.5))
                .text_color(colors.text_muted)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A row with a hint line under the control.
pub fn row_with_hint(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    hint: Option<AnyElement>,
    colors: &Colors,
) -> gpui::Div {
    let label: SharedString = label.into();
    h_flex()
        .items_start()
        .gap(u(12.0))
        .child(
            div()
                .w(u(150.0))
                .flex_none()
                .pt(u(6.0))
                .text_size(u(12.5))
                .text_color(colors.text_muted)
                .child(label),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(u(4.0))
                .child(control)
                .children(hint),
        )
}

/// A text input box styled like the mockups.
pub fn text_input(
    state: &Entity<InputState>,
    mono: bool,
    mask_toggle: bool,
    colors: &Colors,
) -> impl IntoElement {
    let mut input = Input::new(state).appearance(false);
    if mask_toggle {
        input = input.mask_toggle();
    }
    div()
        .h(u(28.0))
        .px(u(8.0))
        .flex()
        .items_center()
        .rounded(u(5.0))
        .bg(colors.input_background)
        .border_1()
        .border_color(colors.border)
        .when(mono, |this| {
            this.font_family(fonts::MONO).text_size(u(12.5))
        })
        .child(input)
}

/// A dim one-line hint.
pub fn hint(text: impl Into<SharedString>, colors: &Colors) -> AnyElement {
    div()
        .text_size(u(11.5))
        .text_color(colors.text_dim)
        .child(text.into())
        .into_any_element()
}

/// A switch.
pub fn toggle(
    id: impl Into<ElementId>,
    on: bool,
    enabled: bool,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let track = if on { colors.accent } else { colors.border };
    div()
        .id(id.into())
        .flex_none()
        .relative()
        .w(u(28.0))
        .h(u(16.0))
        .rounded(u(8.0))
        .bg(track)
        .when(!enabled, |this| this.opacity(0.45))
        .when(enabled, |this| {
            this.cursor_pointer().on_click(move |event, window, cx| {
                cx.stop_propagation();
                on_click(event, window, cx)
            })
        })
        .child(
            div()
                .absolute()
                .top(u(2.0))
                .when(on, |this| this.right(u(2.0)))
                .when(!on, |this| this.left(u(2.0)))
                .size(u(12.0))
                .rounded_full()
                .bg(gpui::white()),
        )
}

/// A switch with a title and a description (board 5's `opt`).
pub fn option_row(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    on: bool,
    enabled: bool,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    h_flex()
        .gap(u(12.0))
        .py(u(6.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_size(u(12.5)).child(title.into()))
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(detail.into()),
                ),
        )
        .child(toggle(id, on, enabled, colors, on_click))
}

/// A checkbox with a label.
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: bool,
    label: impl Into<SharedString>,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    h_flex()
        .id(id.into())
        .items_start()
        .gap(u(8.0))
        .cursor_pointer()
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx)
        })
        .child(
            div()
                .flex_none()
                .mt(u(1.0))
                .size(u(14.0))
                .rounded(u(3.0))
                .flex()
                .items_center()
                .justify_center()
                .map(|this| {
                    if checked {
                        this.bg(colors.accent).child(
                            Icon::new(IconName::Check)
                                .size(11.0)
                                .color(colors.on_accent),
                        )
                    } else {
                        this.border_1().border_color(colors.text_faint)
                    }
                }),
        )
        .child(div().text_size(u(12.5)).child(label))
}

/// A segmented control (`Form | YAML`, the CA source).
pub fn segmented<T: Clone + PartialEq + 'static>(
    id: &str,
    options: Vec<(T, SharedString, Option<IconName>)>,
    selected: T,
    colors: &Colors,
    on_select: impl Fn(&T, &mut Window, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let count = options.len();
    h_flex()
        .h(u(28.0))
        .rounded(u(5.0))
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .children(
            options
                .into_iter()
                .enumerate()
                .map(|(ix, (value, label, icon))| {
                    let on = value == selected;
                    let on_select = on_select.clone();
                    h_flex()
                        .id(ElementId::Name(format!("{id}-{ix}").into()))
                        .flex_1()
                        .h_full()
                        .px(u(10.0))
                        .gap(u(6.0))
                        .justify_center()
                        .text_size(u(12.5))
                        .cursor_pointer()
                        .when(ix + 1 < count, |this| {
                            this.border_r_1().border_color(colors.border)
                        })
                        .map(|this| {
                            if on {
                                this.bg(colors.chip_selected_background)
                                    .text_color(colors.chip_selected_text)
                            } else {
                                this.text_color(colors.text_muted)
                                    .hover(|s| s.bg(colors.hover))
                            }
                        })
                        .on_click(move |_, window, cx| on_select(&value, window, cx))
                        .children(icon.map(|i| Icon::new(i).size(12.0)))
                        .child(label)
                }),
        )
}

/// A warning or error box.
pub fn notice(severity: Severity, text: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    let (color, icon) = severity_style(severity, colors);
    h_flex()
        .items_start()
        .gap(u(8.0))
        .px(u(10.0))
        .py(u(8.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(color.opacity(0.45))
        .bg(color.opacity(0.08))
        .child(
            div()
                .pt(u(1.0))
                .child(Icon::new(icon).size(13.0).color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(u(12.0))
                .child(text.into()),
        )
}

pub fn severity_style(severity: Severity, colors: &Colors) -> (Hsla, IconName) {
    match severity {
        Severity::Error => (colors.red, IconName::CircleX),
        Severity::Warning => (colors.yellow, IconName::TriangleAlert),
        Severity::Info => (colors.text_dim, IconName::Info),
    }
}

/// The icon of a test step.
pub fn status_icon(status: Status, colors: &Colors) -> AnyElement {
    let (icon, color) = match status {
        Status::Ok => (Some(IconName::CircleCheck), colors.green),
        Status::Warn => (Some(IconName::TriangleAlert), colors.yellow),
        Status::Fail => (Some(IconName::CircleX), colors.red),
        Status::Running => (Some(IconName::RefreshCw), colors.accent),
        Status::Pending | Status::Skipped => (None, colors.text_faint),
    };
    match icon {
        Some(icon) => Icon::new(icon).size(15.0).color(color).into_any_element(),
        None => div()
            .size(u(15.0))
            .rounded_full()
            .border_1()
            .border_color(color)
            .into_any_element(),
    }
}

/// A key/value list (`Subject`, `Issuer`…).
pub fn kv(rows: Vec<(&'static str, AnyElement)>, colors: &Colors) -> impl IntoElement {
    v_flex()
        .gap(u(4.0))
        .children(rows.into_iter().map(|(k, v)| {
            h_flex()
                .items_start()
                .gap(u(8.0))
                .text_size(u(12.0))
                .child(
                    div()
                        .w(u(90.0))
                        .flex_none()
                        .text_color(colors.text_dim)
                        .child(k),
                )
                .child(div().flex_1().min_w_0().child(v))
        }))
}

pub fn mono(text: impl Into<SharedString>) -> AnyElement {
    div()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(text.into())
        .into_any_element()
}

pub fn text(text: impl Into<SharedString>) -> AnyElement {
    div().child(text.into()).into_any_element()
}

/// A clickable accent-colored label.
pub fn link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id.into())
        .text_size(u(12.0))
        .text_color(colors.accent)
        .cursor_pointer()
        .hover(|s| s.underline())
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx)
        })
        .child(label.into())
}
