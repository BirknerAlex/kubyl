use gpui::{App, Window, WindowAppearance, px};
use kubyl_settings::{Settings, SettingsSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::theme::{Theme, sizes};

/// Which theme to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    /// Follow the operating system.
    System,
}

/// Appearance settings (top level of settings.json).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AppearanceSettings {
    /// `dark`, `light` or `system`.
    pub theme: ThemeChoice,
    /// UI font size in pixels. Zooming (⌘+ / ⌘-) changes it. Default 13.
    pub ui_font_size: f32,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            ui_font_size: sizes::UI_FONT,
        }
    }
}

impl SettingsSection for AppearanceSettings {
    const KEY: Option<&'static str> = None;
}

const MIN_FONT: f32 = 9.0;
const MAX_FONT: f32 = 24.0;

pub(crate) fn init(cx: &mut App) {
    Settings::register::<AppearanceSettings>(cx);
    apply_theme(cx);
    Settings::observe::<AppearanceSettings>(cx, |_, cx| {
        apply_theme(cx);
        cx.refresh_windows();
    })
    .detach();
}

/// Applies the configured theme. Call again when the OS appearance changes.
pub fn apply_theme(cx: &mut App) {
    let dark = match Settings::get::<AppearanceSettings>(cx).theme {
        ThemeChoice::Dark => true,
        ThemeChoice::Light => false,
        ThemeChoice::System => matches!(
            cx.window_appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    let theme = if dark { Theme::dark() } else { Theme::light() };
    if cx.try_global::<Theme>() != Some(&theme) {
        theme.apply(cx);
    }
}

/// Sets the window's rem size from the UI font size. Call at the start of the root view's render.
pub fn apply_zoom(window: &mut Window, cx: &App) {
    let font = Settings::get::<AppearanceSettings>(cx).ui_font_size;
    window.set_rem_size(px(16.0 * font / sizes::UI_FONT));
}

/// Changes the UI font size by `delta` pixels, or resets it with `None`. Persisted in settings.json.
pub fn zoom(cx: &mut App, delta: Option<f32>) {
    Settings::update::<AppearanceSettings>(cx, |settings| {
        settings.ui_font_size = match delta {
            Some(delta) => (settings.ui_font_size + delta).clamp(MIN_FONT, MAX_FONT),
            None => sizes::UI_FONT,
        };
    });
    cx.refresh_windows();
}
