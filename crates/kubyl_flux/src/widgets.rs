//! Small UI pieces of the Flux views (board 21): Flux states mapped to theme tokens, pills,
//! sections, key/value rows and table cells, in the style of the Argo CD views.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, SharedString, Window,
    div, prelude::*,
};
use kubyl_core::{Align, ColumnDef, ColumnWidth, Tone};
use kubyl_flux_core::kinds::{Category, FluxKind};
use kubyl_flux_core::model::State;
use kubyl_ui::{Colors, IconName, Selectable, StatusDot, fonts, h_flex, sizes, u, v_flex};

/// Flux states as theme tokens: Ready green, Reconciling blue, Failed red, Stalled orange,
/// Suspended purple, Unknown grey.
pub fn state_color(state: State, colors: &Colors) -> Hsla {
    match state {
        State::Ready => colors.green,
        State::Reconciling => colors.accent,
        State::Failed => colors.red,
        State::Stalled => colors.orange,
        State::Suspended => colors.purple,
        State::Unknown => colors.text_dim,
    }
}

/// Tones for generic tables.
pub fn state_tone(state: State) -> Tone {
    match state {
        State::Ready => Tone::Good,
        State::Reconciling => Tone::Info,
        State::Failed | State::Stalled => Tone::Bad,
        State::Suspended | State::Unknown => Tone::Muted,
    }
}

pub fn kind_icon(kind: FluxKind) -> IconName {
    category_icon(kind.category())
}

pub fn category_icon(category: Category) -> IconName {
    match category {
        Category::Kustomizations => IconName::Layers,
        Category::HelmReleases => IconName::Anchor,
        Category::Sources => IconName::GitBranch,
        Category::ImageAutomation => IconName::Box,
        Category::Notifications => IconName::Bell,
    }
}

/// A dot and a colored label.
pub fn pill(label: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    h_flex()
        .gap(u(6.0))
        .min_w_0()
        .child(StatusDot::new(color))
        .child(div().truncate().text_color(color).child(label.into()))
}

pub fn state_pill(state: State, colors: &Colors) -> impl IntoElement {
    pill(state.label(), state_color(state, colors))
}

/// A details section: an upper-case title and its content.
pub fn section(title: impl Into<SharedString>, colors: &Colors) -> gpui::Stateful<gpui::Div> {
    let title: SharedString = title.into();
    let id = SharedString::from(format!(
        "flux-section-{}",
        title.split(" · ").next().unwrap_or_default()
    ));
    v_flex()
        .id(id)
        .px(u(14.0))
        .py(u(12.0))
        .gap(u(8.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(title_line(title, colors))
}

/// A section's title (`DEPENDENCIES · 2`).
pub fn title_line(text: impl Into<SharedString>, colors: &Colors) -> impl IntoElement {
    div()
        .text_size(u(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_dim)
        .child(text.into().to_uppercase())
}

/// Key/value rows.
pub fn kv(rows: Vec<(&'static str, AnyElement)>, colors: &Colors) -> impl IntoElement {
    v_flex()
        .gap(u(5.0))
        .text_size(u(12.0))
        .children(rows.into_iter().map(|(key, value)| {
            h_flex()
                .gap(u(8.0))
                .items_start()
                .child(
                    div()
                        .flex_none()
                        .w(u(96.0))
                        .text_color(colors.text_dim)
                        .child(Selectable::new(
                            SharedString::from(format!("{key}-key")),
                            key,
                        )),
                )
                .child(div().id(key).flex_1().min_w_0().child(value))
        }))
}

pub fn kv_text(value: impl Into<SharedString>) -> AnyElement {
    let value = value.into();
    div()
        .truncate()
        .child(Selectable::new(value.clone(), value))
        .into_any_element()
}

pub fn kv_mono(value: impl Into<SharedString>) -> AnyElement {
    let value = value.into();
    div()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(Selectable::new(value.clone(), value))
        .into_any_element()
}

pub fn mono(value: impl Into<SharedString>) -> AnyElement {
    div()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(value.into())
        .into_any_element()
}

/// A link-colored clickable text.
pub fn link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .truncate()
        .text_color(colors.accent)
        .cursor_pointer()
        .hover(|s| s.underline())
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx)
        })
        .child(label.into())
}

/// A link that opens `url` in the browser.
pub fn url_link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    url: String,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    link(id, label, colors, move |_, _, cx| cx.open_url(&url))
}

/// A cell sized by its column.
pub fn column_cell(def: &ColumnDef) -> gpui::Div {
    let cell = div().min_w_0().overflow_hidden().pr(u(8.0));
    let cell = match def.width {
        ColumnWidth::Fixed(width) => cell.flex_none().w(u(width)),
        ColumnWidth::Flex { weight, min } => {
            cell.flex_basis(u(0.0)).flex_grow(weight).min_w(u(min))
        }
    };
    match def.align {
        Align::Start => cell,
        Align::End => cell.flex().justify_end().pr(u(18.0)),
    }
}

/// A table header.
pub fn header(columns: &[ColumnDef], colors: &Colors) -> impl IntoElement {
    h_flex()
        .flex_none()
        .h(u(sizes::TABLE_HEADER))
        .px(u(12.0))
        .bg(colors.subheader_background)
        .border_b_1()
        .border_color(colors.border_variant)
        .text_size(u(11.5))
        .text_color(colors.text_dim)
        .whitespace_nowrap()
        .overflow_hidden()
        .children(
            columns
                .iter()
                .map(|def| column_cell(def).child(def.title.to_uppercase())),
        )
}

/// A table row (selected: accent outline).
pub fn row(
    id: impl Into<ElementId>,
    selected: bool,
    height: f32,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let hover = colors.hover;
    h_flex()
        .id(id.into())
        .relative()
        .w_full()
        .h(u(height))
        .flex_none()
        .px(u(12.0))
        .border_b_1()
        .border_color(colors.row_border)
        .whitespace_nowrap()
        .overflow_hidden()
        .text_size(u(sizes::UI_FONT))
        .text_color(colors.text)
        .map(|this| {
            if selected {
                this.bg(colors.selection).child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(colors.accent),
                )
            } else {
                this.hover(move |s| s.bg(hover))
            }
        })
}

/// An empty-state message filling its container.
pub fn empty(message: impl Into<SharedString>, colors: &Colors) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .p(u(24.0))
        .text_color(colors.text_dim)
        .text_size(u(12.5))
        .child(message.into())
        .into_any_element()
}

/// `12m`, `3d` since an RFC 3339 time.
pub fn age(time: Option<&str>) -> String {
    let now = jiff::Timestamp::now();
    time.and_then(kubyl_resources::format::timestamp)
        .map(|t| {
            kubyl_resources::format::human_duration(kubyl_resources::format::seconds_since(t, now))
        })
        .unwrap_or_default()
}

/// A warning box (yellow).
pub fn warning_box(colors: &Colors) -> gpui::Div {
    h_flex()
        .items_start()
        .gap(u(10.0))
        .p(u(10.0))
        .rounded(u(7.0))
        .bg(colors.yellow.opacity(0.1))
        .border_1()
        .border_color(colors.yellow.opacity(0.35))
}

/// An error box (red), for Ready's message of a failing object.
pub fn error_box(colors: &Colors) -> gpui::Div {
    h_flex()
        .items_start()
        .gap(u(10.0))
        .p(u(10.0))
        .rounded(u(7.0))
        .bg(colors.red.opacity(0.1))
        .border_1()
        .border_color(colors.red.opacity(0.35))
}

/// A small badge with a count (`3`), used after chips.
pub fn count(n: usize, colors: &Colors) -> impl IntoElement {
    div()
        .font_family(fonts::MONO)
        .text_size(u(11.0))
        .text_color(colors.text_dim)
        .child(n.to_string())
}
