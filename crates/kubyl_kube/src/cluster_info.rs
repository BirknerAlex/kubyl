//! What kind of cluster this is: distribution guess and capabilities (`ClusterCaps`).

use k8s_openapi::apimachinery::pkg::version::Info;
use kubyl_core::{ArgoCdCaps, ClusterCaps};

use crate::discovery::Discovery;
use crate::kubeconfig::ContextInfo;
use crate::settings::ContextSettings;

/// A best guess at the Kubernetes distribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distribution {
    Eks,
    Gke,
    Aks,
    OpenShift,
    K3s,
    Rke2,
    Kind,
    Minikube,
    DockerDesktop,
    Kubernetes,
}

impl Distribution {
    pub fn label(self) -> &'static str {
        match self {
            Distribution::Eks => "EKS",
            Distribution::Gke => "GKE",
            Distribution::Aks => "AKS",
            Distribution::OpenShift => "OpenShift",
            Distribution::K3s => "k3s",
            Distribution::Rke2 => "RKE2",
            Distribution::Kind => "kind",
            Distribution::Minikube => "minikube",
            Distribution::DockerDesktop => "Docker Desktop",
            Distribution::Kubernetes => "Kubernetes",
        }
    }

    /// Guesses from the version string, server URL, context name and served API groups.
    pub fn guess(version: &Info, context: &ContextInfo, discovery: Option<&Discovery>) -> Self {
        let git = version.git_version.to_lowercase();
        let server = context.server.as_deref().unwrap_or_default().to_lowercase();
        let names = format!("{} {}", context.context, context.cluster).to_lowercase();
        let has = |group: &str| discovery.is_some_and(|d| d.has_group(group));
        if has("config.openshift.io") || has("route.openshift.io") {
            Distribution::OpenShift
        } else if git.contains("-eks-") || server.contains(".eks.amazonaws.com") {
            Distribution::Eks
        } else if git.contains("-gke.") || names.starts_with("gke_") {
            Distribution::Gke
        } else if server.contains(".azmk8s.io") {
            Distribution::Aks
        } else if git.contains("+k3s") {
            Distribution::K3s
        } else if git.contains("+rke2") {
            Distribution::Rke2
        } else if names.starts_with("kind-") {
            Distribution::Kind
        } else if names.contains("minikube") {
            Distribution::Minikube
        } else if names.contains("docker-desktop") {
            Distribution::DockerDesktop
        } else {
            Distribution::Kubernetes
        }
    }
}

/// Facts about a connected cluster beyond [`ClusterCaps`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterInfo {
    /// `v1.33.1`.
    pub version: String,
    pub distribution: Distribution,
    pub platform: String,
    /// The Gateway API CRDs are installed.
    pub gateway_api: bool,
    /// Places a Prometheus may be found. Phase 07 probes them.
    pub prometheus_candidates: Vec<String>,
    /// The authenticated user, when the server tells.
    pub user: Option<String>,
}

impl ClusterInfo {
    pub fn new(version: &Info, context: &ContextInfo, user: Option<String>) -> Self {
        Self {
            version: version.git_version.clone(),
            distribution: Distribution::guess(version, context, None),
            platform: version.platform.clone(),
            gateway_api: false,
            prometheus_candidates: Vec::new(),
            user,
        }
    }

    /// Refines the guess and capabilities once discovery is done.
    pub fn apply_discovery(
        &mut self,
        version: &Info,
        context: &ContextInfo,
        discovery: &Discovery,
    ) {
        self.distribution = Distribution::guess(version, context, Some(discovery));
        self.gateway_api = discovery.has_group("gateway.networking.k8s.io");
        self.prometheus_candidates.clear();
        if discovery.has_group("monitoring.coreos.com") {
            self.prometheus_candidates
                .push("prometheus-operator (monitoring.coreos.com)".into());
        }
        if self.distribution == Distribution::OpenShift {
            self.prometheus_candidates
                .push("openshift-monitoring/thanos-querier".into());
        }
    }

    /// `EKS · v1.30.4`, for the title bar.
    pub fn summary(&self) -> String {
        let version = self
            .version
            .split(['-', '+'])
            .next()
            .unwrap_or(&self.version);
        format!("{} · {version}", self.distribution.label())
    }
}

/// Capabilities for action predicates, from discovery and the user's safety settings.
pub fn caps(
    discovery: Option<&Discovery>,
    info: Option<&ClusterInfo>,
    settings: &ContextSettings,
) -> ClusterCaps {
    let has = |group: &str| discovery.is_some_and(|d| d.has_group(group));
    ClusterCaps {
        read_only: settings.read_only,
        production: settings.production,
        openshift: info.is_some_and(|i| i.distribution == Distribution::OpenShift),
        metrics_server: has("metrics.k8s.io"),
        prometheus: info.is_some_and(|i| !i.prometheus_candidates.is_empty()),
        olm: has("operators.coreos.com") || has("olm.operatorframework.io"),
        argocd: argocd_caps(discovery),
    }
}

/// Which Argo CD CRDs are served. Argo Workflows, Rollouts and Events share `argoproj.io`, so
/// the group alone says nothing.
fn argocd_caps(discovery: Option<&Discovery>) -> ArgoCdCaps {
    let served = |resource: &str| {
        discovery.is_some_and(|d| {
            d.resources.iter().any(|r| {
                r.gvr.group == "argoproj.io" && r.gvr.resource == resource && r.is_listable()
            })
        })
    };
    ArgoCdCaps {
        applications: served("applications"),
        application_sets: served("applicationsets"),
        projects: served("appprojects"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMethod;
    use crate::kubeconfig::{CaSource, SourceKind};
    use kubyl_core::ClusterId;

    fn context(name: &str, server: &str) -> ContextInfo {
        ContextInfo {
            id: ClusterId::new(name),
            name: name.into(),
            context: name.into(),
            file: "/k".into(),
            source: SourceKind::Default,
            source_path: "/k".into(),
            cluster: name.into(),
            user: None,
            server: Some(server.into()),
            namespace: None,
            auth: AuthMethod::None,
            insecure_skip_tls_verify: false,
            proxy_url: None,
            tls_server_name: None,
            ca: CaSource::System,
            error: None,
            members: Vec::new(),
            group: None,
        }
    }

    fn version(git: &str) -> Info {
        Info {
            git_version: git.into(),
            ..Default::default()
        }
    }

    #[test]
    fn argocd_caps_follow_each_crd() {
        use crate::discovery::ApiResourceInfo;
        use kubyl_core::{Gvk, Gvr};
        let resource = |plural: &str, kind: &str| ApiResourceInfo {
            gvk: Gvk::new("argoproj.io", "v1alpha1", kind),
            gvr: Gvr::new("argoproj.io", "v1alpha1", plural),
            singular: kind.to_lowercase(),
            namespaced: true,
            verbs: vec!["list".into(), "watch".into()],
            short_names: vec![],
            categories: vec![],
            subresources: vec![],
            preferred: true,
        };
        let settings = ContextSettings::default();
        // Argo Workflows alone is not Argo CD.
        let workflows = Discovery {
            resources: vec![resource("workflows", "Workflow")],
            ..Default::default()
        };
        assert!(!caps(Some(&workflows), None, &settings).argocd.any());
        let apps_only = Discovery {
            resources: vec![
                resource("workflows", "Workflow"),
                resource("applications", "Application"),
            ],
            ..Default::default()
        };
        let argocd = caps(Some(&apps_only), None, &settings).argocd;
        assert!(argocd.any() && argocd.applications);
        assert!(!argocd.application_sets && !argocd.projects);
        assert_eq!(caps(None, None, &settings).argocd, ArgoCdCaps::default());
    }

    #[test]
    fn guesses_distributions() {
        let guess =
            |git, name, server| Distribution::guess(&version(git), &context(name, server), None);
        assert_eq!(
            guess("v1.30.4-eks-a737599", "prod", "https://x"),
            Distribution::Eks
        );
        assert_eq!(
            guess("v1.30.5-gke.1014001", "x", "https://1.2.3.4"),
            Distribution::Gke
        );
        assert_eq!(
            guess("v1.31.1+k3s1", "home", "https://1.2.3.4"),
            Distribution::K3s
        );
        assert_eq!(
            guess("v1.33.1", "kind-kubyl-dev", "https://127.0.0.1:1"),
            Distribution::Kind
        );
        assert_eq!(
            guess("v1.30.0", "x", "https://a.hcp.westeurope.azmk8s.io"),
            Distribution::Aks
        );
        assert_eq!(guess("v1.33.1", "x", "https://x"), Distribution::Kubernetes);

        let mut info = ClusterInfo::new(
            &version("v1.30.4-eks-a737599"),
            &context("p", "https://x"),
            None,
        );
        assert_eq!(info.summary(), "EKS · v1.30.4");
        let discovery = Discovery {
            groups: [
                "metrics.k8s.io",
                "operators.coreos.com",
                "monitoring.coreos.com",
                "gateway.networking.k8s.io",
            ]
            .iter()
            .map(|g| crate::discovery::ApiGroupInfo {
                name: g.to_string(),
                versions: vec!["v1".into()],
            })
            .collect(),
            ..Default::default()
        };
        info.apply_discovery(
            &version("v1.30.4-eks-a737599"),
            &context("p", "https://x"),
            &discovery,
        );
        assert!(info.gateway_api);
        let caps = caps(
            Some(&discovery),
            Some(&info),
            &ContextSettings {
                read_only: true,
                ..Default::default()
            },
        );
        assert!(caps.metrics_server && caps.olm && caps.prometheus && caps.read_only);
        assert!(!caps.openshift);
    }
}
