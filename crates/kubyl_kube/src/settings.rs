//! The `"kubernetes"` settings.json section (from `kubyl_kube_core`), plus theme colors for its
//! color tags.

use gpui::{App, Hsla};
pub use kubyl_kube_core::settings::*;
use kubyl_ui::Colors;

/// Theme colors of [`ColorTag`]s.
pub trait ColorTagExt {
    fn color(self, colors: &Colors) -> Hsla;
}

impl ColorTagExt for ColorTag {
    fn color(self, colors: &Colors) -> Hsla {
        match self {
            ColorTag::Red => colors.red,
            ColorTag::Orange => colors.orange,
            ColorTag::Yellow => colors.yellow,
            ColorTag::Green => colors.green,
            ColorTag::Cyan => colors.cyan,
            ColorTag::Blue => colors.accent,
            ColorTag::Purple => colors.purple,
            ColorTag::Gray => colors.text_faint,
        }
    }
}

/// The color of a context: its tag, or a stable default.
pub fn context_color(id: &str, settings: &ContextSettings, cx: &App) -> Hsla {
    use kubyl_ui::ActiveColors as _;
    context_color_tag(id, settings).color(cx.colors())
}
