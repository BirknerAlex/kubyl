//! Small building blocks of the Updates tab: cards, key/value rows, table headers, status
//! icons, buttons.

use gpui::{AnyElement, App, FontWeight, Hsla, IntoElement, SharedString, Window, div, prelude::*};
use kubyl_ui::{Colors, Icon, IconName, Selectable, fonts, h_flex, u, v_flex};

use crate::check::CheckStatus;

/// A bordered card with a header line.
pub fn card(colors: &Colors) -> gpui::Div {
    v_flex()
        .flex_none()
        .rounded(u(8.0))
        .border_1()
        .border_color(colors.border_variant)
        .bg(colors.panel)
        .overflow_hidden()
}

/// A card's header: a title, a dim note, and elements at the end.
pub fn card_header(
    title: impl Into<SharedString>,
    note: Option<String>,
    end: Vec<AnyElement>,
    colors: &Colors,
) -> impl IntoElement {
    h_flex()
        .gap(u(8.0))
        .px(u(16.0))
        .py(u(10.0))
        .child(
            div()
                .flex_none()
                .font_weight(FontWeight::MEDIUM)
                .child(title.into()),
        )
        .when_some(note, |this, note| {
            this.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(note),
            )
        })
        .child(div().flex_1())
        .children(end)
}

/// A label on the left, a value on the right (version card, confirmation).
pub fn kv(
    label: impl Into<SharedString>,
    value: impl IntoElement,
    width: f32,
    colors: &Colors,
) -> impl IntoElement {
    let label = label.into();
    h_flex()
        .items_start()
        .gap(u(10.0))
        .text_size(u(12.5))
        .child(
            div()
                .flex_none()
                .w(u(width))
                .text_color(colors.text_dim)
                .child(Selectable::new(
                    SharedString::from(format!("{label}-key")),
                    label.clone(),
                )),
        )
        .child(div().id(label).flex_1().min_w_0().child(value))
}

/// Mono text.
pub fn mono(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_family(fonts::MONO)
        .text_size(u(12.0))
        .child(text.into())
}

/// Mono text for a kv value that can be selected and copied.
pub fn kv_mono(text: impl Into<SharedString>) -> gpui::Div {
    let text = text.into();
    div()
        .font_family(fonts::MONO)
        .text_size(u(12.0))
        .child(Selectable::new(text.clone(), text))
}

/// The icon and color of a check status.
pub fn status_icon(status: CheckStatus, colors: &Colors) -> (IconName, Hsla) {
    match status {
        CheckStatus::Fail => (IconName::CircleX, colors.red),
        CheckStatus::Warn => (IconName::TriangleAlert, colors.yellow),
        CheckStatus::Unknown => (IconName::Info, colors.text_dim),
        CheckStatus::Pass => (IconName::CircleCheck, colors.green),
        CheckStatus::Info => (IconName::Info, colors.text_dim),
    }
}

/// A table header row with fixed column widths (`None`: flexible).
pub fn table_header(columns: &[(&'static str, Option<f32>)], colors: &Colors) -> impl IntoElement {
    h_flex()
        .h(u(28.0))
        .px(u(16.0))
        .gap(u(10.0))
        .bg(colors.subheader_background)
        .text_size(u(11.0))
        .text_color(colors.text_dim)
        .children(columns.iter().map(|(title, width)| {
            let cell = div().child(title.to_uppercase());
            match width {
                Some(w) => cell.flex_none().w(u(*w)),
                None => cell.flex_1().min_w_0(),
            }
        }))
}

/// One table cell of a fixed width (`None`: flexible).
pub fn cell(width: Option<f32>, child: impl IntoElement) -> gpui::Div {
    let cell = div().min_w_0().overflow_hidden().child(child);
    match width {
        Some(w) => cell.flex_none().w(u(w)),
        None => cell.flex_1(),
    }
}

/// A table row.
pub fn table_row(colors: &Colors) -> gpui::Div {
    h_flex()
        .min_h(u(34.0))
        .px(u(16.0))
        .gap(u(10.0))
        .border_t_1()
        .border_color(colors.row_border)
        .text_size(u(12.5))
}

/// A thin progress bar.
pub fn bar(percent: f32, width: f32, color: Hsla, colors: &Colors) -> impl IntoElement {
    let percent = percent.clamp(0.0, 100.0);
    div()
        .flex_none()
        .w(u(width))
        .h(u(4.0))
        .rounded(u(2.0))
        .bg(colors.bar_track)
        .child(
            div()
                .h_full()
                .rounded(u(2.0))
                .bg(color)
                .w(gpui::relative(percent / 100.0)),
        )
}

/// A progress bar across its container.
pub fn wide_bar(percent: f32, color: Hsla, colors: &Colors) -> impl IntoElement {
    let percent = percent.clamp(0.0, 100.0);
    div()
        .w_full()
        .h(u(4.0))
        .rounded(u(2.0))
        .bg(colors.bar_track)
        .child(
            div()
                .h_full()
                .rounded(u(2.0))
                .bg(color)
                .w(gpui::relative(percent / 100.0)),
        )
}

/// A small outlined button for rows (`Open PDB`, `Update…`).
pub fn row_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let hover = colors.hover;
    h_flex()
        .id(id)
        .flex_none()
        .h(u(24.0))
        .px(u(9.0))
        .rounded(u(5.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.button_background)
        .text_size(u(12.0))
        .text_color(colors.text)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(label.into())
}

/// A link-styled text.
pub fn link(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .cursor_pointer()
        .text_color(colors.accent)
        .hover(|s| s.underline())
        .child(label.into())
}

/// An icon followed by text.
pub fn icon_text(icon: IconName, color: Hsla, text: impl Into<SharedString>) -> impl IntoElement {
    h_flex()
        .gap(u(6.0))
        .child(Icon::new(icon).size(12.0).color(color))
        .child(div().min_w_0().child(text.into()))
}

/// `2 min ago`.
pub fn ago(timestamp: jiff::Timestamp) -> String {
    let seconds = jiff::Timestamp::now()
        .duration_since(timestamp)
        .as_secs()
        .max(0);
    format!("{} ago", kubyl_resources::format::human_duration(seconds))
}

/// `1 h 12 min`.
pub fn took(started: jiff::Timestamp, completed: jiff::Timestamp) -> String {
    let minutes = completed.duration_since(started).as_secs().max(0) / 60;
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}

/// A date like `2026-08-30`.
pub fn date(timestamp: jiff::Timestamp) -> String {
    timestamp.strftime("%Y-%m-%d").to_string()
}

/// Opens a URL in the browser (http(s) only).
pub fn open_url(url: &str, _: &mut Window, cx: &mut App) {
    if url.starts_with("https://") || url.starts_with("http://") {
        cx.open_url(url);
    }
}
