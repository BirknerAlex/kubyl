//! Series colors.
//!
//! Eight slots in a fixed order. The first four are the theme's accent, orange, purple and cyan
//! hues (the mockup's order); slots 5–8 add magenta, mint, indigo and amber, chosen away from
//! the status green/yellow/red. Lightness and chroma are re-stepped per theme so neighbouring
//! slots also differ in lightness and stay apart under color-vision deficiencies. Checked with
//! the dataviz palette validator against the card surface (`panel`), adjacent pairs:
//!
//! - One Dark (`#2f343e`): worst CVD ΔE 11.3, normal-vision ΔE 23.8, every color ≥ 3:1. The
//!   validator's dark lightness band (0.48–0.67) assumes a near-black surface; on this lighter
//!   panel it was shifted up by 0.1 so the marks keep 3:1 contrast.
//! - One Light (`#f0f0f1`): worst CVD ΔE 8.5, normal-vision ΔE 26.1. Cyan, mint and amber are
//!   below 3:1, so charts always show a legend and values in the tooltip.
//!
//! Eight hues can't all be told apart when any two meet, so the [`ColorRegistry`] hands out the
//! first slots first: the common case (four or fewer entities across a view's charts) stays on
//! the four safest colors. A chart shows at most four series plus a dashed, neutral "other".

use std::collections::{HashMap, HashSet};

use gpui::{App, Global, Hsla, Rgba, SharedString, rgb};
use kubyl_ui::Colors;

/// How many distinct series colors there are.
pub const SERIES_COLORS: usize = 8;

const DARK: [u32; SERIES_COLORS] = [
    0x67aaed, 0xb9742e, 0xc079df, 0x4fcdcd, 0xd9639c, 0x7ad4a5, 0x7178d5, 0xd4a14a,
];
const LIGHT: [u32; SERIES_COLORS] = [
    0x3072c1, 0xbf7028, 0x8a3aa0, 0x20acb3, 0xba3f7f, 0x3aa273, 0x4343a2, 0xbb881a,
];

/// Whether the theme is a light one (dark text on a light background).
pub fn is_light(colors: &Colors) -> bool {
    colors.background.l > 0.5
}

/// Color of palette slot `index` (wraps past [`SERIES_COLORS`]).
pub fn series_color(index: usize, colors: &Colors) -> Hsla {
    let table = if is_light(colors) { LIGHT } else { DARK };
    let rgba: Rgba = rgb(table[index % SERIES_COLORS]);
    rgba.into()
}

/// The "other" series: neutral, drawn dashed so it never reads as another category.
pub fn other_color(colors: &Colors) -> Hsla {
    colors.text_dim
}

/// Colors that follow an entity (a namespace, pod, node…) across every chart of a scope, so
/// `payments` is the same color on the CPU, memory and network charts, in the overview and in
/// the details.
///
/// Scopes are free-form keys, e.g. `<cluster id>/namespace`. An entity keeps its slot while it
/// keeps showing up; a new one takes the lowest free slot, or the slot whose entity was seen
/// longest ago when all eight are taken. Within one chart, slots are always distinct.
#[derive(Default)]
pub struct ColorRegistry {
    scopes: HashMap<SharedString, Slots>,
}

impl Global for ColorRegistry {}

impl ColorRegistry {
    /// Palette slots for the series of one chart, in the order of `names`.
    pub fn assign(cx: &mut App, scope: &str, names: &[SharedString]) -> Vec<usize> {
        cx.default_global::<Self>()
            .scopes
            .entry(SharedString::from(scope.to_string()))
            .or_default()
            .assign(names)
    }
}

#[derive(Default)]
struct Slots {
    /// Entity → (slot, last seen).
    owners: HashMap<SharedString, (usize, u64)>,
    clock: u64,
}

impl Slots {
    fn assign(&mut self, names: &[SharedString]) -> Vec<usize> {
        self.clock += 1;
        let mut used = HashSet::new();
        let mut out: Vec<Option<usize>> = vec![None; names.len()];
        // Entities that already have a slot keep it (unless another series of this chart does).
        for (i, name) in names.iter().enumerate() {
            if let Some((slot, seen)) = self.owners.get_mut(name)
                && used.insert(*slot)
            {
                *seen = self.clock;
                out[i] = Some(*slot);
            }
        }
        for (i, name) in names.iter().enumerate() {
            if out[i].is_some() {
                continue;
            }
            let last_seen = |slot: usize| {
                self.owners
                    .values()
                    .filter(|(s, _)| *s == slot)
                    .map(|(_, seen)| *seen)
                    .max()
            };
            // A free slot first (lowest index), else the one whose entity was seen longest ago.
            let slot = (0..SERIES_COLORS)
                .filter(|s| !used.contains(s))
                .min_by_key(|s| (last_seen(*s).is_some(), last_seen(*s).unwrap_or(0), *s))
                .unwrap_or(i % SERIES_COLORS);
            used.insert(slot);
            self.owners.retain(|_, (s, _)| *s != slot);
            self.owners.insert(name.clone(), (slot, self.clock));
            out[i] = Some(slot);
        }
        out.into_iter().map(|s| s.unwrap_or(0)).collect()
    }
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

    fn names(list: &[&str]) -> Vec<SharedString> {
        list.iter()
            .map(|s| SharedString::from(s.to_string()))
            .collect()
    }

    #[test]
    fn entities_keep_their_color_across_charts() {
        let mut slots = Slots::default();
        // CPU chart, then memory chart with a different order and one new namespace.
        assert_eq!(
            slots.assign(&names(&["ngp", "kube-system", "keycloak", "monitoring"])),
            [0, 1, 2, 3]
        );
        assert_eq!(
            slots.assign(&names(&["ngp", "keycloak", "kube-system", "web"])),
            [0, 2, 1, 4]
        );
        // Next refresh of the CPU chart: unchanged.
        assert_eq!(
            slots.assign(&names(&["keycloak", "monitoring", "ngp", "kube-system"])),
            [2, 3, 0, 1]
        );
        // All eight taken: the least recently seen entity gives its slot up.
        slots.assign(&names(&["a", "b", "c"]));
        let fresh = slots.assign(&names(&["z"]));
        assert_eq!(fresh, [4], "\"web\" was seen longest ago");
    }

    #[test]
    fn slots_are_distinct_within_a_chart() {
        let mut slots = Slots::default();
        slots.assign(&names(&["a"]));
        let many = names(&["a", "b", "c", "d", "e", "f", "g", "h"]);
        let assigned = slots.assign(&many);
        let unique: HashSet<usize> = assigned.iter().copied().collect();
        assert_eq!(unique.len(), SERIES_COLORS);
        assert_eq!(assigned[0], 0);
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
