//! Small pieces of the Network Flows view (board 18).

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, IntoElement, SharedString, Window,
    div, prelude::*,
};
use jiff::Timestamp;
use kubyl_core::{ClusterId, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{Colors, Icon, IconName, Selectable, fonts, h_flex, u, v_flex};

use crate::model::{PolicyRef, PolicySummary, Verdict};

/// The object behind a policy a backend named, when the cluster serves its kind.
pub fn policy_target(cluster: &ClusterId, policy: &PolicyRef, cx: &App) -> Option<ResourceRef> {
    let (groups, kind) = policy.api()?;
    let manager = ConnectionManager::try_global(cx)?;
    let discovery = manager.read(cx).cluster(cluster)?.discovery.as_ref()?;
    let info = groups.iter().find_map(|group| {
        discovery
            .preferred()
            .find(|r| r.gvk.group == *group && r.gvk.kind == kind)
    })?;
    let namespace = policy
        .namespace
        .as_ref()
        .filter(|_| info.namespaced)
        .map(|n| n.to_string());
    Some(ResourceRef::object(
        cluster.clone(),
        info.gvr.clone(),
        namespace,
        policy.name.to_string(),
    ))
}

pub fn verdict_color(verdict: Verdict, colors: &Colors) -> Hsla {
    match verdict {
        Verdict::Forwarded | Verdict::Redirected | Verdict::Translated => colors.green,
        Verdict::Dropped => colors.red,
        Verdict::NoReply => colors.yellow,
        Verdict::Error => colors.orange,
        Verdict::Audit => colors.purple,
        Verdict::Traced | Verdict::Unknown => colors.text_muted,
    }
}

/// `● dropped`.
pub fn verdict_pill(verdict: Verdict, colors: &Colors) -> AnyElement {
    let color = verdict_color(verdict, colors);
    h_flex()
        .gap(u(6.0))
        .text_color(color)
        .child(div().flex_none().size(u(7.0)).rounded_full().bg(color))
        .child(verdict.label())
        .into_any_element()
}

/// The Policy column: a shield for named denials, red for blocked flows.
pub fn policy_cell(summary: &PolicySummary, colors: &Colors) -> AnyElement {
    let text = summary.text();
    match summary {
        PolicySummary::DeniedBy(_) => h_flex()
            .min_w_0()
            .gap(u(5.0))
            .text_color(colors.red)
            .child(Icon::new(IconName::Shield).size(11.0).color(colors.red))
            .child(div().min_w_0().truncate().child(text))
            .into_any_element(),
        PolicySummary::Isolated(_) | PolicySummary::Reason(_) => div()
            .min_w_0()
            .truncate()
            .text_color(colors.red)
            .child(text)
            .into_any_element(),
        PolicySummary::AllowedBy(_) => div().min_w_0().truncate().child(text).into_any_element(),
        PolicySummary::None => div()
            .text_color(colors.text_faint)
            .child("—")
            .into_any_element(),
    }
}

/// `14:02:31.482` (local), with the date when it isn't today.
pub fn clock(time: Timestamp, today: jiff::civil::Date) -> String {
    let local = time.to_zoned(jiff::tz::TimeZone::system());
    if local.date() == today {
        local.strftime("%H:%M:%S%.3f").to_string()
    } else {
        local.strftime("%m-%d %H:%M:%S").to_string()
    }
}

/// `Denied by`, from `denied by`.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// An aggregate's interval: `14:02:15–30`, `14:02:45–03:00` (the end as far as it differs).
pub fn interval(start: Timestamp, end: Timestamp, today: jiff::civil::Date) -> String {
    let zone = jiff::tz::TimeZone::system();
    let (a, b) = (start.to_zoned(zone.clone()), end.to_zoned(zone));
    let head = if a.date() == today {
        a.strftime("%H:%M:%S").to_string()
    } else {
        a.strftime("%m-%d %H:%M:%S").to_string()
    };
    let tail = if a.date() != b.date() || a.hour() != b.hour() {
        b.strftime("%H:%M:%S")
    } else if a.minute() != b.minute() {
        b.strftime("%M:%S")
    } else {
        b.strftime("%S")
    };
    format!("{head}–{tail}")
}

/// `14:02:31.482 · 12:02:31 UTC`, with the date.
pub fn local_and_utc(time: Timestamp) -> String {
    let local = time.to_zoned(jiff::tz::TimeZone::system());
    let utc = time.to_zoned(jiff::tz::TimeZone::UTC);
    format!(
        "{} · {} UTC",
        local.strftime("%Y-%m-%d %H:%M:%S%.3f"),
        utc.strftime("%H:%M:%S")
    )
}

/// `1.2 KB`, `3 B`.
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit < UNITS.len() - 1 {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}

/// `12,418`.
pub fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 flow`, `3 flows`.
pub fn plural(value: usize, one: &str, many: &str) -> String {
    format!("{} {}", count(value), if value == 1 { one } else { many })
}

/// A details section: `TITLE` and its content.
pub fn section(title: impl Into<SharedString>, colors: &Colors) -> gpui::Stateful<gpui::Div> {
    let title: SharedString = title.into();
    // The id scopes the selectable text inside to this section.
    let id = SharedString::from(format!(
        "section-{}",
        title.split(" · ").next().unwrap_or_default()
    ));
    v_flex()
        .id(id)
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

/// Key/value rows with a fixed key column.
pub fn kv(rows: Vec<(SharedString, AnyElement)>, key_width: f32, colors: &Colors) -> AnyElement {
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
                        .w(u(key_width))
                        .text_color(colors.text_dim)
                        .child(Selectable::new(
                            SharedString::from(format!("{key}-key")),
                            key.clone(),
                        )),
                )
                .child(div().id(key).flex_1().min_w_0().child(value))
        }))
        .into_any_element()
}

/// Monospace text that can be selected and copied.
pub fn mono(text: impl Into<SharedString>, colors: &Colors) -> gpui::Div {
    let text = text.into();
    div()
        .min_w_0()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .text_color(colors.text)
        .child(Selectable::new(text.clone(), text))
}

/// A small bordered button (`This connection`).
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
        .h(u(24.0))
        .px(u(9.0))
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

/// A text link.
pub fn link(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    colors: &Colors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .min_w_0()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(count(12_418), "12,418");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000_000), "1,000,000");
        assert_eq!(plural(1, "flow", "flows"), "1 flow");
        assert_eq!(plural(2_000, "flow", "flows"), "2,000 flows");
        assert_eq!(bytes(592), "592 B");
        assert_eq!(bytes(31 * 1024), "31.0 KB");
    }
}
