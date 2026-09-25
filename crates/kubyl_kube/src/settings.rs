//! The `"kubernetes"` section of settings.json: kubeconfig sources and per-context overrides.
//!
//! Overrides live here and never in the kubeconfig files, which Kubyl doesn't modify.

use std::collections::BTreeMap;

use gpui::{App, Hsla};
use kubyl_settings::SettingsSection;
use kubyl_ui::Colors;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kubeconfig sources and per-context settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct KubeSettings {
    /// Load `~/.kube/config`.
    pub load_default_kubeconfig: bool,
    /// Load every file listed in `$KUBECONFIG`.
    pub load_kubeconfig_env: bool,
    /// Extra kubeconfig files or folders (every file in a folder is loaded). `~` is expanded.
    /// Only the kubeconfig editor writes them, after the user turns on editing for a file.
    pub kubeconfigs: Vec<String>,
    /// Per-context overrides, keyed by the cluster id (`<context>@<kubeconfig path>`).
    pub contexts: BTreeMap<String, ContextSettings>,
    /// Seconds between health pings of connected clusters.
    pub health_check_interval: u64,
}

impl Default for KubeSettings {
    fn default() -> Self {
        Self {
            load_default_kubeconfig: true,
            load_kubeconfig_env: true,
            kubeconfigs: Vec::new(),
            contexts: BTreeMap::new(),
            health_check_interval: 30,
        }
    }
}

impl SettingsSection for KubeSettings {
    const KEY: Option<&'static str> = Some("kubernetes");
}

impl KubeSettings {
    pub fn context(&self, id: &str) -> ContextSettings {
        self.contexts.get(id).cloned().unwrap_or_default()
    }
}

/// Settings for one context. Every field is optional; defaults come from the kubeconfig.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ContextSettings {
    /// Name shown instead of the context name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Color tag of the cluster root, favorites and title bar icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorTag>,
    /// Namespace selected when switching to this cluster (overrides the kubeconfig namespace).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_namespace: Option<String>,
    /// Red PROD badge and typed confirmation for destructive actions.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub production: bool,
    /// Blocks mutating requests from this app.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    /// Hides the context from the sidebar and the cluster switcher.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Namespaces to offer when listing namespaces is forbidden.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<String>,
}

/// A cluster color tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorTag {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Purple,
    Gray,
}

impl ColorTag {
    pub const ALL: [ColorTag; 8] = [
        ColorTag::Red,
        ColorTag::Orange,
        ColorTag::Yellow,
        ColorTag::Green,
        ColorTag::Cyan,
        ColorTag::Blue,
        ColorTag::Purple,
        ColorTag::Gray,
    ];

    pub fn color(self, colors: &Colors) -> Hsla {
        match self {
            ColorTag::Red => colors.red,
            ColorTag::Orange => colors.orange,
            ColorTag::Yellow => colors.yellow,
            ColorTag::Green => colors.green,
            ColorTag::Cyan => colors.cyan,
            ColorTag::Blue => colors.accent,
            ColorTag::Purple => colors.purple,
            ColorTag::Gray => colors.text_faint,
        }
    }

    /// A stable default for contexts without a tag, so a cluster keeps its color across runs.
    /// Production clusters default to red.
    pub fn default_for(id: &str, production: bool) -> Self {
        if production {
            return ColorTag::Red;
        }
        const PALETTE: [ColorTag; 5] = [
            ColorTag::Blue,
            ColorTag::Green,
            ColorTag::Cyan,
            ColorTag::Purple,
            ColorTag::Orange,
        ];
        // FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
        let hash = id.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
        PALETTE[(hash % PALETTE.len() as u64) as usize]
    }
}

/// The color of a context: its tag, or a stable default.
pub fn context_color(id: &str, settings: &ContextSettings, cx: &App) -> Hsla {
    use kubyl_ui::ActiveColors as _;
    settings
        .color
        .unwrap_or_else(|| ColorTag::default_for(id, settings.production))
        .color(cx.colors())
}

/// Expands a leading `~` to the home directory.
pub fn expand_home(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~")
        && (rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest.trim_start_matches(['/', '\\']));
    }
    std::path::PathBuf::from(path)
}

/// Shortens a path under the home directory to `~/…` for display.
pub fn display_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        let sep = std::path::MAIN_SEPARATOR;
        return format!("~{sep}{}", rest.display());
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_default_and_env_kubeconfigs() {
        let settings: KubeSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.load_default_kubeconfig);
        assert!(settings.load_kubeconfig_env);
        assert_eq!(settings.health_check_interval, 30);
    }

    #[test]
    fn context_settings_skip_defaults_when_serialized() {
        let settings = ContextSettings {
            production: true,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&settings).unwrap(),
            serde_json::json!({ "production": true })
        );
    }

    #[test]
    fn default_color_is_stable_and_red_for_production() {
        assert_eq!(ColorTag::default_for("a", true), ColorTag::Red);
        assert_eq!(
            ColorTag::default_for("kind-dev@/x", false),
            ColorTag::default_for("kind-dev@/x", false)
        );
    }

    #[test]
    fn expands_home() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/x/y"), home.join("x/y"));
        assert_eq!(expand_home("/abs"), std::path::PathBuf::from("/abs"));
        assert_eq!(
            display_path(&home.join("a")),
            format!("~{}a", std::path::MAIN_SEPARATOR)
        );
    }
}
