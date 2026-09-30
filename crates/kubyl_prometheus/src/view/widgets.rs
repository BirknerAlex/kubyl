//! Small pieces shared by the tabs.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, FontWeight, Hsla, IntoElement, SharedString,
    Window, div, prelude::*,
};
use gpui_component::input::{Input, InputState};
use jiff::Timestamp;
use kubyl_core::{ColumnDef, ColumnWidth};
use kubyl_ui::{Colors, Icon, IconName, StatusDot, fonts, h_flex, sizes, u, v_flex};

use crate::model::Health;

pub fn health_color(health: Health, colors: &Colors) -> Hsla {
    match health {
        Health::Up => colors.green,
        Health::Down => colors.red,
        Health::Unknown => colors.text_dim,
    }
}

/// `● up`, `● down`.
pub fn health_label(health: Health, colors: &Colors) -> AnyElement {
    let color = health_color(health, colors);
    h_flex()
        .gap(u(6.0))
        .text_color(color)
        .child(StatusDot::new(color))
        .child(health.label())
        .into_any_element()
}

/// `14s`, `19m`, `2h 14m`, `3d 1h`.
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

/// How long ago, `—` when never.
pub fn ago(time: Option<Timestamp>, now: Timestamp) -> String {
    match time {
        Some(time) => format!("{} ago", short_duration(now.duration_since(time).as_secs())),
        None => "—".into(),
    }
}

/// A scrape or evaluation time: `12 ms`, `1.4 s`.
pub fn seconds(value: Option<f64>) -> String {
    match value {
        None => "—".into(),
        Some(s) if s < 1.0 => format!("{:.0} ms", (s * 1000.0).max(0.0)),
        Some(s) => format!("{s:.1} s"),
    }
}

/// `1,234,567`.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `2026-09-30 13:58` in the local time zone.
pub fn local_time(time: Timestamp) -> String {
    time.to_zoned(jiff::tz::TimeZone::system())
        .strftime("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// A query value, readable: `10.3`, `0.0213`, `1.2e+09`.
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
    format!("{number:.digits$}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
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

/// A panel section: `TITLE` and its content.
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
                        .w(u(116.0))
                        .text_color(colors.text_dim)
                        .child(key),
                )
                .child(div().id(key).flex_1().min_w_0().child(value))
        }))
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

/// Wrapping label chips.
pub fn chips<'a>(
    labels: impl IntoIterator<Item = (&'a String, &'a String)>,
    colors: &Colors,
) -> gpui::Div {
    h_flex()
        .flex_wrap()
        .gap(u(4.0))
        .children(labels.into_iter().map(|(k, v)| label_chip(k, v, colors)))
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

/// A monospace block (an expression, an error).
pub fn code(text: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    div()
        .w_full()
        .px(u(8.0))
        .py(u(6.0))
        .rounded(u(5.0))
        .bg(colors.chip_background)
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .text_color(colors.text)
        .child(text.into())
}

/// A small bordered button.
pub fn button(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    label: impl Into<SharedString>,
    primary: bool,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let (bg, fg, border) = if primary {
        (colors.accent, colors.on_accent, colors.accent)
    } else {
        (colors.button_background, colors.text, colors.border)
    };
    let hover = colors.hover;
    h_flex()
        .id(id.into())
        .flex_none()
        .h(u(sizes::CONTROL))
        .px(u(10.0))
        .gap(u(6.0))
        .rounded(u(5.0))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_size(u(12.0))
        .cursor_pointer()
        .when(!primary, |this| this.hover(move |s| s.bg(hover)))
        .when_some(icon, |this, icon| {
            this.child(Icon::new(icon).size(12.0).color(fg))
        })
        .child(label.into())
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx)
        })
}

/// A filter box around an input.
pub fn filter_box(
    input: &Entity<InputState>,
    width: f32,
    focused: bool,
    colors: &Colors,
) -> gpui::Div {
    div()
        .flex_none()
        .w(u(width))
        .h(u(sizes::CONTROL))
        .px(u(8.0))
        .flex()
        .items_center()
        .gap(u(7.0))
        .rounded(u(5.0))
        .bg(colors.input_background)
        .border_1()
        .border_color(if focused {
            colors.accent
        } else {
            colors.border
        })
        .child(Icon::new(IconName::Funnel).size(12.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(input).appearance(false).text_size(u(12.0))),
        )
}

/// A toolbar row above a tab's list.
pub fn toolbar(colors: &Colors) -> gpui::Div {
    h_flex()
        .flex_none()
        .h(u(40.0))
        .px(u(12.0))
        .gap(u(10.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .text_size(u(12.0))
        .text_color(colors.text_muted)
}

/// Copies text and says so.
pub fn copy(text: String, what: &str, cx: &mut App) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    kubyl_core::NotificationCenter::push(
        cx,
        kubyl_core::Notification::info(format!("Copied {what}.")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(count(12), "12");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(12), "12 B");
        assert_eq!(seconds(Some(0.012)), "12 ms");
        assert_eq!(seconds(Some(1.44)), "1.4 s");
        assert_eq!(short_duration(2 * 3600 + 14 * 60), "2h 14m");
    }

    #[test]
    fn values() {
        assert_eq!(format_value("1.0297191939677406e+01"), "10.297");
        assert_eq!(format_value("1"), "1");
        assert_eq!(format_value("NaN"), "NaN");
        assert_eq!(format_value("1234567890123"), "1.235e12");
    }
}
