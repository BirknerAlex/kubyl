//! Small pieces shared by the Alerts tabs, the dialogs and the chrome.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, SharedString, Window,
    div, prelude::*,
};
use jiff::Timestamp;
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{Colors, Icon, IconName, StatusDot, fonts, h_flex, sizes, u, v_flex};

use crate::model::{AlertState, Severity};

pub fn severity_color(severity: &Severity, colors: &Colors) -> Hsla {
    match severity {
        Severity::Critical => colors.red,
        Severity::Warning => colors.yellow,
        Severity::Info => colors.accent,
        _ => colors.text_dim,
    }
}

pub fn state_color(state: AlertState, colors: &Colors) -> Hsla {
    match state {
        AlertState::Firing | AlertState::Unprocessed => colors.red,
        AlertState::Pending => colors.yellow,
        AlertState::Resolved => colors.green,
        AlertState::Silenced | AlertState::Inhibited => colors.text_dim,
    }
}

/// `● critical`.
pub fn severity_pill(severity: &Severity, colors: &Colors) -> AnyElement {
    let color = severity_color(severity, colors);
    h_flex()
        .gap(u(6.0))
        .text_color(color)
        .child(StatusDot::new(color))
        .child(severity.label().to_string())
        .into_any_element()
}

/// `● firing`, `○ pending`, `silenced`.
pub fn state_label(state: AlertState, colors: &Colors) -> AnyElement {
    let color = state_color(state, colors);
    let dot = match state {
        AlertState::Pending => div()
            .flex_none()
            .size(u(7.0))
            .rounded_full()
            .border_1()
            .border_color(color)
            .into_any_element(),
        AlertState::Silenced => Icon::new(IconName::BellOff)
            .size(11.0)
            .color(color)
            .into_any_element(),
        _ => StatusDot::new(color).into_any_element(),
    };
    h_flex()
        .gap(u(6.0))
        .text_color(color)
        .child(dot)
        .child(state.label())
        .into_any_element()
}

/// `2h 14m`, `19m`, `14s` (a future time counts as "just now").
pub fn ago(time: Option<Timestamp>, now: Timestamp) -> String {
    let Some(time) = time else {
        return String::new();
    };
    let seconds = now.duration_since(time).as_secs().max(0);
    short_duration(seconds)
}

pub fn short_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        let m = minutes % 60;
        return if m == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {m}m")
        };
    }
    let days = hours / 24;
    let h = hours % 24;
    if h == 0 || days >= 7 {
        format!("{days}d")
    } else {
        format!("{days}d {h}h")
    }
}

/// `13:58 (11:58 UTC)`, with the date when it isn't today.
pub fn local_and_utc(time: Timestamp) -> String {
    let tz = jiff::tz::TimeZone::system();
    let local = time.to_zoned(tz.clone());
    let utc = time.to_zoned(jiff::tz::TimeZone::UTC);
    let today = Timestamp::now().to_zoned(tz).date();
    if local.date() == today {
        format!(
            "{} ({} UTC)",
            local.strftime("%H:%M"),
            utc.strftime("%H:%M")
        )
    } else {
        format!(
            "{} ({} UTC)",
            local.strftime("%Y-%m-%d %H:%M"),
            utc.strftime("%H:%M")
        )
    }
}

/// A Prometheus value, readable: `10.3`, `0.0213`, `1.2e+09`.
pub fn format_value(value: &str) -> String {
    let Ok(number) = value.parse::<f64>() else {
        return value.to_string();
    };
    if !number.is_finite() {
        return value.to_string();
    }
    let magnitude = number.abs();
    if magnitude != 0.0 && !(1e-4..1e9).contains(&magnitude) {
        return format!("{number:.3e}");
    }
    if number.fract() == 0.0 {
        return format!("{number:.0}");
    }
    let digits = if magnitude >= 100.0 {
        1
    } else if magnitude >= 1.0 {
        3
    } else {
        4
    };
    let text = format!("{number:.digits$}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn column_cell(def: &ColumnDef) -> gpui::Div {
    let cell = div().min_w_0().overflow_hidden().pr(u(8.0));
    match def.width {
        ColumnWidth::Fixed(width) => cell.flex_none().w(u(width)),
        ColumnWidth::Flex { weight, min } => {
            cell.flex_basis(u(0.0)).flex_grow(weight).min_w(u(min))
        }
    }
}

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

/// A details section: `TITLE` and its content.
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
                        .w(u(104.0))
                        .text_color(colors.text_dim)
                        .child(key),
                )
                .child(div().flex_1().min_w_0().child(value))
        }))
}

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

/// A muted `name=value` chip.
pub fn label_chip(name: &str, value: &str, colors: &Colors) -> gpui::Div {
    div()
        .flex_none()
        .px(u(6.0))
        .py(u(1.0))
        .rounded(u(4.0))
        .bg(colors.chip_background)
        .font_family(fonts::MONO)
        .text_size(u(11.0))
        .text_color(colors.text_muted)
        .max_w_full()
        .truncate()
        .child(format!("{name}={value}"))
}

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

/// A small text button (`Show`, `Clear`).
pub fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex_none()
        .text_color(colors.accent)
        .cursor_pointer()
        .hover(|s| s.underline())
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx)
        })
        .child(label.into())
}

/// Copies text and says so.
pub fn copy(text: String, what: &str, cx: &mut App) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    kubyl_core::NotificationCenter::push(
        cx,
        kubyl_core::Notification::info(format!("Copied {what}.")),
    );
}

/// Opens an `http(s)` URL in the system browser.
pub fn open_url(url: &str, cx: &mut App) {
    if !crate::model::is_web_url(url) {
        return;
    }
    if let Err(err) = open::that_detached(url) {
        kubyl_core::NotificationCenter::push(
            cx,
            kubyl_core::Notification::error(format!("Couldn't open {url}: {err}")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(format_value("1.0297191939677406e+01"), "10.297");
        assert_eq!(format_value("1"), "1");
        assert_eq!(format_value("0.02134"), "0.0213");
        assert_eq!(format_value("1234567890123"), "1.235e12");
        assert_eq!(format_value("NaN"), "NaN");
        assert_eq!(format_value("x"), "x");
    }

    #[test]
    fn short_durations() {
        assert_eq!(short_duration(14), "14s");
        assert_eq!(short_duration(19 * 60), "19m");
        assert_eq!(short_duration(2 * 3600 + 14 * 60), "2h 14m");
        assert_eq!(short_duration(3 * 86_400 + 3600), "3d 1h");
        assert_eq!(short_duration(-5), "0s", "a future start is just now");
    }
}
