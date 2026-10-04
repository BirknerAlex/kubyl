//! Which provider updates a cluster, in a fixed order (README "Cluster update providers"):
//! OpenShift, the managed clouds, k3s/RKE2, Cluster API management clusters, else
//! self-managed.

use kubyl_kube::cluster_info::Distribution;
use kubyl_kube::discovery::Discovery;

use crate::model::ProviderKind;

/// What detection looks at, gathered on the UI thread from the connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `/version`'s `gitVersion` (`v1.30.4-eks-1a2b3c`).
    pub git_version: String,
    /// The API server URL.
    pub server: String,
    /// The context and cluster entry names (GKE's `gke_<project>_<location>_<name>`).
    pub names: String,
    /// The exec plugin's command (`aws`, `gke-gcloud-auth-plugin`, `kubelogin`).
    pub exec_command: Option<String>,
    /// `clusterversions.config.openshift.io` is served.
    pub cluster_versions: bool,
    /// system-upgrade-controller's `plans.upgrade.cattle.io` is served.
    pub suc_plans: bool,
    /// Cluster API's `clusters.cluster.x-k8s.io` is served.
    pub capi_clusters: bool,
}

impl Facts {
    /// The served APIs detection needs, from discovery.
    pub fn with_discovery(mut self, discovery: &Discovery) -> Self {
        let served = |group: &str, resource: &str| {
            discovery
                .preferred()
                .any(|r| r.gvr.group == group && r.gvr.resource == resource)
        };
        self.cluster_versions = served("config.openshift.io", "clusterversions");
        self.suc_plans = served("upgrade.cattle.io", "plans");
        self.capi_clusters = served("cluster.x-k8s.io", "clusters");
        self
    }
}

/// The provider and why it was picked (shown in the header's tooltip).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub kind: ProviderKind,
    pub reason: String,
}

fn detected(kind: ProviderKind, reason: impl Into<String>) -> Detected {
    Detected {
        kind,
        reason: reason.into(),
    }
}

/// Picks the provider.
pub fn detect(facts: &Facts) -> Detected {
    let git = facts.git_version.to_lowercase();
    let server = facts.server.to_lowercase();
    let names = facts.names.to_lowercase();
    let exec = facts
        .exec_command
        .as_deref()
        .map(|c| {
            // `/usr/local/bin/aws`, `aws.exe`.
            let base = c.rsplit(['/', '\\']).next().unwrap_or(c);
            base.trim_end_matches(".exe")
                .trim_end_matches(".cmd")
                .to_lowercase()
        })
        .unwrap_or_default();
    if facts.cluster_versions {
        return detected(
            ProviderKind::OpenShift,
            "serves clusterversions.config.openshift.io",
        );
    }
    if git.contains("-eks-") || server.contains(".eks.amazonaws.com") {
        return detected(ProviderKind::Eks, "EKS API server");
    }
    if git.contains("-gke.") || names.split_whitespace().any(|n| n.starts_with("gke_")) {
        return detected(ProviderKind::Gke, "GKE version and context name");
    }
    if exec == "gke-gcloud-auth-plugin" {
        return detected(ProviderKind::Gke, "gke-gcloud-auth-plugin credentials");
    }
    if server.contains(".azmk8s.io") {
        return detected(ProviderKind::Aks, "AKS API server");
    }
    if git.contains("+k3s") {
        return detected(ProviderKind::K3s, "k3s version");
    }
    if git.contains("+rke2") {
        return detected(ProviderKind::Rke2, "RKE2 version");
    }
    if facts.capi_clusters {
        return detected(
            ProviderKind::ClusterApi,
            "serves clusters.cluster.x-k8s.io (a Cluster API management cluster)",
        );
    }
    detected(ProviderKind::SelfManaged, "no provider Kubyl can update")
}

/// The distribution label of a self-managed cluster (`kind`, `kubeadm`…), for the header.
pub fn self_managed_label(distribution: Option<Distribution>, facts: &Facts) -> &'static str {
    match distribution {
        Some(Distribution::Kind) => "kind",
        Some(Distribution::Minikube) => "minikube",
        Some(Distribution::DockerDesktop) => "Docker Desktop",
        Some(Distribution::OpenShift) => "MicroShift or OpenShift without ClusterVersion",
        _ if facts.git_version.is_empty() => "unknown",
        _ => "self-managed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(git: &str, server: &str) -> Facts {
        Facts {
            git_version: git.into(),
            server: server.into(),
            names: "ctx cluster".into(),
            ..Facts::default()
        }
    }

    #[test]
    fn detection_order() {
        let mut ocp = facts("v1.30.6", "https://api.ocp.example.com:6443");
        ocp.cluster_versions = true;
        // OpenShift wins even with other hints.
        ocp.capi_clusters = true;
        assert_eq!(detect(&ocp).kind, ProviderKind::OpenShift);
        assert_eq!(
            detect(&facts(
                "v1.30.4-eks-1a2b3c",
                "https://ABC.gr7.eu-west-1.eks.amazonaws.com"
            ))
            .kind,
            ProviderKind::Eks
        );
        assert_eq!(
            detect(&facts("v1.30.5-gke.1014001", "https://34.1.2.3")).kind,
            ProviderKind::Gke
        );
        let mut gke = facts("v1.30.5", "https://34.1.2.3");
        gke.exec_command = Some("/opt/google/bin/gke-gcloud-auth-plugin".into());
        assert_eq!(detect(&gke).kind, ProviderKind::Gke);
        assert_eq!(
            detect(&facts(
                "v1.30.3",
                "https://dev-abc123.hcp.westeurope.azmk8s.io:443"
            ))
            .kind,
            ProviderKind::Aks
        );
        assert_eq!(
            detect(&facts("v1.33.4+k3s1", "https://127.0.0.1:6443")).kind,
            ProviderKind::K3s
        );
        assert_eq!(
            detect(&facts("v1.33.4+rke2r1", "https://10.0.0.1:6443")).kind,
            ProviderKind::Rke2
        );
        let mut capi = facts("v1.37.0", "https://127.0.0.1:6443");
        capi.capi_clusters = true;
        assert_eq!(detect(&capi).kind, ProviderKind::ClusterApi);
        // Routes alone (MicroShift) aren't OpenShift's ClusterVersion.
        assert_eq!(
            detect(&facts("v1.37.0", "https://127.0.0.1:6443")).kind,
            ProviderKind::SelfManaged
        );
    }
}
