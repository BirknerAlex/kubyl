//! The `"updates"` section of settings.json.

use std::collections::BTreeMap;

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Cluster update settings.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct UpdatesSettings {
    /// Per cluster (entry id, member id or context name): where the cloud provider finds the
    /// cluster when the kubeconfig doesn't say.
    pub clusters: BTreeMap<String, ClusterUpdateSettings>,
}

impl SettingsSection for UpdatesSettings {
    const KEY: Option<&'static str> = Some("updates");
}

/// One cluster's overrides.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ClusterUpdateSettings {
    pub eks: Option<EksSettings>,
    pub gke: Option<GkeSettings>,
    pub aks: Option<AksSettings>,
}

/// An EKS cluster (defaults come from the `aws eks get-token` exec plugin).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EksSettings {
    pub cluster: Option<String>,
    pub region: Option<String>,
    /// The AWS CLI profile.
    pub profile: Option<String>,
}

/// A GKE cluster (defaults come from the `gke_<project>_<location>_<cluster>` names).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GkeSettings {
    pub project: Option<String>,
    /// A region or zone.
    pub location: Option<String>,
    pub cluster: Option<String>,
}

/// An AKS cluster (found by the API server's FQDN when unset).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AksSettings {
    pub subscription: Option<String>,
    pub resource_group: Option<String>,
    pub name: Option<String>,
}

impl UpdatesSettings {
    /// The overrides for a cluster, looked up by its settings keys (entry id, members,
    /// context names; `ConnectionManager::settings_keys`).
    pub fn for_keys(&self, keys: &[String]) -> ClusterUpdateSettings {
        keys.iter()
            .find_map(|k| self.clusters.get(k))
            .cloned()
            .unwrap_or_default()
    }
}
