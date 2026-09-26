//! OLM v0 objects (`operators.coreos.com`) parsed from the watches' JSON: Subscriptions,
//! ClusterServiceVersions, InstallPlans, CatalogSources and OperatorGroups.
//!
//! Parsing keeps what the views, the join and the upgrade review need. Nothing here is Secret
//! data.

use std::collections::BTreeSet;

use jiff::Timestamp;
use kubyl_core::Gvr;
use serde_json::Value;

pub const GROUP: &str = "operators.coreos.com";
pub const PACKAGES_GROUP: &str = "packages.operators.coreos.com";

pub fn subscriptions() -> Gvr {
    Gvr::new(GROUP, "v1alpha1", "subscriptions")
}

pub fn csvs() -> Gvr {
    Gvr::new(GROUP, "v1alpha1", "clusterserviceversions")
}

pub fn install_plans() -> Gvr {
    Gvr::new(GROUP, "v1alpha1", "installplans")
}

pub fn catalog_sources() -> Gvr {
    Gvr::new(GROUP, "v1alpha1", "catalogsources")
}

pub fn operator_groups() -> Gvr {
    Gvr::new(GROUP, "v1", "operatorgroups")
}

pub fn package_manifests() -> Gvr {
    Gvr::new(PACKAGES_GROUP, "v1", "packagemanifests")
}

pub fn crds() -> Gvr {
    Gvr::new("apiextensions.k8s.io", "v1", "customresourcedefinitions")
}

/// The label OLM puts on the copies of a CSV it makes in every target namespace.
pub const COPIED_LABEL: &str = "olm.copiedFrom";

pub(crate) fn str_at<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer).and_then(Value::as_str)
}

pub(crate) fn string_at(v: &Value, pointer: &str) -> Option<String> {
    str_at(v, pointer)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn time_at(v: &Value, pointer: &str) -> Option<Timestamp> {
    str_at(v, pointer).and_then(|s| s.parse().ok())
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A status condition (Kubernetes style).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Condition {
    pub kind: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition: Option<Timestamp>,
}

impl Condition {
    pub fn is_true(&self) -> bool {
        self.status == "True"
    }
}

pub(crate) fn conditions(v: &Value, pointer: &str) -> Vec<Condition> {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|c| Condition {
                    kind: string_at(c, "/type").unwrap_or_default(),
                    status: string_at(c, "/status").unwrap_or_default(),
                    reason: string_at(c, "/reason"),
                    message: string_at(c, "/message"),
                    last_transition: time_at(c, "/lastTransitionTime"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `Automatic` or `Manual`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Approval {
    #[default]
    Automatic,
    Manual,
}

impl Approval {
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("Manual") => Approval::Manual,
            _ => Approval::Automatic,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Approval::Automatic => "Automatic",
            Approval::Manual => "Manual",
        }
    }
}

/// `operators.coreos.com/v1alpha1` Subscription.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Subscription {
    pub namespace: String,
    pub name: String,
    /// `spec.name`: the package.
    pub package: String,
    pub channel: Option<String>,
    pub source: String,
    pub source_namespace: String,
    pub approval: Approval,
    pub starting_csv: Option<String>,
    pub installed_csv: Option<String>,
    pub current_csv: Option<String>,
    /// `AtLatestKnown`, `UpgradePending`, `UpgradeAvailable`, `UpgradeFailed`…
    pub state: Option<String>,
    /// The InstallPlan the Subscription points at (`installPlanRef`, else `installplan`).
    pub install_plan: Option<String>,
    pub conditions: Vec<Condition>,
    /// `false` when a catalog it uses is unhealthy.
    pub catalog_healthy: Option<bool>,
    pub created: Option<Timestamp>,
    pub uid: String,
}

impl Subscription {
    pub fn parse(object: &Value) -> Option<Self> {
        let name = string_at(object, "/metadata/name")?;
        let catalog_health = object
            .pointer("/status/catalogHealth")
            .and_then(Value::as_array);
        Some(Self {
            namespace: string_at(object, "/metadata/namespace").unwrap_or_default(),
            name,
            package: string_at(object, "/spec/name").unwrap_or_default(),
            channel: string_at(object, "/spec/channel"),
            source: string_at(object, "/spec/source").unwrap_or_default(),
            source_namespace: string_at(object, "/spec/sourceNamespace").unwrap_or_default(),
            approval: Approval::parse(str_at(object, "/spec/installPlanApproval")),
            starting_csv: string_at(object, "/spec/startingCSV"),
            installed_csv: string_at(object, "/status/installedCSV"),
            current_csv: string_at(object, "/status/currentCSV"),
            state: string_at(object, "/status/state"),
            install_plan: string_at(object, "/status/installPlanRef/name")
                .or_else(|| string_at(object, "/status/installplan/name")),
            conditions: conditions(object, "/status/conditions"),
            catalog_healthy: catalog_health.map(|items| {
                items
                    .iter()
                    .all(|h| h.get("healthy").and_then(Value::as_bool) != Some(false))
            }),
            created: time_at(object, "/metadata/creationTimestamp"),
            uid: string_at(object, "/metadata/uid").unwrap_or_default(),
        })
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    /// A condition that is `True`, by type.
    pub fn condition(&self, kind: &str) -> Option<&Condition> {
        self.conditions
            .iter()
            .find(|c| c.kind == kind && c.is_true())
    }

    /// Why the subscription can't make progress, if it can't (`ResolutionFailed`…).
    pub fn failure(&self) -> Option<String> {
        for kind in [
            "ResolutionFailed",
            "InstallPlanFailed",
            "BundleUnpackFailed",
        ] {
            if let Some(c) = self.condition(kind) {
                return Some(
                    c.message
                        .clone()
                        .or_else(|| c.reason.clone())
                        .unwrap_or_else(|| kind.to_string()),
                );
            }
        }
        if self.state.as_deref() == Some("UpgradeFailed") {
            return Some("The upgrade failed.".into());
        }
        None
    }
}

/// A CRD the operator owns (`spec.customresourcedefinitions.owned[]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnedCrd {
    /// `<plural>.<group>`.
    pub name: String,
    pub version: String,
    pub kind: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

impl OwnedCrd {
    pub fn group(&self) -> &str {
        self.name.split_once('.').map_or("", |(_, g)| g)
    }

    pub fn plural(&self) -> &str {
        self.name.split_once('.').map_or(&self.name, |(p, _)| p)
    }

    pub fn gvr(&self) -> Gvr {
        Gvr::new(self.group(), &self.version, self.plural())
    }
}

pub(crate) fn owned_crds(v: Option<&Value>) -> Vec<OwnedCrd> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|c| {
                    Some(OwnedCrd {
                        name: string_at(c, "/name")?,
                        version: string_at(c, "/version").unwrap_or_default(),
                        kind: string_at(c, "/kind").unwrap_or_default(),
                        display_name: string_at(c, "/displayName"),
                        description: string_at(c, "/description"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `OwnNamespace`, `SingleNamespace`, `MultiNamespace`, `AllNamespaces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InstallMode {
    OwnNamespace,
    SingleNamespace,
    MultiNamespace,
    AllNamespaces,
}

impl InstallMode {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "OwnNamespace" => InstallMode::OwnNamespace,
            "SingleNamespace" => InstallMode::SingleNamespace,
            "MultiNamespace" => InstallMode::MultiNamespace,
            "AllNamespaces" => InstallMode::AllNamespaces,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            InstallMode::OwnNamespace => "Own namespace",
            InstallMode::SingleNamespace => "A single namespace",
            InstallMode::MultiNamespace => "Several namespaces",
            InstallMode::AllNamespaces => "All namespaces",
        }
    }
}

/// The install modes a CSV supports.
pub(crate) fn install_modes(v: Option<&Value>) -> BTreeSet<InstallMode> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|m| m.get("supported").and_then(Value::as_bool) == Some(true))
                .filter_map(|m| InstallMode::parse(str_at(m, "/type")?))
                .collect()
        })
        .unwrap_or_default()
}

/// An RBAC rule of a CSV's install strategy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PolicyRule {
    pub api_groups: Vec<String>,
    pub resources: Vec<String>,
    pub verbs: Vec<String>,
    pub resource_names: Vec<String>,
    pub non_resource_urls: Vec<String>,
}

/// The permissions one service account gets (`permissions` or `clusterPermissions`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Permission {
    pub service_account: String,
    pub rules: Vec<PolicyRule>,
}

pub(crate) fn permissions(v: Option<&Value>) -> Vec<Permission> {
    v.and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|p| Permission {
                    service_account: string_at(p, "/serviceAccountName").unwrap_or_default(),
                    rules: p
                        .get("rules")
                        .and_then(Value::as_array)
                        .map(|rules| {
                            rules
                                .iter()
                                .map(|r| PolicyRule {
                                    api_groups: strings(r.get("apiGroups")),
                                    resources: strings(r.get("resources")),
                                    verbs: strings(r.get("verbs")),
                                    resource_names: strings(r.get("resourceNames")),
                                    non_resource_urls: strings(r.get("nonResourceURLs")),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A bundle property (`olm.maxOpenShiftVersion`…), from the CSV's `olm.properties` or
/// `operatorframework.io/properties` annotation.
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub kind: String,
    pub value: Value,
}

/// Properties from both annotations: `olm.properties` (a JSON list, set by the author) and
/// `operatorframework.io/properties` (`{"properties": [...]}`, set by OLM).
pub(crate) fn properties(annotations: Option<&Value>) -> Vec<Property> {
    let mut out = Vec::new();
    let Some(annotations) = annotations else {
        return out;
    };
    let mut add = |items: &[Value]| {
        for item in items {
            if let Some(kind) = str_at(item, "/type") {
                let property = Property {
                    kind: kind.to_string(),
                    value: item.get("value").cloned().unwrap_or(Value::Null),
                };
                if !out.contains(&property) {
                    out.push(property);
                }
            }
        }
    };
    if let Some(text) = str_at(annotations, "/olm.properties")
        && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text)
    {
        add(&items);
    }
    if let Some(text) = annotations
        .get("operatorframework.io/properties")
        .and_then(Value::as_str)
        && let Ok(value) = serde_json::from_str::<Value>(text)
        && let Some(items) = value.get("properties").and_then(Value::as_array)
    {
        add(items);
    }
    out
}

/// A property's value as text (`"4.15"`, `4.15`).
pub(crate) fn property_text(properties: &[Property], kind: &str) -> Option<String> {
    let property = properties.iter().find(|p| p.kind == kind)?;
    match &property.value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `operators.coreos.com/v1alpha1` ClusterServiceVersion.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Csv {
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub version: Option<String>,
    pub provider: Option<String>,
    /// The short description (`metadata.annotations.description`).
    pub summary: Option<String>,
    /// `Succeeded`, `Installing`, `Failed`, `Pending`, `InstallReady`, `Replacing`, `Deleting`.
    pub phase: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub owned: Vec<OwnedCrd>,
    pub install_modes: BTreeSet<InstallMode>,
    pub min_kube_version: Option<String>,
    pub properties: Vec<Property>,
    /// operatorhub.io's `operatorhub.io/ui-metadata-max-k8s-version` annotation.
    pub listed_max_kube_version: Option<String>,
    /// `alm-examples`: a JSON list of example objects, as the author wrote it.
    pub alm_examples: Option<String>,
    pub capabilities: Option<String>,
    pub categories: Vec<String>,
    pub repository: Option<String>,
    pub container_image: Option<String>,
    /// `olm.operatorGroup`, `olm.targetNamespaces` (set by OLM).
    pub operator_group: Option<String>,
    pub target_namespaces: Option<String>,
    pub replaces: Option<String>,
    pub permissions: Vec<Permission>,
    pub cluster_permissions: Vec<Permission>,
    /// The operator's Deployments.
    pub deployments: Vec<String>,
    /// `spec.icon[0]`: media type and base64 data.
    pub icon: Option<(String, String)>,
    /// A copy OLM made (`olm.copiedFrom`).
    pub copied: bool,
    pub deleting: bool,
    /// The author marked it deprecated (`[DEPRECATED]` in the display name).
    pub deprecated: bool,
    pub created: Option<Timestamp>,
}

impl Csv {
    pub fn parse(object: &Value) -> Option<Self> {
        let name = string_at(object, "/metadata/name")?;
        let annotations = object.pointer("/metadata/annotations");
        let annotation = |key: &str| {
            annotations
                .and_then(|a| a.get(key))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let spec = object.get("spec").cloned().unwrap_or(Value::Null);
        let install = spec.pointer("/install/spec");
        let display_name = string_at(&spec, "/displayName").unwrap_or_else(|| name.clone());
        let deprecated = display_name.starts_with("[DEPRECATED]");
        Some(Self {
            namespace: string_at(object, "/metadata/namespace").unwrap_or_default(),
            display_name: display_name
                .trim_start_matches("[DEPRECATED]")
                .trim()
                .to_string(),
            deprecated,
            name,
            version: string_at(&spec, "/version"),
            provider: string_at(&spec, "/provider/name"),
            summary: annotation("description"),
            phase: string_at(object, "/status/phase").unwrap_or_default(),
            reason: string_at(object, "/status/reason"),
            message: string_at(object, "/status/message"),
            owned: owned_crds(spec.pointer("/customresourcedefinitions/owned")),
            install_modes: install_modes(spec.get("installModes")),
            min_kube_version: string_at(&spec, "/minKubeVersion"),
            properties: properties(annotations),
            listed_max_kube_version: annotation("operatorhub.io/ui-metadata-max-k8s-version"),
            alm_examples: annotation("alm-examples"),
            capabilities: annotation("capabilities"),
            categories: annotation("categories")
                .map(|c| {
                    c.split(',')
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            repository: annotation("repository"),
            container_image: annotation("containerImage"),
            operator_group: annotation("olm.operatorGroup"),
            target_namespaces: annotation("olm.targetNamespaces"),
            replaces: string_at(&spec, "/replaces"),
            permissions: permissions(install.and_then(|i| i.get("permissions"))),
            cluster_permissions: permissions(install.and_then(|i| i.get("clusterPermissions"))),
            deployments: install
                .and_then(|i| i.get("deployments"))
                .and_then(Value::as_array)
                .map(|d| d.iter().filter_map(|d| string_at(d, "/name")).collect())
                .unwrap_or_default(),
            icon: spec
                .pointer("/icon/0")
                .and_then(|i| Some((string_at(i, "/mediatype")?, string_at(i, "/base64data")?))),
            copied: object
                .pointer("/metadata/labels")
                .and_then(|l| l.get(COPIED_LABEL))
                .is_some(),
            deleting: object.pointer("/metadata/deletionTimestamp").is_some(),
            created: time_at(object, "/metadata/creationTimestamp"),
        })
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    /// `olm.maxOpenShiftVersion` (OpenShift blocks cluster upgrades past it).
    pub fn max_openshift_version(&self) -> Option<String> {
        property_text(&self.properties, "olm.maxOpenShiftVersion")
    }

    /// An upper Kubernetes version the author declared: an `olm.maxKubeVersion` property (not
    /// an OLM standard; OLM doesn't enforce it) or operatorhub.io's
    /// `operatorhub.io/ui-metadata-max-k8s-version` annotation. Advisory only.
    pub fn max_kube_version(&self) -> Option<String> {
        property_text(&self.properties, "olm.maxKubeVersion")
            .or_else(|| self.listed_max_kube_version.clone())
    }

    /// Whether the CSV reached its final successful phase.
    pub fn succeeded(&self) -> bool {
        self.phase == "Succeeded"
    }

    pub fn failed(&self) -> bool {
        self.phase == "Failed"
    }

    /// The `alm-examples` entries (objects), in the author's order. Bad JSON gives none.
    pub fn examples(&self) -> Vec<Value> {
        examples(self.alm_examples.as_deref())
    }
}

/// Parses `alm-examples` (a JSON list of objects).
pub fn examples(text: Option<&str>) -> Vec<Value> {
    text.and_then(|t| serde_json::from_str::<Value>(t).ok())
        .and_then(|v| match v {
            Value::Array(items) => Some(items.into_iter().filter(Value::is_object).collect()),
            _ => None,
        })
        .unwrap_or_default()
}

/// One step of an InstallPlan (`status.plan[]`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub kind: String,
    pub name: String,
    pub group: String,
    pub version: String,
    /// `Unknown`, `NotPresent`, `Present`, `Created`, `WaitingForAPI`…
    pub status: String,
    /// The object as JSON, or a reference to the bundle ConfigMap OLM unpacked (see
    /// [`BundleRef`]).
    pub manifest: String,
    pub source: Option<String>,
    pub source_namespace: Option<String>,
}

/// A step's manifest that points at an unpacked bundle ConfigMap instead of holding the object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BundleRef {
    pub namespace: String,
    pub name: String,
    pub replaces: Option<String>,
    /// The bundle's package and version (`olm.package` property), if listed.
    pub package: Option<(String, String)>,
}

impl BundleRef {
    /// `{"kind":"ConfigMap","name":…,"namespace":…,"catalogSourceName":…}` without an
    /// `apiVersion` is a reference; anything else is an inline object.
    pub fn parse(manifest: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(manifest).ok()?;
        if value.get("apiVersion").is_some() || str_at(&value, "/kind") != Some("ConfigMap") {
            return None;
        }
        value.get("catalogSourceName")?;
        let package = str_at(&value, "/properties")
            .and_then(|p| serde_json::from_str::<Value>(p).ok())
            .and_then(|p| {
                p.get("properties")?
                    .as_array()?
                    .iter()
                    .find(|p| str_at(p, "/type") == Some("olm.package"))
                    .and_then(|p| {
                        Some((
                            string_at(p, "/value/packageName")?,
                            string_at(p, "/value/version")?,
                        ))
                    })
            });
        Some(Self {
            namespace: string_at(&value, "/namespace")?,
            name: string_at(&value, "/name")?,
            replaces: string_at(&value, "/replaces"),
            package,
        })
    }
}

/// `operators.coreos.com/v1alpha1` InstallPlan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstallPlan {
    pub namespace: String,
    pub name: String,
    pub approval: Approval,
    pub approved: bool,
    pub csv_names: Vec<String>,
    /// `Planning`, `RequiresApproval`, `Installing`, `Complete`, `Failed`.
    pub phase: String,
    pub steps: Vec<Step>,
    pub conditions: Vec<Condition>,
    /// Subscriptions that own the plan (by name).
    pub subscriptions: Vec<String>,
    pub created: Option<Timestamp>,
    /// Bundles still being unpacked (no steps yet).
    pub unpacking: bool,
}

impl InstallPlan {
    pub fn parse(object: &Value) -> Option<Self> {
        let name = string_at(object, "/metadata/name")?;
        let steps = object
            .pointer("/status/plan")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .map(|s| Step {
                        kind: string_at(s, "/resource/kind").unwrap_or_default(),
                        name: string_at(s, "/resource/name").unwrap_or_default(),
                        group: string_at(s, "/resource/group").unwrap_or_default(),
                        version: string_at(s, "/resource/version").unwrap_or_default(),
                        status: string_at(s, "/status").unwrap_or_default(),
                        manifest: string_at(s, "/resource/manifest").unwrap_or_default(),
                        source: string_at(s, "/resource/sourceName"),
                        source_namespace: string_at(s, "/resource/sourceNamespace"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            namespace: string_at(object, "/metadata/namespace").unwrap_or_default(),
            name,
            approval: Approval::parse(str_at(object, "/spec/approval")),
            approved: object
                .pointer("/spec/approved")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            csv_names: strings(object.pointer("/spec/clusterServiceVersionNames")),
            phase: string_at(object, "/status/phase").unwrap_or_default(),
            steps,
            conditions: conditions(object, "/status/conditions"),
            subscriptions: object
                .pointer("/metadata/ownerReferences")
                .and_then(Value::as_array)
                .map(|refs| {
                    refs.iter()
                        .filter(|r| str_at(r, "/kind") == Some("Subscription"))
                        .filter_map(|r| string_at(r, "/name"))
                        .collect()
                })
                .unwrap_or_default(),
            created: time_at(object, "/metadata/creationTimestamp"),
            unpacking: object
                .pointer("/status/bundleLookups")
                .and_then(Value::as_array)
                .is_some_and(|l| !l.is_empty())
                && object
                    .pointer("/status/plan")
                    .and_then(Value::as_array)
                    .is_none_or(|p| p.is_empty()),
        })
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    /// Waiting for someone to approve it.
    pub fn needs_approval(&self) -> bool {
        self.phase == "RequiresApproval" && !self.approved
    }

    pub fn failed(&self) -> bool {
        self.phase == "Failed"
    }

    pub fn complete(&self) -> bool {
        self.phase == "Complete"
    }

    /// Why it failed (the `Installed` condition's message).
    pub fn failure(&self) -> Option<String> {
        self.conditions
            .iter()
            .find(|c| c.status == "False" && c.message.is_some())
            .and_then(|c| c.message.clone())
    }

    /// The version of a CSV this plan installs, from its step's bundle reference.
    pub fn version_of(&self, csv: &str) -> Option<String> {
        self.steps
            .iter()
            .find(|s| s.kind == "ClusterServiceVersion" && s.name == csv)
            .and_then(|s| BundleRef::parse(&s.manifest))
            .and_then(|r| r.package.map(|(_, v)| v))
            .or_else(|| version_from_csv_name(csv))
    }
}

/// `cloudnative-pg.v1.30.1` → `1.30.1` (the usual naming; `None` when it doesn't look so).
pub fn version_from_csv_name(csv: &str) -> Option<String> {
    let (_, tail) = csv.split_once(".v")?;
    tail.chars()
        .next()
        .filter(char::is_ascii_digit)
        .map(|_| tail.to_string())
}

/// `operators.coreos.com/v1alpha1` CatalogSource.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CatalogSource {
    pub namespace: String,
    pub name: String,
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub image: Option<String>,
    /// `READY`, `CONNECTING`, `TRANSIENT_FAILURE`…
    pub state: Option<String>,
}

impl CatalogSource {
    pub fn parse(object: &Value) -> Option<Self> {
        Some(Self {
            namespace: string_at(object, "/metadata/namespace").unwrap_or_default(),
            name: string_at(object, "/metadata/name")?,
            display_name: string_at(object, "/spec/displayName"),
            publisher: string_at(object, "/spec/publisher"),
            image: string_at(object, "/spec/image"),
            state: string_at(object, "/status/connectionState/lastObservedState"),
        })
    }

    pub fn label(&self) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| self.name.clone())
    }

    pub fn ready(&self) -> bool {
        self.state.as_deref() == Some("READY")
    }
}

/// `operators.coreos.com/v1` OperatorGroup.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperatorGroup {
    pub namespace: String,
    pub name: String,
    pub target_namespaces: Vec<String>,
    pub has_selector: bool,
    /// `status.namespaces` (`[""]`: all namespaces).
    pub status_namespaces: Vec<String>,
}

impl OperatorGroup {
    pub fn parse(object: &Value) -> Option<Self> {
        Some(Self {
            namespace: string_at(object, "/metadata/namespace").unwrap_or_default(),
            name: string_at(object, "/metadata/name")?,
            target_namespaces: strings(object.pointer("/spec/targetNamespaces")),
            has_selector: object
                .pointer("/spec/selector")
                .is_some_and(|s| s.as_object().is_some_and(|o| !o.is_empty())),
            status_namespaces: strings(object.pointer("/status/namespaces")),
        })
    }

    /// Targets every namespace (no target list, no selector).
    pub fn is_global(&self) -> bool {
        self.target_namespaces.is_empty() && !self.has_selector
    }

    /// Targets exactly its own namespace.
    pub fn is_own_namespace(&self) -> bool {
        !self.has_selector && self.target_namespaces == [self.namespace.clone()]
    }

    /// The install mode operators in this group run in.
    pub fn mode(&self) -> Option<InstallMode> {
        if self.is_global() {
            Some(InstallMode::AllNamespaces)
        } else if self.is_own_namespace() {
            Some(InstallMode::OwnNamespace)
        } else if self.target_namespaces.len() == 1 {
            Some(InstallMode::SingleNamespace)
        } else if self.target_namespaces.len() > 1 {
            Some(InstallMode::MultiNamespace)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subscriptions_parse_state_and_the_pending_plan() {
        let sub = Subscription::parse(&json!({
            "metadata": {"name": "cloudnative-pg", "namespace": "kubyl-manual", "uid": "u1"},
            "spec": {"name": "cloudnative-pg", "channel": "stable-v1", "source": "operatorhubio-catalog",
                     "sourceNamespace": "olm", "installPlanApproval": "Manual", "startingCSV": "cloudnative-pg.v1.30.0"},
            "status": {"installedCSV": "cloudnative-pg.v1.30.0", "currentCSV": "cloudnative-pg.v1.30.1",
                       "state": "UpgradePending", "installPlanRef": {"name": "install-54wd4"},
                       "catalogHealth": [{"healthy": true}],
                       "conditions": [{"type": "InstallPlanPending", "status": "True", "reason": "RequiresApproval"}]}
        }))
        .unwrap();
        assert_eq!(sub.approval, Approval::Manual);
        assert_eq!(sub.install_plan.as_deref(), Some("install-54wd4"));
        assert_eq!(sub.catalog_healthy, Some(true));
        assert!(sub.failure().is_none());
        assert!(sub.condition("InstallPlanPending").is_some());

        let failed = Subscription::parse(&json!({
            "metadata": {"name": "sail", "namespace": "istio-system"},
            "spec": {"name": "sailoperator"},
            "status": {"conditions": [{"type": "ResolutionFailed", "status": "True",
                       "message": "constraints not satisfiable"}]}
        }))
        .unwrap();
        assert_eq!(
            failed.failure().as_deref(),
            Some("constraints not satisfiable")
        );
    }

    #[test]
    fn csvs_parse_owned_crds_modes_properties_and_permissions() {
        let csv = Csv::parse(&json!({
            "metadata": {"name": "strimzi-cluster-operator.v0.43.0", "namespace": "kafka",
                "annotations": {
                    "alm-examples": "[{\"apiVersion\":\"kafka.strimzi.io/v1beta2\",\"kind\":\"Kafka\",\"metadata\":{\"name\":\"my-cluster\"}}]",
                    "categories": "Streaming & Messaging, Big Data",
                    "capabilities": "Deep Insights",
                    "olm.properties": "[{\"type\":\"olm.maxOpenShiftVersion\",\"value\":\"4.15\"}]",
                    "operatorframework.io/properties": "{\"properties\":[{\"type\":\"olm.package\",\"value\":{\"packageName\":\"strimzi-kafka-operator\",\"version\":\"0.43.0\"}},{\"type\":\"olm.maxKubeVersion\",\"value\":\"1.33\"}]}",
                    "olm.operatorGroup": "kafka-og", "olm.targetNamespaces": "kafka"}},
            "spec": {"displayName": "Strimzi", "version": "0.43.0", "minKubeVersion": "1.25.0",
                "customresourcedefinitions": {"owned": [
                    {"name": "kafkas.kafka.strimzi.io", "version": "v1beta2", "kind": "Kafka", "displayName": "Kafka"}]},
                "installModes": [{"type": "OwnNamespace", "supported": true}, {"type": "AllNamespaces", "supported": false}],
                "install": {"spec": {
                    "deployments": [{"name": "strimzi-cluster-operator"}],
                    "clusterPermissions": [{"serviceAccountName": "strimzi", "rules": [
                        {"apiGroups": [""], "resources": ["nodes"], "verbs": ["get", "list"]}]}]}}},
            "status": {"phase": "Succeeded"}
        }))
        .unwrap();
        assert_eq!(csv.display_name, "Strimzi");
        assert_eq!(
            csv.owned[0].gvr(),
            Gvr::new("kafka.strimzi.io", "v1beta2", "kafkas")
        );
        assert_eq!(
            csv.install_modes.iter().copied().collect::<Vec<_>>(),
            [InstallMode::OwnNamespace]
        );
        assert_eq!(csv.max_openshift_version().as_deref(), Some("4.15"));
        assert_eq!(csv.max_kube_version().as_deref(), Some("1.33"));
        assert_eq!(csv.categories, ["Streaming & Messaging", "Big Data"]);
        assert_eq!(csv.cluster_permissions[0].rules[0].resources, ["nodes"]);
        assert_eq!(csv.deployments, ["strimzi-cluster-operator"]);
        assert_eq!(csv.examples().len(), 1);
        assert!(csv.succeeded() && !csv.copied);
    }

    #[test]
    fn install_plan_steps_point_at_bundle_config_maps() {
        let manifest = r#"{"kind":"ConfigMap","name":"4e13d6","namespace":"olm","catalogSourceName":"operatorhubio-catalog","catalogSourceNamespace":"olm","replaces":"cloudnative-pg.v1.30.0","properties":"{\"properties\":[{\"type\":\"olm.package\",\"value\":{\"packageName\":\"cloudnative-pg\",\"version\":\"1.30.1\"}}]}"}"#;
        let plan = InstallPlan::parse(&json!({
            "metadata": {"name": "install-54wd4", "namespace": "kubyl-manual",
                "ownerReferences": [{"kind": "Subscription", "name": "cloudnative-pg"}]},
            "spec": {"approval": "Manual", "approved": false, "clusterServiceVersionNames": ["cloudnative-pg.v1.30.1"]},
            "status": {"phase": "RequiresApproval", "plan": [
                {"status": "Unknown", "resource": {"kind": "ClusterServiceVersion", "name": "cloudnative-pg.v1.30.1",
                  "group": "operators.coreos.com", "version": "v1alpha1", "manifest": manifest}}]}
        }))
        .unwrap();
        assert!(plan.needs_approval());
        assert_eq!(plan.subscriptions, ["cloudnative-pg"]);
        let reference = BundleRef::parse(&plan.steps[0].manifest).unwrap();
        assert_eq!(reference.namespace, "olm");
        assert_eq!(
            reference.package,
            Some(("cloudnative-pg".into(), "1.30.1".into()))
        );
        assert_eq!(
            plan.version_of("cloudnative-pg.v1.30.1").as_deref(),
            Some("1.30.1")
        );
        // An inline object isn't a reference.
        assert!(
            BundleRef::parse(r#"{"apiVersion":"v1","kind":"ConfigMap","metadata":{}}"#).is_none()
        );
        assert_eq!(
            version_from_csv_name("etcdoperator.v0.9.4").as_deref(),
            Some("0.9.4")
        );
        assert_eq!(version_from_csv_name("prometheusoperator.0.47.0"), None);
    }

    #[test]
    fn operator_groups_know_their_mode() {
        let global = OperatorGroup::parse(
            &json!({"metadata": {"name": "global-operators", "namespace": "operators"}}),
        )
        .unwrap();
        assert!(global.is_global());
        assert_eq!(global.mode(), Some(InstallMode::AllNamespaces));
        let own = OperatorGroup::parse(&json!({"metadata": {"name": "og", "namespace": "kafka"},
            "spec": {"targetNamespaces": ["kafka"]}}))
        .unwrap();
        assert_eq!(own.mode(), Some(InstallMode::OwnNamespace));
        let single = OperatorGroup::parse(&json!({"metadata": {"name": "og", "namespace": "ops"},
            "spec": {"targetNamespaces": ["apps"]}}))
        .unwrap();
        assert_eq!(single.mode(), Some(InstallMode::SingleNamespace));
    }
}
