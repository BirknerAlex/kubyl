//! The `"explorer"` section of settings.json and what the explorer remembers in state.json.

use std::collections::BTreeSet;

use kubyl_settings::{SettingsSection, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Sidebar and list settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ExplorerSettings {
    /// Order of the groups under each cluster. Ids: `overview`, `events`, `workloads`,
    /// `network`, `config`, `storage`, `access`, `cluster`, `administration`, `custom`.
    /// Groups not listed follow in their default order.
    pub group_order: Vec<String>,
    /// Groups to hide (same ids as `group_order`).
    pub hidden_groups: Vec<String>,
    /// Show resource counts next to kinds in expanded groups (one metadata watch per kind).
    pub show_counts: bool,
    /// Grace period in seconds for `Delete` (empty: the object's default).
    pub default_grace_period: Option<u32>,
    /// Order of the cluster rows: `name` (by display name) or `connected_first`.
    pub cluster_order: ClusterOrder,
}

/// How the Clusters section sorts its rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterOrder {
    /// By display name.
    #[default]
    Name,
    /// Connected clusters first, then by display name.
    ConnectedFirst,
}

impl Default for ExplorerSettings {
    fn default() -> Self {
        Self {
            group_order: Vec::new(),
            hidden_groups: Vec::new(),
            show_counts: true,
            default_grace_period: None,
            cluster_order: ClusterOrder::Name,
        }
    }
}

impl SettingsSection for ExplorerSettings {
    const KEY: Option<&'static str> = Some("explorer");
}

/// Expanded tree nodes, remembered across restarts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TreeState {
    /// Expanded cluster roots (cluster ids).
    pub roots: BTreeSet<String>,
    /// Expanded groups: `<cluster id>|<group id>` (API groups of custom resources:
    /// `<cluster id>|custom:<group>`).
    pub groups: BTreeSet<String>,
    /// Collapsed sidebar sections (`favorites`, `clusters`).
    pub collapsed_sections: BTreeSet<String>,
    /// The Clusters section shows connected clusters only (and the active one).
    pub connected_only: bool,
}

impl StateSection for TreeState {
    const KEY: &'static str = "explorer";
}
