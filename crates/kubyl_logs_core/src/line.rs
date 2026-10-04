//! A single line in the ring buffer.

use gpui::{Hsla, SharedString};
use jiff::Timestamp;

use crate::level::{LogLevel, detect_level};

/// One log line, plus the metadata Kubyl attaches (source pod/container, detected level).
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Monotonic sequence number, assigned by the ring buffer. Stable identity for a line even
    /// after older lines are evicted.
    pub seq: u64,
    pub pod: SharedString,
    pub container: SharedString,
    /// Stable per-pod color prefix (hash of the pod name).
    pub pod_color_index: u8,
    /// When the container wrote the line (from the API's `timestamps=true` prefix).
    pub timestamp: Option<Timestamp>,
    pub text: String,
    pub level: LogLevel,
    /// `true` for the synthetic lines Kubyl inserts: reconnect gaps, pods joining or leaving.
    pub marker: bool,
}

impl LogLine {
    pub fn new(
        seq: u64,
        pod: SharedString,
        container: SharedString,
        timestamp: Option<Timestamp>,
        text: String,
    ) -> Self {
        let level = detect_level(&text);
        Self::with_level(seq, pod, container, timestamp, text, level)
    }

    /// Like [`Self::new`] with an already detected level (the view detects it once to keep its
    /// counters, the ring stores it).
    pub fn with_level(
        seq: u64,
        pod: SharedString,
        container: SharedString,
        timestamp: Option<Timestamp>,
        text: String,
        level: LogLevel,
    ) -> Self {
        let pod_color_index = color_index(&pod);
        Self {
            seq,
            pod,
            container,
            pod_color_index,
            timestamp,
            text,
            level,
            marker: false,
        }
    }

    pub fn marker(seq: u64, pod: SharedString, container: SharedString, message: String) -> Self {
        let mut line = Self::with_level(seq, pod, container, None, message, LogLevel::Unknown);
        line.marker = true;
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

/// The distinguishing part of a pod name: the random suffix of workload pods
/// (`checkout-api-7d9f8c6b5-x2kqp` → `x2kqp`, `ledger-writer-0` → `0` stays whole as
/// `ledger-writer-0`), so interleaved lines of replicas stay short.
pub fn short_pod_name(pod: &str) -> &str {
    let Some((prefix, suffix)) = pod.rsplit_once('-') else {
        return pod;
    };
    // Only suffixes Kubernetes generates (no vowels, no 0/1/3): `web-proxy` stays whole.
    const GENERATED: &[u8] = b"bcdfghjklmnpqrstvwxz2456789";
    let generated = suffix.len() == 5 && suffix.bytes().all(|b| GENERATED.contains(&b));
    if generated && !prefix.is_empty() {
        suffix
    } else {
        pod
    }
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

    #[test]
    fn short_names_keep_the_generated_suffix() {
        assert_eq!(short_pod_name("checkout-api-7d9f8c6b5-x2kqp"), "x2kqp");
        assert_eq!(short_pod_name("ledger-writer-0"), "ledger-writer-0");
        assert_eq!(short_pod_name("standalone"), "standalone");
        assert_eq!(short_pod_name("redis-cache"), "redis-cache");
    }
}
