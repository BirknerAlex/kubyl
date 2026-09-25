//! Series colors.
//!
//! The hues are the theme's accent, orange, purple and cyan tokens, in that fixed order (the
//! mockup's order). Their lightness and chroma are re-stepped so adjacent series also differ in
//! lightness and stay apart under color-vision deficiencies. Both steps were checked with the
//! dataviz palette validator against the card surface (`panel`):
//!
//! - One Dark (`#2f343e`): OKLCH L 0.72 / 0.62 / 0.69 / 0.78, chroma ≥ 0.10, every color ≥ 3:1,
//!   worst adjacent CVD ΔE 11.6, normal-vision ΔE 23.8. The validator's dark lightness band
//!   (0.48–0.67) assumes a near-black surface; on this lighter panel it was shifted up by 0.1 so
//!   the marks keep 3:1 contrast.
//! - One Light (`#f0f0f1`): L 0.55 / 0.62 / 0.50 / 0.68, worst adjacent CVD ΔE 19.0. Cyan is
//!   2.4:1, so charts always show a legend and values in the tooltip.
//!
//! A fifth series is never a new hue: callers fold the rest into "other" ([`other_color`]).

use gpui::{Hsla, Rgba, rgb};
use kubyl_ui::Colors;

/// How many distinct series colors there are.
pub const SERIES_COLORS: usize = 4;

const DARK: [u32; SERIES_COLORS] = [0x67aaed, 0xb9742e, 0xc079df, 0x4fcdcd];
const LIGHT: [u32; SERIES_COLORS] = [0x3072c1, 0xbf7028, 0x8a3aa0, 0x20acb3];

/// Whether the theme is a light one (dark text on a light background).
pub fn is_light(colors: &Colors) -> bool {
    colors.background.l > 0.5
}

/// Color of the series at `index` (fixed order; wraps only past [`SERIES_COLORS`], which callers
/// avoid by folding into "other").
pub fn series_color(index: usize, colors: &Colors) -> Hsla {
    let table = if is_light(colors) { LIGHT } else { DARK };
    let rgba: Rgba = rgb(table[index % SERIES_COLORS]);
    rgba.into()
}

/// The "other" series: neutral, drawn dashed so it never reads as a fifth category.
pub fn other_color(colors: &Colors) -> Hsla {
    colors.text_dim
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Perceptual (OKLab) lightness.
    fn oklab_l(color: Hsla) -> f32 {
        let rgba = Rgba::from(color);
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(rgba.r), lin(rgba.g), lin(rgba.b));
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s
    }

    #[test]
    fn palettes_follow_the_theme() {
        let dark = Colors::one_dark();
        let light = Colors::one_light();
        assert!(!is_light(&dark));
        assert!(is_light(&light));
        assert_ne!(series_color(0, &dark), series_color(0, &light));
        // Adjacent series differ in lightness, not just hue.
        for colors in [&dark, &light] {
            for i in 0..SERIES_COLORS - 1 {
                let a = oklab_l(series_color(i, colors));
                let b = oklab_l(series_color(i + 1, colors));
                assert!((a - b).abs() > 0.05, "series {i} and {} too close", i + 1);
            }
        }
    }
}
