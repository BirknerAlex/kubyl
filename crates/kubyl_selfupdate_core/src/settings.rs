//! The `updates` section of `settings.json`.

use kubyl_settings::SettingsSection;
use serde::{Deserialize, Serialize};

use crate::manifest::Channel;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct SelfUpdateSettings {
    /// Checks and offers updates automatically. Off still lets a manual check run (not wired
    /// to a UI action yet — see `plans/09-packaging-release.md`).
    pub auto_check: bool,
    pub channel: Channel,
}

impl Default for SelfUpdateSettings {
    fn default() -> Self {
        Self {
            auto_check: true,
            channel: Channel::Stable,
        }
    }
}

impl SettingsSection for SelfUpdateSettings {
    // Distinct from `kubyl_updates`' "updates" key (Kubernetes cluster updates) — this is the
    // Kubyl app checking for its own new versions.
    const KEY: Option<&'static str> = Some("self_update");
}
