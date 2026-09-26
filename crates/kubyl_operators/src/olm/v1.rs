//! OLM v1 (`olm.operatorframework.io`): ClusterExtensions and ClusterCatalogs, read-only, plus
//! the YAML template that installs one.

use jiff::Timestamp;
use kubyl_core::Gvr;
use serde_json::Value;

use super::model::{Condition, conditions, string_at, time_at};

pub const GROUP: &str = "olm.operatorframework.io";

pub fn cluster_extensions() -> Gvr {
    Gvr::new(GROUP, "v1", "clusterextensions")
}

pub fn cluster_catalogs() -> Gvr {
    Gvr::new(GROUP, "v1", "clustercatalogs")
}

/// `olm.operatorframework.io/v1` ClusterExtension.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClusterExtension {
    pub name: String,
    pub package: Option<String>,
    /// `spec.source.catalog.version` (a version or range).
    pub version_constraint: Option<String>,
    pub channels: Vec<String>,
    /// Where the extension's namespaced objects go.
    pub namespace: Option<String>,
    pub service_account: Option<String>,
    /// The installed bundle.
    pub bundle: Option<String>,
    pub version: Option<String>,
    pub conditions: Vec<Condition>,
    pub created: Option<Timestamp>,
}

impl ClusterExtension {
    pub fn parse(object: &Value) -> Option<Self> {
        Some(Self {
            name: string_at(object, "/metadata/name")?,
            package: string_at(object, "/spec/source/catalog/packageName"),
            version_constraint: string_at(object, "/spec/source/catalog/version"),
            channels: object
                .pointer("/spec/source/catalog/channels")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            namespace: string_at(object, "/spec/namespace"),
            service_account: string_at(object, "/spec/serviceAccount/name"),
            bundle: string_at(object, "/status/install/bundle/name"),
            version: string_at(object, "/status/install/bundle/version"),
            conditions: conditions(object, "/status/conditions"),
            created: time_at(object, "/metadata/creationTimestamp"),
        })
    }

    fn condition(&self, kind: &str) -> Option<&Condition> {
        self.conditions.iter().find(|c| c.kind == kind)
    }

    /// `Installed`, `Installing`, `Failed` (`Installed=False` with a reason other than
    /// progress), `Deprecated` stays a note.
    pub fn status(&self) -> (&'static str, kubyl_core::Tone) {
        use kubyl_core::Tone;
        let installed = self.condition("Installed");
        let progressing = self.condition("Progressing");
        match (installed, progressing) {
            (Some(i), _) if i.is_true() => ("Installed", Tone::Good),
            (_, Some(p)) if p.reason.as_deref() == Some("Blocked") => ("Blocked", Tone::Bad),
            (_, Some(p)) if p.is_true() => ("Installing", Tone::Info),
            (Some(i), _) if i.status == "False" => ("Failed", Tone::Bad),
            _ => ("Unknown", Tone::Muted),
        }
    }
}

/// `olm.operatorframework.io/v1` ClusterCatalog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClusterCatalog {
    pub name: String,
    pub image: Option<String>,
    pub poll_minutes: Option<u64>,
    pub priority: Option<i64>,
    pub conditions: Vec<Condition>,
    pub last_unpacked: Option<Timestamp>,
}

impl ClusterCatalog {
    pub fn parse(object: &Value) -> Option<Self> {
        Some(Self {
            name: string_at(object, "/metadata/name")?,
            image: string_at(object, "/spec/source/image/ref"),
            poll_minutes: object
                .pointer("/spec/source/image/pollIntervalMinutes")
                .and_then(Value::as_u64),
            priority: object.pointer("/spec/priority").and_then(Value::as_i64),
            conditions: conditions(object, "/status/conditions"),
            last_unpacked: time_at(object, "/status/lastUnpacked"),
        })
    }

    pub fn serving(&self) -> bool {
        self.conditions
            .iter()
            .any(|c| c.kind == "Serving" && c.is_true())
    }
}

/// A ClusterExtension with the installer ServiceAccount it needs. OLM v1 installs with that
/// account's permissions: the template binds `cluster-admin` and says to narrow it.
pub fn extension_template(package: &str, namespace: &str) -> String {
    format!(
        r#"# OLM v1 installs an extension with the permissions of its installer service account.
# cluster-admin is the simplest start; narrow it to what the bundle needs for production.
# Upgrade later by editing spec.source.catalog.version (a version or a range like ">=1.2 <2").
apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: {package}-installer
  namespace: {namespace}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: {package}-installer
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: cluster-admin
subjects:
  - kind: ServiceAccount
    name: {package}-installer
    namespace: {namespace}
---
apiVersion: olm.operatorframework.io/v1
kind: ClusterExtension
metadata:
  name: {package}
spec:
  namespace: {namespace}
  serviceAccount:
    name: {package}-installer
  source:
    sourceType: Catalog
    catalog:
      packageName: {package}
      # channels: [stable]
      # version: ">=1.0.0"
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extensions_and_catalogs() {
        let ext = ClusterExtension::parse(&json!({
            "metadata": {"name": "argocd"},
            "spec": {"namespace": "argocd", "serviceAccount": {"name": "installer"},
                     "source": {"sourceType": "Catalog", "catalog": {"packageName": "argocd-operator", "version": ">=0.13"}}},
            "status": {"install": {"bundle": {"name": "argocd-operator.v0.13.0", "version": "0.13.0"}},
                       "conditions": [{"type": "Installed", "status": "True"}, {"type": "Progressing", "status": "True", "reason": "Succeeded"}]}
        }))
        .unwrap();
        assert_eq!(ext.package.as_deref(), Some("argocd-operator"));
        assert_eq!(ext.version.as_deref(), Some("0.13.0"));
        assert_eq!(ext.status().0, "Installed");
        let catalog = ClusterCatalog::parse(&json!({
            "metadata": {"name": "operatorhubio"},
            "spec": {"source": {"type": "Image", "image": {"ref": "quay.io/operatorhubio/catalog:latest", "pollIntervalMinutes": 10}}},
            "status": {"conditions": [{"type": "Serving", "status": "True"}]}
        }))
        .unwrap();
        assert!(catalog.serving());
        assert_eq!(catalog.poll_minutes, Some(10));
        // The template is valid YAML with four documents.
        let docs: Vec<Value> =
            serde_saphyr::from_multiple(&extension_template("argocd-operator", "argocd")).unwrap();
        assert_eq!(docs.len(), 4);
        assert_eq!(docs[3]["kind"], "ClusterExtension");
    }
}
