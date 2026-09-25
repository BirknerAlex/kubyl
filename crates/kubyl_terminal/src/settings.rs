//! The `"terminal"` section of settings.json.

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where `s` (exec shell) opens terminals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpenIn {
    /// A tab of the Terminal panel in the bottom dock.
    #[default]
    Panel,
    /// An editor tab in the center pane.
    Tab,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalSettings {
    /// Overrides shell auto-detection (`/bin/bash` -> `/bin/sh` -> `sh`) for every exec.
    pub shell_override: Option<String>,
    /// Scrollback lines kept per terminal.
    pub scrollback_lines: usize,
    /// Font size of terminals, in pixels.
    pub font_size: f32,
    /// Option/Alt sends Escape-prefixed keys (meta), for shell word movement and Emacs keys.
    /// Off: Option types the layout's characters (macOS).
    pub option_as_meta: bool,
    /// Where exec shells open.
    pub open_in: OpenIn,
    /// Image of ephemeral debug containers (`kubectl debug`).
    pub debug_image: String,
    /// Image of node-shell pods; needs `nsenter` (busybox and alpine have it).
    pub node_shell_image: String,
    /// Namespace node-shell pods are created in (it must allow privileged pods).
    pub node_shell_namespace: String,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            shell_override: None,
            scrollback_lines: 10_000,
            font_size: 13.0,
            option_as_meta: !cfg!(target_os = "macos"),
            open_in: OpenIn::Panel,
            debug_image: "busybox:1.37".into(),
            node_shell_image: "busybox:1.37".into(),
            node_shell_namespace: "kube-system".into(),
        }
    }
}

impl SettingsSection for TerminalSettings {
    const KEY: Option<&'static str> = Some("terminal");
}
