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
