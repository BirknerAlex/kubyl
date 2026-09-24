use gpui::{App, Global, Hsla, px, rgb, rgba};

/// Font families. Both are bundled (see `assets/fonts`).
pub mod fonts {
    pub const UI: &str = "IBM Plex Sans";
    pub const MONO: &str = "IBM Plex Mono";
}

/// Fixed chrome sizes from the mockups, in unscaled pixels (wrap with [`crate::u`]).
pub mod sizes {
    pub const TITLE_BAR: f32 = 38.0;
    pub const TAB_BAR: f32 = 34.0;
    pub const STATUS_BAR: f32 = 28.0;
    pub const KEY_HINTS: f32 = 28.0;
    pub const PANEL_HEADER: f32 = 34.0;
    pub const SECTION_HEADER: f32 = 24.0;
    pub const TREE_ROW: f32 = 23.0;
    pub const TABLE_HEADER: f32 = 28.0;
    pub const TABLE_ROW: f32 = 30.0;
    pub const TOOLBAR: f32 = 40.0;
    pub const CONTROL: f32 = 26.0;
    pub const SIDEBAR_WIDTH: f32 = 268.0;
    pub const RIGHT_DOCK_WIDTH: f32 = 340.0;
    pub const BOTTOM_DOCK_HEIGHT: f32 = 260.0;
    /// Default UI font size; zoom is relative to it.
    pub const UI_FONT: f32 = 13.0;
    pub const SMALL_FONT: f32 = 12.0;
    pub const LABEL_FONT: f32 = 11.0;
}

/// Color tokens. Names follow the mockup CSS variables.
#[derive(Clone, Debug, PartialEq)]
pub struct Colors {
    /// Editor/content background (`--bg`).
    pub background: Hsla,
    /// Sidebar, tab bar, status bar, docks (`--panel`).
    pub panel: Hsla,
    /// Title bar (`--elev`).
    pub elevated: Hsla,
    pub border: Hsla,
    /// Subtle separators inside panels (`--bv`).
    pub border_variant: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_dim: Hsla,
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub green: Hsla,
    pub red: Hsla,
    pub yellow: Hsla,
    pub purple: Hsla,
    pub cyan: Hsla,
    pub orange: Hsla,
    /// Selected row background (`--sel`).
    pub selection: Hsla,
    /// Hovered row/button background (`--hover`).
    pub hover: Hsla,
    pub title_bar_hover: Hsla,
    pub input_background: Hsla,
    pub button_background: Hsla,
    pub kbd_background: Hsla,
    pub chip_background: Hsla,
    pub chip_selected_background: Hsla,
    pub chip_selected_text: Hsla,
    pub chip_selected_border: Hsla,
    /// Key-hint bar and table header background.
    pub subheader_background: Hsla,
    pub row_border: Hsla,
    pub bar_track: Hsla,
    pub error_row_background: Hsla,
    pub avatar_background: Hsla,
    pub avatar_text: Hsla,
    /// Text on accent/red fills (primary buttons, PROD badge).
    pub on_accent: Hsla,
    pub window_border: Hsla,
    pub modal_backdrop: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

fn ca(hex_with_alpha: u32) -> Hsla {
    rgba(hex_with_alpha).into()
}

impl Colors {
    /// One Dark, as in the mockups.
    pub fn one_dark() -> Self {
        Self {
            background: c(0x282c33),
            panel: c(0x2f343e),
            elevated: c(0x3b414d),
            border: c(0x464b57),
            border_variant: c(0x363c46),
            text: c(0xdce0e5),
            text_muted: c(0xa9afbc),
            text_dim: c(0x959aa6),
            text_faint: c(0x5d636f),
            accent: c(0x74ade8),
            green: c(0xa1c181),
            red: c(0xd07277),
            yellow: c(0xdec184),
            purple: c(0xb477cf),
            cyan: c(0x6eb4bf),
            orange: c(0xbf956a),
            selection: c(0x363c48),
            hover: c(0x343944),
            title_bar_hover: c(0x454b58),
            input_background: c(0x2b3038),
            button_background: c(0x343944),
            kbd_background: c(0x343944),
            chip_background: c(0x353a45),
            chip_selected_background: c(0x2d3b4d),
            chip_selected_text: c(0xa8cdf3),
            chip_selected_border: c(0x3f5a78),
            subheader_background: c(0x2a2e36),
            row_border: c(0x2e333b),
            bar_track: c(0x3a3f4a),
            error_row_background: c(0x3a2e31),
            avatar_background: c(0x4a6a8a),
            avatar_text: c(0xe8f0f8),
            on_accent: c(0x1b1e24),
            window_border: c(0x4b5160),
            modal_backdrop: ca(0x0f11148c),
        }
    }

    /// One Light. A stub until a light mockup exists.
    pub fn one_light() -> Self {
        Self {
            background: c(0xfafafa),
            panel: c(0xf0f0f1),
            elevated: c(0xe5e5e6),
            border: c(0xc9c9ca),
            border_variant: c(0xdcdcdd),
            text: c(0x242529),
            text_muted: c(0x58585a),
            text_dim: c(0x6f6f73),
            text_faint: c(0xa1a1a3),
            accent: c(0x3b72d8),
            green: c(0x669f59),
            red: c(0xd36151),
            yellow: c(0xb8860b),
            purple: c(0x9d52c0),
            cyan: c(0x3a8ea0),
            orange: c(0xb5762f),
            selection: c(0xdce7fa),
            hover: c(0xe8e8e9),
            title_bar_hover: c(0xd7d7d9),
            input_background: c(0xffffff),
            button_background: c(0xffffff),
            kbd_background: c(0xf3f3f4),
            chip_background: c(0xe6e6e8),
            chip_selected_background: c(0xdce7fa),
            chip_selected_text: c(0x2a5cb8),
            chip_selected_border: c(0xa9c2ee),
            subheader_background: c(0xf3f3f4),
            row_border: c(0xe6e6e8),
            bar_track: c(0xdcdcdd),
            error_row_background: c(0xfbe7e5),
            avatar_background: c(0x6f8fb0),
            avatar_text: c(0xffffff),
            on_accent: c(0xffffff),
            window_border: c(0xc0c0c2),
            modal_backdrop: ca(0x0000004d),
        }
    }
}

/// The active Kubyl theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub colors: Colors,
}

impl Global for Theme {}

impl Theme {
    pub fn dark() -> Self {
        Self {
            dark: true,
            colors: Colors::one_dark(),
        }
    }

    pub fn light() -> Self {
        Self {
            dark: false,
            colors: Colors::one_light(),
        }
    }

    /// Makes this the active theme and restyles gpui-component to match.
    pub fn apply(self, cx: &mut App) {
        let mode = if self.dark {
            gpui_component::ThemeMode::Dark
        } else {
            gpui_component::ThemeMode::Light
        };
        gpui_component::Theme::change(mode, None, cx);
        style_components(&self.colors, cx);
        cx.set_global(self);
        cx.refresh_windows();
    }
}

/// `cx.colors()` for any context that derefs to [`App`].
pub trait ActiveColors {
    fn colors(&self) -> &Colors;
}

impl ActiveColors for App {
    fn colors(&self) -> &Colors {
        &self.global::<Theme>().colors
    }
}

/// Maps our tokens onto gpui-component's theme so its inputs, menus, dialogs and toasts match.
fn style_components(colors: &Colors, cx: &mut App) {
    let theme = gpui_component::Theme::global_mut(cx);
    theme.font_family = fonts::UI.into();
    theme.mono_font_family = fonts::MONO.into();
    theme.font_size = px(sizes::UI_FONT);
    theme.mono_font_size = px(sizes::SMALL_FONT);
    theme.radius = px(5.0);
    theme.radius_lg = px(8.0);
    theme.shadow = true;
    theme.focus_ring = false;

    if let Some(editor) = editor_theme(colors, theme.mode.is_dark()) {
        theme.highlight_theme = std::sync::Arc::new(editor);
    }

    let t = &mut theme.colors;
    let transparent = Hsla::transparent_black();
    t.background = colors.background;
    t.foreground = colors.text;
    t.border = colors.border;
    t.input = colors.border;
    t.ring = colors.accent;
    t.caret = colors.accent;
    t.selection = colors.accent.opacity(0.3);
    t.muted = colors.chip_background;
    t.muted_foreground = colors.text_dim;
    t.accent = colors.hover;
    t.accent_foreground = colors.text;
    t.primary = colors.accent;
    t.primary_hover = colors.accent.opacity(0.9);
    t.primary_active = colors.accent.opacity(0.8);
    t.primary_foreground = colors.on_accent;
    t.secondary = colors.button_background;
    t.secondary_hover = colors.hover;
    t.secondary_active = colors.selection;
    t.secondary_foreground = colors.text;
    t.danger = colors.red;
    t.danger_foreground = colors.on_accent;
    t.success = colors.green;
    t.warning = colors.yellow;
    t.info = colors.accent;
    t.link = colors.accent;
    t.link_hover = colors.accent.opacity(0.85);
    t.popover = colors.panel;
    t.popover_foreground = colors.text;
    t.list = colors.panel;
    t.list_hover = colors.hover;
    t.list_active = colors.selection;
    t.list_active_border = colors.accent;
    t.table = colors.background;
    t.table_head = colors.subheader_background;
    t.table_head_foreground = colors.text_dim;
    t.table_hover = colors.hover;
    t.table_active = colors.selection;
    t.table_active_border = colors.accent;
    t.table_row_border = colors.row_border;
    t.tab_bar = colors.panel;
    t.tab = transparent;
    t.tab_active = colors.background;
    t.tab_foreground = colors.text_dim;
    t.tab_active_foreground = colors.text;
    t.title_bar = colors.elevated;
    t.title_bar_border = colors.border;
    t.status_bar = colors.panel;
    t.status_bar_border = colors.border;
    t.sidebar = colors.panel;
    t.sidebar_border = colors.border;
    t.sidebar_foreground = colors.text_muted;
    t.sidebar_accent = colors.selection;
    t.sidebar_accent_foreground = colors.text;
    t.scrollbar = transparent;
    t.scrollbar_thumb = colors.border.opacity(0.8);
    t.scrollbar_thumb_hover = colors.border;
    t.overlay = colors.modal_backdrop;
    t.window_border = colors.window_border;
    t.progress_bar = colors.accent;
    t.drag_border = colors.accent;
    t.drop_target = colors.accent.opacity(0.15);
    t.red = colors.red;
    t.green = colors.green;
    t.blue = colors.accent;
    t.yellow = colors.yellow;
    t.magenta = colors.purple;
    t.cyan = colors.cyan;
    gpui_component::Theme::sync_base(cx);
}

/// Converts an `Hsla` to a CSS-style `#rrggbb` string.
#[cfg(test)]
pub fn to_hex(color: Hsla) -> String {
    let gpui::Rgba { r, g, b, .. } = color.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}",
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8
    )
}

/// Code editor colors (YAML editor, board 3): One Dark syntax, gutter and active line on our
/// tokens. Built through serde because gpui-component only exposes the Zed theme format.
fn editor_theme(
    colors: &Colors,
    dark: bool,
) -> Option<gpui_component::highlighter::HighlightTheme> {
    let c = |color: Hsla| serde_json::to_value(color).ok();
    let style = |color: Hsla| serde_json::json!({ "color": c(color) });
    let comment = serde_json::json!({ "color": c(colors.text_dim), "font_style": "italic" });
    let value = serde_json::json!({
        "name": "Kubyl",
        "appearance": if dark { "dark" } else { "light" },
        "style": {
            "editor.background": c(colors.background),
            "editor.foreground": c(colors.text),
            "editor.gutter.background": c(colors.background),
            "editor.active_line.background": c(colors.panel),
            "editor.line_number": c(colors.text_faint),
            "editor.active_line_number": c(colors.text),
            "editor.invisible": c(colors.text_faint),
            "syntax": {
                "property": style(colors.red),
                "attribute": style(colors.red),
                "string": style(colors.green),
                "string.special": style(colors.green),
                "number": style(colors.orange),
                "boolean": style(colors.orange),
                "constant": style(colors.orange),
                "constant.builtin": style(colors.orange),
                "comment": comment,
                "punctuation": style(colors.text_muted),
                "punctuation.delimiter": style(colors.text_muted),
                "punctuation.special": style(colors.text_dim),
                "punctuation.bracket": style(colors.text_muted),
                "type": style(colors.cyan),
                "label": style(colors.purple),
                "keyword": style(colors.purple),
            }
        }
    });
    serde_json::from_value(value)
        .inspect_err(|err| tracing::warn!("editor theme: {err}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_tokens_match_the_mockup() {
        let colors = Colors::one_dark();
        assert_eq!(to_hex(colors.background), "#282c33");
        assert_eq!(to_hex(colors.panel), "#2f343e");
        assert_eq!(to_hex(colors.elevated), "#3b414d");
        assert_eq!(to_hex(colors.border), "#464b57");
        assert_eq!(to_hex(colors.text), "#dce0e5");
        assert_eq!(to_hex(colors.text_muted), "#a9afbc");
        assert_eq!(to_hex(colors.accent), "#74ade8");
        assert_eq!(to_hex(colors.green), "#a1c181");
        assert_eq!(to_hex(colors.red), "#d07277");
        assert_eq!(to_hex(colors.yellow), "#dec184");
        assert_eq!(to_hex(colors.purple), "#b477cf");
        assert_eq!(to_hex(colors.cyan), "#6eb4bf");
        assert_eq!(to_hex(colors.orange), "#bf956a");
    }
}
