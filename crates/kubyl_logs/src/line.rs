//! A single line in the ring buffer.

use gpui::Hsla;

use crate::level::{LogLevel, detect_level};

/// One log line, plus the metadata Kubyl attaches (source pod/container, detected level).
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Monotonic sequence number, assigned by the ring buffer. Stable identity for a line even
    /// after older lines are evicted.
    pub seq: u64,
    pub pod: String,
    pub container: String,
    /// Stable per-pod color prefix (hash of the pod name).
    pub pod_color_index: u8,
    pub text: String,
    pub level: LogLevel,
    /// `true` for the synthetic line inserted when a stream reconnects after a gap.
    pub gap_marker: bool,
}

impl LogLine {
    pub fn new(seq: u64, pod: String, container: String, text: String) -> Self {
        let level = detect_level(&text);
        let pod_color_index = color_index(&pod);
        Self {
            seq,
            pod,
            container,
            pod_color_index,
            text,
            level,
            gap_marker: false,
        }
    }

    pub fn gap(seq: u64, pod: String, container: String, message: String) -> Self {
        let mut line = Self::new(seq, pod, container, message);
        line.gap_marker = true;
        line.level = LogLevel::Unknown;
        line
    }
}

/// A small fixed palette of per-pod prefix colors, cycled by [`color_index`].
pub const POD_COLORS: usize = 8;

/// Stable index into a palette of `POD_COLORS` colors for a pod name.
pub fn color_index(pod: &str) -> u8 {
    let mut hash: u32 = 2166136261;
    for byte in pod.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    (hash % POD_COLORS as u32) as u8
}

/// Resolves a pod color index to an actual color from the theme's accent palette.
pub fn pod_color(index: u8, colors: &kubyl_ui::Colors) -> Hsla {
    let palette = [
        colors.accent,
        colors.green,
        colors.yellow,
        colors.purple,
        colors.cyan,
        colors.orange,
        colors.red,
        colors.text_muted,
    ];
    palette[index as usize % palette.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_index_is_stable_and_bounded() {
        let a = color_index("web-0");
        let b = color_index("web-0");
        assert_eq!(a, b);
        assert!((a as usize) < POD_COLORS);
    }
}
