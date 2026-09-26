//! The `"kubeconfig_editor"` section of settings.json. Context overrides (display name, color,
//! production, read-only) stay in the `"kubernetes"` section (phase 01), never in kubeconfigs.

use std::path::Path;

use gpui::App;
use kubyl_settings::{Settings, SettingsSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kubeconfig editor settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct KubeconfigSettings {
    /// Timestamped backups kept per kubeconfig (`<config dir>/kubeconfig-backups/`).
    pub backups_kept: usize,
    /// Offer "Edit this file" for kubeconfigs Kubyl doesn't own (`~/.kube/config`,
    /// `$KUBECONFIG`, added files). With `false` they stay read-only; "Save as a Kubyl copy"
    /// still works.
    pub allow_external_edits: bool,
    /// Kubeconfigs Kubyl doesn't own that the user turned editing on for.
    pub editable_files: Vec<String>,
}

impl Default for KubeconfigSettings {
    fn default() -> Self {
        Self {
            backups_kept: 10,
            allow_external_edits: true,
            editable_files: Vec::new(),
        }
    }
}

impl SettingsSection for KubeconfigSettings {
    const KEY: Option<&'static str> = Some("kubeconfig_editor");
}

impl KubeconfigSettings {
    /// Whether the user turned editing on for `path`.
    pub fn opted_in(&self, path: &Path) -> bool {
        self.allow_external_edits
            && self
                .editable_files
                .iter()
                .any(|p| kubyl_kube::settings::expand_home(p) == path)
    }
}

/// Turns editing on (or off) for a file Kubyl doesn't own.
pub fn set_opt_in(path: &Path, on: bool, cx: &mut App) {
    let display = kubyl_kube::settings::display_path(path);
    let path = path.to_path_buf();
    Settings::update::<KubeconfigSettings>(cx, move |settings| {
        settings
            .editable_files
            .retain(|p| kubyl_kube::settings::expand_home(p) != path);
        if on {
            settings.editable_files.push(display);
        }
    });
}
