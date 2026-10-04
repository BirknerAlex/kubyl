//! A single line in the ring buffer (from `kubyl_logs_core`), plus its theme color.

use gpui::Hsla;
pub use kubyl_logs_core::line::*;

/// Resolves a pod color index to an actual color from the theme's accent palette.
pub fn pod_color(index: u8, colors: &kubyl_ui::Colors) -> Hsla {
    let palette = [
        colors.cyan,
        colors.purple,
        colors.orange,
        colors.green,
        colors.accent,
        colors.yellow,
        colors.red,
        colors.text_muted,
    ];
    palette[index as usize % palette.len()]
}
