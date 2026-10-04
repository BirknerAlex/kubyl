//! The providers. Kubernetes-native ones read through the cluster's own client; the cloud
//! ones (behind the `updates-eks`, `updates-gke`, `updates-aks` features) talk to their cloud
//! API with credentials read like the kubeconfig's exec plugin reads them.

#[cfg(feature = "updates-aks")]
pub mod aks;
#[cfg(any(
    feature = "updates-eks",
    feature = "updates-gke",
    feature = "updates-aks"
))]
pub mod cloud;
#[cfg(feature = "updates-eks")]
pub mod eks;
#[cfg(feature = "updates-gke")]
pub mod gke;

use std::path::PathBuf;

use crate::model::ProviderKind;
use crate::settings::ClusterUpdateSettings;

/// What a cloud provider gets to find its cluster and credentials. Nothing here is a secret:
/// credentials are fetched by the provider when it reads, and kept in memory only.
#[derive(Clone, Debug)]
pub struct CloudContext {
    /// Shown in messages (the cluster's display name).
    pub display_name: String,
    /// The kubeconfig file and the context and user entries (to read the exec plugin's
    /// arguments and environment).
    pub kubeconfig: PathBuf,
    pub context: String,
    pub user: Option<String>,
    /// The kubeconfig's cluster entry name.
    pub cluster_entry: String,
    /// The API server URL.
    pub server: String,
    pub settings: ClusterUpdateSettings,
}

/// Whether this build includes the provider of a cloud.
pub fn cloud_built(kind: ProviderKind) -> bool {
    let eks = cfg!(feature = "updates-eks");
    let gke = cfg!(feature = "updates-gke");
    let aks = cfg!(feature = "updates-aks");
    match kind {
        ProviderKind::Eks => eks,
        ProviderKind::Gke => gke,
        ProviderKind::Aks => aks,
        _ => true,
    }
}

/// The cargo feature of a cloud provider.
pub fn feature_of(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Eks => Some("updates-eks"),
        ProviderKind::Gke => Some("updates-gke"),
        ProviderKind::Aks => Some("updates-aks"),
        _ => None,
    }
}
