//! The `"helm"` section of settings.json.

use kubyl_settings_core::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Helm (phase 22): the `helm` Kubyl runs and the defaults of its writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HelmSettings {
    /// The `helm` to run: a path, or empty to find `helm` in your login shell's `PATH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Also search Artifact Hub (artifacthub.io) in the Charts tab. Off by default: Kubyl doesn't
    /// call third-party services unasked.
    pub artifact_hub: bool,
    /// Helm's `--timeout` for installs, upgrades, rollbacks and uninstalls (`5m0s`, `90s`).
    pub default_timeout: String,
    /// Roll back a failed install or upgrade (`--atomic`, Helm 4: `--rollback-on-failure`).
    pub atomic: bool,
}

impl Default for HelmSettings {
    fn default() -> Self {
        Self {
            path: None,
            artifact_hub: false,
            default_timeout: "5m0s".into(),
            atomic: true,
        }
    }
}

impl HelmSettings {
    /// `default_timeout` when Helm would take it, else Helm's own default (`5m0s`).
    pub fn timeout(&self) -> String {
        let timeout = self.default_timeout.trim();
        if crate::cmd::parse_duration(timeout).is_some() {
            timeout.to_string()
        } else {
            "5m0s".into()
        }
    }
}

impl SettingsSection for HelmSettings {
    const KEY: Option<&'static str> = Some("helm");
}
