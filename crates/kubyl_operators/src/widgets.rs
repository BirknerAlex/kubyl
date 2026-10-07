//! Small pieces shared by the Operators, OperatorHub and Helm release views and the dialogs.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, Hsla, Image, IntoElement, ObjectFit,
    SharedString, Window, div, img, prelude::*,
};
use jiff::Timestamp;
use kubyl_core::{ColumnDef, ColumnWidth, Notification, NotificationCenter, Tone};
use kubyl_ui::{
    Colors, Icon, IconName, Selectable, StatusDot, fonts, h_flex, sizes, tone_color, u, v_flex,
};

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

/// A group row inside a list (`Waiting for approval · 2`).
pub fn group_row(id: impl Into<ElementId>, label: String, colors: &Colors) -> AnyElement {
    h_flex()
        .id(id.into())
        .w_full()
        .h(u(26.0))
        .px(u(12.0))
        .bg(colors.subheader_background)
        .border_b_1()
        .border_color(colors.row_border)
        .text_size(u(11.5))
        .text_color(colors.text_dim)
        .child(label)
        .into_any_element()
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

/// A details section without a title.
pub fn plain_section(colors: &Colors) -> gpui::Div {
    v_flex()
        .px(u(14.0))
        .py(u(12.0))
        .gap(u(8.0))
        .border_b_1()
        .border_color(colors.border_variant)
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
                        .child(Selectable::new(
                            SharedString::from(format!("{key}-key")),
                            key,
                        )),
                )
                .child(div().id(key).flex_1().min_w_0().child(value))
        }))
}

/// Text for a kv value.
pub fn text(value: impl Into<SharedString>) -> AnyElement {
    div().truncate().child(value.into()).into_any_element()
}

/// Monospace text for a kv value.
pub fn mono(value: impl Into<SharedString>) -> AnyElement {
    div()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(value.into())
        .into_any_element()
}

/// Text for a kv value that can be selected and copied.
pub fn kv_text(value: impl Into<SharedString>) -> AnyElement {
    let value = value.into();
    div()
        .truncate()
        .child(Selectable::new(value.clone(), value))
        .into_any_element()
}

/// Monospace text for a kv value that can be selected and copied.
pub fn kv_mono(value: impl Into<SharedString>) -> AnyElement {
    let value = value.into();
    div()
        .truncate()
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(Selectable::new(value.clone(), value))
        .into_any_element()
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

/// `● Succeeded` in a tone's color.
pub fn pill(label: impl Into<SharedString>, tone: Tone, colors: &Colors) -> AnyElement {
    let color = tone_color(tone, colors);
    h_flex()
        .gap(u(6.0))
        .min_w_0()
        .text_color(color)
        .child(StatusDot::new(color))
        .child(div().truncate().child(label.into()))
        .into_any_element()
}

/// A line with an icon: `✓ Kubernetes 1.30 meets minKubeVersion 1.25`.
pub fn note(
    icon: IconName,
    color: Hsla,
    text: impl Into<SharedString>,
    colors: &Colors,
) -> AnyElement {
    h_flex()
        .items_start()
        .gap(u(8.0))
        .py(u(2.0))
        .text_size(u(12.0))
        .child(
            div()
                .pt(u(1.0))
                .child(Icon::new(icon).size(13.0).color(color)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(colors.text_muted)
                .child(text.into()),
        )
        .into_any_element()
}

/// The palette letter tiles pick from, by name.
pub fn tile_color(name: &str, colors: &Colors) -> Hsla {
    let palette = [
        colors.accent,
        colors.green,
        colors.cyan,
        colors.orange,
        colors.purple,
        colors.yellow,
        colors.red,
    ];
    let hash = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    palette[hash as usize % palette.len()]
}

/// A package's tile: its icon when loaded, else two letters in a tinted square.
pub fn tile(
    initials: String,
    name: &str,
    icon: Option<Arc<Image>>,
    size: f32,
    colors: &Colors,
) -> AnyElement {
    if let Some(icon) = icon {
        return div()
            .flex_none()
            .size(u(size))
            .rounded(u(6.0))
            .bg(gpui::white().opacity(0.92))
            .p(u(3.0))
            .child(img(icon).size_full().object_fit(ObjectFit::Contain))
            .into_any_element();
    }
    let color = tile_color(name, colors);
    div()
        .flex_none()
        .size(u(size))
        .rounded(u(6.0))
        .bg(color.opacity(0.14))
        .border_1()
        .border_color(color.opacity(0.35))
        .flex()
        .items_center()
        .justify_center()
        .text_color(color)
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(u(if size < 30.0 { 11.0 } else { 13.0 }))
        .child(initials)
        .into_any_element()
}

/// Most decoded CSV icons kept.
const CSV_ICONS_MAX: usize = 256;

thread_local! {
    /// Decoded CSV icons by content hash (`None`: undecodable), so a frame never decodes again.
    static CSV_ICONS: std::cell::RefCell<std::collections::HashMap<u64, Option<Arc<Image>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// A tile from a CSV's inline icon (base64), decoded once per distinct icon.
pub fn csv_icon(icon: &Option<(String, String)>) -> Option<Arc<Image>> {
    use std::hash::{Hash as _, Hasher as _};
    let (mediatype, data) = icon.as_ref()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    mediatype.hash(&mut hasher);
    data.hash(&mut hasher);
    let key = hasher.finish();
    CSV_ICONS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(hit) = cache.get(&key) {
            return hit.clone();
        }
        let decoded = decode_csv_icon(mediatype, data);
        if cache.len() >= CSV_ICONS_MAX {
            cache.clear();
        }
        cache.insert(key, decoded.clone());
        decoded
    })
}

fn decode_csv_icon(mediatype: &str, data: &str) -> Option<Arc<Image>> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()?;
    let format = if mediatype.contains("svg") {
        gpui::ImageFormat::Svg
    } else if mediatype.contains("png") {
        gpui::ImageFormat::Png
    } else if mediatype.contains("jpeg") || mediatype.contains("jpg") {
        gpui::ImageFormat::Jpeg
    } else {
        return None;
    };
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// `2h 14m`, `19m`, `3d`.
pub fn ago(time: Option<Timestamp>) -> String {
    let Some(time) = time else {
        return String::new();
    };
    let seconds = Timestamp::now().duration_since(time).as_secs().max(0);
    kubyl_resources::format::human_duration(seconds)
}

/// Opens an `http(s)` URL in the system browser.
pub fn open_url(url: &str, cx: &mut App) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    if let Err(err) = open::that_detached(url) {
        NotificationCenter::push(
            cx,
            Notification::error(format!("Couldn't open {url}: {err}")),
        );
    }
}

/// A sub-tab of a view header.
pub fn sub_tab(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    badge: Option<(String, Option<Hsla>)>,
    active: bool,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    h_flex()
        .id(id.into())
        .h_full()
        .px(u(10.0))
        .gap(u(7.0))
        .cursor_pointer()
        .whitespace_nowrap()
        .text_size(u(13.0))
        .text_color(if active {
            colors.text
        } else {
            colors.text_muted
        })
        .border_b_2()
        .border_color(if active {
            colors.accent
        } else {
            gpui::transparent_black()
        })
        .child(Icon::new(icon).size(13.0).color(if active {
            colors.accent
        } else {
            colors.text_dim
        }))
        .child(label.into())
        .when_some(badge, |this, (text, color)| {
            this.child(
                div()
                    .px(u(6.0))
                    .rounded(u(4.0))
                    .bg(colors.chip_background)
                    .font_family(fonts::MONO)
                    .text_size(u(11.0))
                    .text_color(color.unwrap_or(colors.text_muted))
                    .child(text),
            )
        })
}

/// A chip that acts as a toggle.
pub fn toggle_chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .cursor_pointer()
        .child(kubyl_ui::Chip::new(label).selected(on))
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_icon_is_decoded_once() {
        let icon = Some(("image/svg+xml".to_string(), "PHN2Zy8+".to_string()));
        let first = csv_icon(&icon).expect("decodes");
        let second = csv_icon(&icon.clone()).expect("cached");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(csv_icon(&Some(("text/plain".into(), "PHN2Zy8+".into()))).is_none());
        assert!(csv_icon(&None).is_none());
    }
}
