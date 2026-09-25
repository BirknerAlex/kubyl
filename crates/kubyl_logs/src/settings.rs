//! The `"logs"` section of settings.json.

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ring::DEFAULT_CAPACITY;

/// Log viewer settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LogsSettings {
    /// Lines kept in the ring buffer per log view.
    pub ring_buffer_lines: usize,
    /// Show timestamps by default.
    pub timestamps: bool,
    /// Wrap long lines by default.
    pub wrap: bool,
    /// Follow (tail) new lines by default.
    pub follow: bool,
    /// Lines requested when a stream starts (`kubectl logs --tail`).
    pub tail_lines: i64,
}

impl Default for LogsSettings {
    fn default() -> Self {
        Self {
            ring_buffer_lines: DEFAULT_CAPACITY,
            timestamps: false,
            wrap: false,
            follow: true,
            tail_lines: 1000,
        }
    }
}

impl SettingsSection for LogsSettings {
    const KEY: Option<&'static str> = Some("logs");
}
