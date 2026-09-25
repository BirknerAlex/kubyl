//! Small UI pieces shared by the Argo CD views (boards 12–15): Argo CD's status colors mapped
//! to theme tokens, pills and chips, switches and checkboxes, sections and table cells.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, SharedString, Window,
    div, prelude::*,
};
use kubyl_core::{Align, ColumnDef, ColumnWidth, Tone};
use kubyl_ui::{
    ActiveColors, Chip, Colors, Icon, IconName, StatusDot, fonts, h_flex, sizes, u, v_flex,
};

use crate::model::{Activity, Health, OperationPhase, SyncStatus};
use crate::state::ApiState;

/// Argo CD's sync colors as theme tokens: Synced green, OutOfSync yellow, Unknown grey.
pub fn sync_color(status: SyncStatus, colors: &Colors) -> Hsla {
    match status {
        SyncStatus::Synced => colors.green,
        SyncStatus::OutOfSync => colors.yellow,
        SyncStatus::Unknown => colors.text_dim,
    }
}

/// Argo CD's health colors as theme tokens.
pub fn health_color(health: Health, colors: &Colors) -> Hsla {
    match health {
        Health::Healthy => colors.green,
        Health::Progressing => colors.accent,
        Health::Degraded => colors.red,
        Health::Suspended => colors.purple,
        Health::Missing => colors.yellow,
        Health::Unknown => colors.text_dim,
    }
}

/// Tones for generic tables (the explorer's Applications table).
pub fn sync_tone(status: SyncStatus) -> Tone {
    match status {
        SyncStatus::Synced => Tone::Good,
        SyncStatus::OutOfSync => Tone::Warning,
        SyncStatus::Unknown => Tone::Muted,
    }
}

pub fn health_tone(health: Health) -> Tone {
    match health {
        Health::Healthy => Tone::Good,
        Health::Progressing => Tone::Info,
        Health::Degraded => Tone::Bad,
        Health::Missing => Tone::Warning,
        Health::Suspended | Health::Unknown => Tone::Muted,
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

pub fn sync_pill(status: SyncStatus, colors: &Colors) -> impl IntoElement {
    pill(status.label(), sync_color(status, colors))
}

pub fn health_pill(health: Health, colors: &Colors) -> impl IntoElement {
    pill(health.label(), health_color(health, colors))
}

/// The running operation as a chip (`Syncing 12/40`).
pub fn activity_chip(activity: &Activity, colors: &Colors) -> impl IntoElement {
    h_flex()
        .flex_none()
        .h(u(19.0))
        .px(u(6.0))
        .gap(u(5.0))
        .rounded(u(4.0))
        .bg(colors.chip_selected_background)
        .text_color(colors.chip_selected_text)
        .text_size(u(11.5))
        .whitespace_nowrap()
        .child(
            Icon::new(IconName::RefreshCw)
                .size(10.0)
                .color(colors.accent),
        )
        .child(activity.label())
}

/// The last operation's result as an icon.
pub fn result_icon(phase: OperationPhase, colors: &Colors) -> Icon {
    match phase {
        OperationPhase::Succeeded => Icon::new(IconName::CircleCheck).color(colors.green),
        OperationPhase::Failed | OperationPhase::Error => {
            Icon::new(IconName::CircleX).color(colors.red)
        }
        _ => Icon::new(IconName::RefreshCw).color(colors.accent),
    }
}

/// "Kubernetes mode" or "API · alice", for toolbars and headers.
pub fn mode_chip(state: &ApiState) -> Chip {
    match state {
        ApiState::Connected { user, .. } => Chip::new(format!("API · {user}"))
            .icon(IconName::Link)
            .selected(true),
        ApiState::Connecting => Chip::new("API · connecting…").icon(IconName::Link),
        ApiState::SignInRequired(_) => Chip::new("API · sign in").icon(IconName::Key),
        ApiState::Failed(_) => Chip::new("Kubernetes mode · API failed").icon(IconName::ShipWheel),
        ApiState::Off => Chip::new("Kubernetes mode").icon(IconName::ShipWheel),
    }
}

/// A details section: an upper-case title and its content.
pub fn section(title: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    let title: SharedString = title.into();
    v_flex()
        .px(u(14.0))
        .py(u(12.0))
        .gap(u(8.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(
            div()
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .child(title.to_uppercase()),
        )
}

/// Key/value rows (`.kv`).
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
                        .w(u(84.0))
                        .text_color(colors.text_dim)
                        .child(key),
                )
                .child(div().flex_1().min_w_0().child(value))
        }))
}

/// Text for a value cell.
pub fn text(value: impl Into<SharedString>) -> AnyElement {
    div().truncate().child(value.into()).into_any_element()
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
) -> impl IntoElement {
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
) -> impl IntoElement {
    link(id, label, colors, move |_, _, cx| cx.open_url(&url))
}

/// A switch (`auto-sync`, `prune`, `self-heal`).
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
        .w(u(26.0))
        .h(u(15.0))
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
                .size(u(11.0))
                .rounded_full()
                .bg(gpui::white()),
        )
}

/// A checkbox with a label and an optional explanation.
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: bool,
    label: impl Into<SharedString>,
    detail: Option<SharedString>,
    enabled: bool,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    h_flex()
        .id(id.into())
        .items_start()
        .gap(u(8.0))
        .when(!enabled, |this| this.opacity(0.5))
        .when(enabled, |this| {
            this.cursor_pointer().on_click(move |event, window, cx| {
                cx.stop_propagation();
                on_click(event, window, cx)
            })
        })
        .child(check_box(checked, colors))
        .when(!label.is_empty(), |this| {
            this.child(
                v_flex()
                    .min_w_0()
                    .child(div().text_size(u(12.5)).child(label))
                    .when_some(detail, |this, detail| {
                        this.child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(detail),
                        )
                    }),
            )
        })
}

/// Just the box.
pub fn check_box(checked: bool, colors: &Colors) -> impl IntoElement {
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
                    Icon::new(IconName::CircleCheck)
                        .size(10.0)
                        .color(colors.on_accent),
                )
            } else {
                this.border_1().border_color(colors.text_faint)
            }
        })
}

/// A cell sized by its column (like the explorer's tables).
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

/// A table header for `columns`.
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

/// A table row (selected: accent outline, like the explorer).
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

/// `12m` since a timestamp.
pub fn age_of(time: Option<jiff::Timestamp>) -> String {
    let now = jiff::Timestamp::now();
    time.map(|t| {
        kubyl_resources::format::human_duration(kubyl_resources::format::seconds_since(t, now))
    })
    .unwrap_or_default()
}

/// The theme's background for a warning box.
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

/// Sort helper: a column id and direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: SharedString,
    pub ascending: bool,
}

/// A section's title row with a count (`OUT OF SYNC · 2 OF 6 RESOURCES`).
pub fn title_line(text: impl Into<SharedString>, colors: &Colors) -> impl IntoElement {
    div()
        .text_size(u(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_dim)
        .child(text.into().to_uppercase())
}

/// Shortens a long string in the middle (`kube-prome…-operator`).
pub fn middle_ellipsis(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max || max < 5 {
        return text.to_string();
    }
    let keep = max - 1;
    let head = keep / 2 + keep % 2;
    let tail = keep / 2;
    format!(
        "{}…{}",
        chars[..head].iter().collect::<String>(),
        chars[chars.len() - tail..].iter().collect::<String>()
    )
}

/// Colors for the current theme.
pub fn colors(cx: &App) -> Colors {
    cx.colors().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ellipsis() {
        assert_eq!(middle_ellipsis("short", 10), "short");
        assert_eq!(
            middle_ellipsis("kube-prometheus-stack-operator", 12),
            "kube-p…rator"
        );
    }
}
