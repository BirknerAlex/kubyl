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
    /// The provider to use when detection picks the wrong one (e.g. an EKS cluster behind a
    /// proxy URL).
    pub provider: Option<ProviderSetting>,
    pub eks: Option<EksSettings>,
    pub gke: Option<GkeSettings>,
    pub aks: Option<AksSettings>,
}

/// A provider picked in settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSetting {
    Openshift,
    Eks,
    Gke,
    Aks,
    K3s,
    Rke2,
    ClusterApi,
    SelfManaged,
}

impl ProviderSetting {
    pub fn kind(self) -> crate::model::ProviderKind {
        use crate::model::ProviderKind;
        match self {
            ProviderSetting::Openshift => ProviderKind::OpenShift,
            ProviderSetting::Eks => ProviderKind::Eks,
            ProviderSetting::Gke => ProviderKind::Gke,
            ProviderSetting::Aks => ProviderKind::Aks,
            ProviderSetting::K3s => ProviderKind::K3s,
            ProviderSetting::Rke2 => ProviderKind::Rke2,
            ProviderSetting::ClusterApi => ProviderKind::ClusterApi,
            ProviderSetting::SelfManaged => ProviderKind::SelfManaged,
        }
    }
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
