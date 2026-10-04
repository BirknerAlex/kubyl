//! The `"prometheus"` section of settings.json. Query text and results are never stored.

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prometheus view settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PrometheusSettings {
    /// Show the Prometheus row under every cluster that has a Prometheus or Thanos Query.
    pub sidebar: bool,
    /// Seconds between reads of the open tab (targets, rules, overview).
    pub refresh_interval: u64,
}

impl Default for PrometheusSettings {
    fn default() -> Self {
        Self {
            sidebar: true,
            refresh_interval: 15,
        }
    }
}

impl SettingsSection for PrometheusSettings {
    const KEY: Option<&'static str> = Some("prometheus");
}

impl PrometheusSettings {
    pub fn refresh(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.refresh_interval.clamp(5, 3600))
    }
}
