//! IDs and references shared by every crate.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Identifies one cluster root in the sidebar: a context in a specific kubeconfig source.
///
/// Opaque to everyone but `kubyl_kube`, which builds it (for example from the kubeconfig path
/// and context name). Cheap to clone.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClusterId(Arc<str>);

impl ClusterId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClusterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of a context inside a kubeconfig.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextName(Arc<str>);

impl ContextName {
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContextName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Group, version and kind. The core group is the empty string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Gvk {
    pub group: String,
    pub version: String,
    pub kind: String,
}

impl Gvk {
    pub fn new(
        group: impl Into<String>,
        version: impl Into<String>,
        kind: impl Into<String>,
    ) -> Self {
        Self {
            group: group.into(),
            version: version.into(),
            kind: kind.into(),
        }
    }

    /// `v1` for the core group, `apps/v1` otherwise.
    pub fn api_version(&self) -> String {
        api_version(&self.group, &self.version)
    }
}

impl fmt::Display for Gvk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.api_version(), self.kind)
    }
}

/// Group, version and plural resource name, as used in API paths.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Gvr {
    pub group: String,
    pub version: String,
    pub resource: String,
}

impl Gvr {
    pub fn new(
        group: impl Into<String>,
        version: impl Into<String>,
        resource: impl Into<String>,
    ) -> Self {
        Self {
            group: group.into(),
            version: version.into(),
            resource: resource.into(),
        }
    }

    pub fn api_version(&self) -> String {
        api_version(&self.group, &self.version)
    }
}

impl fmt::Display for Gvr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.group.is_empty() {
            write!(f, "{}", self.resource)
        } else {
            write!(f, "{}.{}", self.resource, self.group)
        }
    }
}

fn api_version(group: &str, version: &str) -> String {
    if group.is_empty() {
        version.to_string()
    } else {
        format!("{group}/{version}")
    }
}

/// Points at a resource type, a namespace-scoped list, or a single object in one cluster.
///
/// - `name: None`: the list of `gvr` (in `namespace`, or all namespaces when that is `None`).
/// - `name: Some(_)`: one object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceRef {
    pub cluster: ClusterId,
    pub gvr: Gvr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ResourceRef {
    pub fn list(cluster: ClusterId, gvr: Gvr, namespace: Option<String>) -> Self {
        Self {
            cluster,
            gvr,
            namespace,
            name: None,
        }
    }

    pub fn object(cluster: ClusterId, gvr: Gvr, namespace: Option<String>, name: String) -> Self {
        Self {
            cluster,
            gvr,
            namespace,
            name: Some(name),
        }
    }

    pub fn is_object(&self) -> bool {
        self.name.is_some()
    }
}

/// What kind of tab or pane a view is. Persisted in `state.json` to restore open tabs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Welcome,
    Table,
    Details,
    Yaml,
    Logs,
    Terminal,
    Files,
    Overview,
    Events,
    Operators,
    Updates,
    Settings,
    /// Views added by feature crates that don't fit the list above.
    Custom(String),
}

impl ViewKind {
    pub fn as_str(&self) -> &str {
        match self {
            ViewKind::Welcome => "welcome",
            ViewKind::Table => "table",
            ViewKind::Details => "details",
            ViewKind::Yaml => "yaml",
            ViewKind::Logs => "logs",
            ViewKind::Terminal => "terminal",
            ViewKind::Files => "files",
            ViewKind::Overview => "overview",
            ViewKind::Events => "events",
            ViewKind::Operators => "operators",
            ViewKind::Updates => "updates",
            ViewKind::Settings => "settings",
            ViewKind::Custom(name) => name,
        }
    }
}

/// What a connected cluster supports. Filled in by `kubyl_kube` after discovery and used by
/// action availability predicates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterCaps {
    /// The user marked the cluster read-only; mutating actions must be hidden.
    pub read_only: bool,
    /// Production clusters get a red badge and typed confirmation for destructive actions.
    pub production: bool,
    pub openshift: bool,
    pub metrics_server: bool,
    pub prometheus: bool,
    pub olm: bool,
    /// Which Argo CD CRDs the cluster serves (some installs only have some of them).
    pub argocd: ArgoCdCaps,
    /// Which Flux CRDs the cluster serves (`*.toolkit.fluxcd.io`, phase 23).
    pub flux: FluxCaps,
    /// Which Trivy Operator report CRDs the cluster serves (`aquasecurity.github.io`, phase 25).
    pub trivy: TrivyCaps,
}

/// The Argo CD custom resources a cluster serves (`argoproj.io`). Argo Workflows, Rollouts and
/// Events share the group, so each kind is checked on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArgoCdCaps {
    /// `applications.argoproj.io`.
    pub applications: bool,
    /// `applicationsets.argoproj.io`.
    pub application_sets: bool,
    /// `appprojects.argoproj.io`.
    pub projects: bool,
}

impl ArgoCdCaps {
    /// Any Argo CD CRD is served: the Argo CD UI is shown.
    pub fn any(&self) -> bool {
        self.applications || self.application_sets || self.projects
    }
}

/// The Flux custom resources a cluster serves (`*.toolkit.fluxcd.io`). Each kind is checked on
/// its own: installs choose their components (image automation is optional).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FluxCaps {
    /// `kustomizations.kustomize.toolkit.fluxcd.io`.
    pub kustomizations: bool,
    /// `helmreleases.helm.toolkit.fluxcd.io`.
    pub helm_releases: bool,
    /// Any source kind of `source.toolkit.fluxcd.io` (GitRepository, OCIRepository,
    /// HelmRepository, HelmChart, Bucket, ExternalArtifact).
    pub sources: bool,
    /// Any kind of `image.toolkit.fluxcd.io` (ImageRepository, ImagePolicy,
    /// ImageUpdateAutomation).
    pub image_automation: bool,
    /// Any kind of `notification.toolkit.fluxcd.io` (Alert, Provider, Receiver).
    pub notifications: bool,
}

impl FluxCaps {
    /// Any Flux CRD is served: the Flux UI is shown.
    pub fn any(&self) -> bool {
        self.kustomizations
            || self.helm_releases
            || self.sources
            || self.image_automation
            || self.notifications
    }
}

/// The Trivy Operator report kinds a cluster serves (`aquasecurity.github.io`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrivyCaps {
    /// `vulnerabilityreports`: image vulnerabilities.
    pub vulnerabilities: bool,
    /// `configauditreports`: misconfigurations of workloads.
    pub config_audit: bool,
    /// `exposedsecretreports`: secrets baked into images.
    pub exposed_secrets: bool,
    /// `rbacassessmentreports`: Roles.
    pub rbac: bool,
    /// `clusterrbacassessmentreports`: ClusterRoles.
    pub cluster_rbac: bool,
}

impl TrivyCaps {
    /// Any report kind is served: the Security Center has something to read.
    pub fn any(&self) -> bool {
        self.vulnerabilities
            || self.config_audit
            || self.exposed_secrets
            || self.rbac
            || self.cluster_rbac
    }
}

/// The cluster and namespace a tab shows, for the title bar to follow when it's activated.
#[derive(Clone, Debug, PartialEq)]
pub struct TabContext {
    pub cluster: ClusterId,
    pub namespace: TabNamespace,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TabNamespace {
    /// The view follows the title bar's namespace (or has none): leave it.
    Keep,
    All,
    One(String),
}

/// Semantic color of a status value; the UI maps it to theme colors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    /// Running, Ready, Succeeded.
    Good,
    /// Pending, Upgrade available.
    Warning,
    /// CrashLoopBackOff, Failed, NotReady.
    Bad,
    /// ContainerCreating, Installing.
    Info,
    /// Completed, disabled.
    Muted,
}

/// One forward, as other crates see it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveForward {
    /// Pass to `kubyl_core::actions::StopForward`.
    pub id: u64,
    /// The pod, Service or workload that was forwarded.
    pub target: ResourceRef,
    /// The forwarded port as picked (`None` = the first port of the target).
    pub remote_port: Option<u16>,
    /// `localhost:18080` once the local port listens.
    pub local: Option<String>,
    /// `http://localhost:18080` for HTTP ports.
    pub url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_version_omits_the_core_group() {
        assert_eq!(Gvk::new("", "v1", "Pod").api_version(), "v1");
        assert_eq!(
            Gvk::new("apps", "v1", "Deployment").to_string(),
            "apps/v1/Deployment"
        );
        assert_eq!(Gvr::new("", "v1", "pods").to_string(), "pods");
        assert_eq!(
            Gvr::new("cert-manager.io", "v1", "certificates").to_string(),
            "certificates.cert-manager.io"
        );
    }

    #[test]
    fn resource_ref_round_trips_through_json() {
        let r = ResourceRef::object(
            ClusterId::new("kind-kubyl-dev"),
            Gvr::new("", "v1", "pods"),
            Some("default".into()),
            "web-0".into(),
        );
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<ResourceRef>(&json).unwrap(), r);
        assert!(r.is_object());
    }

    #[test]
    fn view_kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ViewKind::Welcome).unwrap(),
            "\"welcome\""
        );
        let custom = ViewKind::Custom("helm_releases".into());
        assert_eq!(
            serde_json::from_str::<ViewKind>(&serde_json::to_string(&custom).unwrap()).unwrap(),
            custom
        );
    }
}
