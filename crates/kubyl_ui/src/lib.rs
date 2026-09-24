//! Kubyl's UI kit: theme tokens, fonts, icons and Zed-like components.
//!
//! Sizes are written with [`u`] ("UI pixels"), which converts to rems. The window's rem size
//! follows the UI font size setting, so zooming scales the whole interface.
//!
//! Colors come from [`ActiveColors::colors`] (`cx.colors().accent`). They mirror the CSS variables
//! of the mockups (`design/mockups/generate.py`).

mod appearance;
mod assets;
pub mod components;
mod icon;
mod theme;

pub use appearance::{AppearanceSettings, ThemeChoice, apply_theme, apply_zoom, zoom};
pub use assets::{Assets, load_fonts};
pub use components::*;
pub use icon::{Icon, IconName};
pub use theme::{ActiveColors, Colors, Theme, fonts, sizes};

use gpui::{App, Rems, rems};

/// Pixels at the default UI size, converted to rems so they scale with zoom.
pub fn u(px: f32) -> Rems {
    rems(px / 16.0)
}

/// Loads fonts, registers settings, applies the theme and installs the component library.
///
/// Call after `kubyl_settings::init`.
pub fn init(cx: &mut App) {
    load_fonts(cx);
    gpui_component::init(cx);
    appearance::init(cx);
    components::init(cx);
}
