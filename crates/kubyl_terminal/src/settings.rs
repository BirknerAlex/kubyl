//! The `"terminal"` section of settings.json.

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalSettings {
    /// Overrides shell auto-detection (`/bin/bash` -> `/bin/sh` -> `sh`) for every exec.
    pub shell_override: Option<String>,
    /// Scrollback lines kept per terminal.
    pub scrollback_lines: usize,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell_override: None,
            scrollback_lines: 10_000,
        }
    }
}

impl SettingsSection for TerminalSettings {
    const KEY: Option<&'static str> = Some("terminal");
}
