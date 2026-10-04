//! The `"overview"` section of settings.json and what the overview remembers in state.json.

use kubyl_settings::{SettingsSection, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Overview dashboard and events stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OverviewSettings {
    /// Series per chart (namespaces, pods) before the rest is folded into "other" (1–4).
    pub top_n: usize,
    /// Show `OOMKilled` warnings derived from pod status in the events stream (Kubernetes
    /// itself only reports the following `BackOff`).
    pub derived_events: bool,
    /// Notify about new Warning events in favorited namespaces (opt-in).
    pub notify_warnings: bool,
    /// Minimum seconds between two warning notifications for the same namespace.
    pub notify_interval: u64,
}

impl Default for OverviewSettings {
    fn default() -> Self {
        Self {
            top_n: 4,
            derived_events: true,
            notify_warnings: false,
            notify_interval: 60,
        }
    }
}

impl SettingsSection for OverviewSettings {
    const KEY: Option<&'static str> = Some("overview");
}

/// The last chosen time range (`15m`, `1h`, `6h`, `24h`, `7d`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverviewState {
    pub range: Option<String>,
}

impl StateSection for OverviewState {
    const KEY: &'static str = "overview";
}
