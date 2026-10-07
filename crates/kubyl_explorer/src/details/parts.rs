//! Building blocks of the resource-view sections (phase 24): labelled rows, cards, CEL blocks,
//! state labels and links, with selectable text whose ids come from the call site.

use gpui::{ElementId, IntoElement, SharedString, div, prelude::*};
use kubyl_core::{ResourceRef, Tone};
use kubyl_ui::{Colors, Selectable, StatusDot, fonts, h_flex, tone_color, u, v_flex};

use super::link;

/// The id of selectable text: the call site, plus the row index for calls in a loop.
fn site_id(site: &std::panic::Location<'_>, ix: Option<usize>) -> ElementId {
    let ix = ix.map(|ix| format!(":{ix}")).unwrap_or_default();
    ElementId::Name(SharedString::from(format!(
        "v{}:{}{ix}",
        site.line(),
        site.column()
    )))
}

/// Selectable text, identified by its call site.
#[track_caller]
pub(super) fn text(value: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    let id = site_id(std::panic::Location::caller(), None);
    div()
        .min_w_0()
        .truncate()
        .text_color(color)
        .child(Selectable::new(id, value))
}

/// [`text`] in a loop: the row index tells the rows apart.
#[track_caller]
pub(super) fn text_at(ix: usize, value: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    let id = site_id(std::panic::Location::caller(), Some(ix));
    div()
        .min_w_0()
        .truncate()
        .text_color(color)
        .child(Selectable::new(id, value))
}

/// Monospace [`text_at`].
#[track_caller]
pub(super) fn mono_at(ix: usize, value: impl Into<SharedString>, color: gpui::Hsla) -> gpui::Div {
    let id = site_id(std::panic::Location::caller(), Some(ix));
    div()
        .min_w_0()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .text_color(color)
        .child(Selectable::new(id, value))
}

/// A label and a value element, aligned like the details' key/value rows.
pub(super) fn kv_row(
    label: impl Into<SharedString>,
    width: f32,
    value: impl IntoElement,
    colors: &Colors,
) -> impl IntoElement {
    let label: SharedString = label.into();
    h_flex()
        .items_start()
        .gap(u(8.0))
        .min_h(u(18.0))
        .text_size(u(12.0))
        .child(
            div()
                .flex_none()
                .w(u(width))
                .text_color(colors.text_dim)
                .child(Selectable::new(
                    ElementId::Name(format!("kv-{label}").into()),
                    label,
                )),
        )
        .child(div().flex_1().min_w_0().child(value))
}

/// A bordered box for one item (a request, a device, a listener).
pub(super) fn card(colors: &Colors) -> gpui::Div {
    v_flex()
        .gap(u(3.0))
        .px(u(10.0))
        .py(u(7.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border_variant)
        .bg(colors.subheader_background)
        .text_size(u(12.0))
}

/// A CEL expression in a monospace block, as written (no highlighting yet).
pub(super) fn cel(id: impl Into<ElementId>, expression: &str, colors: &Colors) -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .px(u(8.0))
        .py(u(5.0))
        .rounded(u(5.0))
        .border_1()
        .border_color(colors.border_variant)
        .bg(colors.input_background)
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .line_height(u(17.0))
        .text_color(colors.text)
        .child(Selectable::new(id, expression.to_string()))
}

/// A state with its tone's dot and color.
pub(super) fn state(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    colors: &Colors,
) -> impl IntoElement {
    let color = tone_color(tone, colors);
    h_flex()
        .flex_none()
        .gap(u(5.0))
        .child(StatusDot::new(color))
        .child(div().text_color(color).child(Selectable::new(id, label)))
}

/// A monospace link to an object.
pub(super) fn mono_link(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    target: ResourceRef,
    colors: &Colors,
) -> impl IntoElement {
    div()
        .min_w_0()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(link(id, label, target, colors))
}

/// A dimmed line for "nothing here".
#[track_caller]
pub(super) fn empty(message: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    text(message, colors.text_dim).text_size(u(12.0))
}

/// A section's list.
pub(super) fn list() -> gpui::Div {
    v_flex().gap(u(6.0)).text_size(u(12.0))
}

/// The small title above a sub-list inside a section ("Devices · 2").
#[track_caller]
pub(super) fn subtitle(value: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    text(value, colors.text_dim).text_size(u(11.5)).mt(u(4.0))
}

/// Text that wraps instead of being cut with an ellipsis ([`text`] truncates by default).
pub(super) trait Wrapped: Styled + Sized {
    fn wrapped(mut self) -> Self {
        self.text_style().text_overflow = None;
        self.whitespace_normal()
    }
}

impl<T: Styled> Wrapped for T {}
