//! What the cluster tree shows: groups (Workloads, Network…) and the kinds in them, plus the
//! custom resources found by discovery, grouped by API group.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::{App, BorrowAppContext as _, Global, SharedString};
use kubyl_core::{ClusterId, Gvr, ViewKind};
use kubyl_kube::discovery::{ApiResourceInfo, Discovery};
use kubyl_ui::IconName;

/// A kind in the curated catalog, by group and plural resource name (the version comes from
/// discovery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownKind {
    pub group: &'static str,
    pub resource: &'static str,
    pub label: &'static str,
    pub icon: IconName,
}

/// A [`KnownKind`] (for [`TreeGroup::kinds`]).
pub const fn k(
    group: &'static str,
    resource: &'static str,
    label: &'static str,
    icon: IconName,
) -> KnownKind {
    KnownKind {
        group,
        resource,
        label,
        icon,
    }
}

/// A non-resource entry (Overview, Installed Operators…), opened as a view of the cluster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewEntry {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: IconName,
    pub kind: ViewKind,
    /// Only shown when the cluster runs OLM.
    pub needs_olm: bool,
}

/// A group of the cluster tree.
#[derive(Clone, Debug)]
pub struct GroupDef {
    pub id: &'static str,
    pub label: &'static str,
    /// `None`: the group is a single top-level row (Overview, Events).
    pub collapsible: bool,
    pub kinds: &'static [KnownKind],
    pub views: fn() -> Vec<ViewEntry>,
}

fn no_views() -> Vec<ViewEntry> {
    Vec::new()
}

fn overview_views() -> Vec<ViewEntry> {
    vec![ViewEntry {
        id: "overview",
        label: "Overview",
        icon: IconName::Gauge,
        kind: ViewKind::Overview,
        needs_olm: false,
    }]
}

fn administration_views() -> Vec<ViewEntry> {
    vec![
        ViewEntry {
            id: "operators",
            label: "Installed Operators",
            icon: IconName::Blocks,
            kind: ViewKind::Operators,
            needs_olm: true,
        },
        ViewEntry {
            id: "operatorhub",
            label: "OperatorHub",
            icon: IconName::Store,
            kind: ViewKind::Custom("operatorhub".into()),
            needs_olm: true,
        },
        ViewEntry {
            id: "updates",
            label: "Cluster Updates",
            icon: IconName::ArrowUp,
            kind: ViewKind::Updates,
            needs_olm: false,
        },
    ]
}

const EVENTS: &[KnownKind] = &[k("", "events", "Events", IconName::Bell)];

const WORKLOADS: &[KnownKind] = &[
    k("", "pods", "Pods", IconName::Box),
    k("apps", "deployments", "Deployments", IconName::Layers),
    k("apps", "statefulsets", "StatefulSets", IconName::Database),
    k("apps", "daemonsets", "DaemonSets", IconName::Server),
    k("apps", "replicasets", "ReplicaSets", IconName::Copy),
    k("batch", "jobs", "Jobs", IconName::Play),
    k("batch", "cronjobs", "CronJobs", IconName::Clock),
];

const NETWORK: &[KnownKind] = &[
    k("", "services", "Services", IconName::Network),
    k("", "endpoints", "Endpoints", IconName::Link),
    k(
        "networking.k8s.io",
        "ingresses",
        "Ingresses",
        IconName::Globe,
    ),
    k(
        "networking.k8s.io",
        "ingressclasses",
        "IngressClasses",
        IconName::Globe,
    ),
    k(
        "networking.k8s.io",
        "networkpolicies",
        "NetworkPolicies",
        IconName::Shield,
    ),
    k(
        "gateway.networking.k8s.io",
        "gateways",
        "Gateways",
        IconName::Globe,
    ),
    k(
        "gateway.networking.k8s.io",
        "httproutes",
        "HTTPRoutes",
        IconName::ArrowRight,
    ),
    k(
        "gateway.networking.k8s.io",
        "gatewayclasses",
        "GatewayClasses",
        IconName::Globe,
    ),
];

const CONFIG: &[KnownKind] = &[
    k("", "configmaps", "ConfigMaps", IconName::File),
    k("", "secrets", "Secrets", IconName::Key),
    k("", "resourcequotas", "ResourceQuotas", IconName::Gauge),
    k("", "limitranges", "LimitRanges", IconName::SlidersVertical),
    k(
        "autoscaling",
        "horizontalpodautoscalers",
        "HorizontalPodAutoscalers",
        IconName::Activity,
    ),
    k(
        "policy",
        "poddisruptionbudgets",
        "PodDisruptionBudgets",
        IconName::Shield,
    ),
];

const STORAGE: &[KnownKind] = &[
    k(
        "",
        "persistentvolumeclaims",
        "PersistentVolumeClaims",
        IconName::HardDrive,
    ),
    k(
        "",
        "persistentvolumes",
        "PersistentVolumes",
        IconName::HardDrive,
    ),
    k(
        "storage.k8s.io",
        "storageclasses",
        "StorageClasses",
        IconName::Database,
    ),
    k(
        "storage.k8s.io",
        "volumeattachments",
        "VolumeAttachments",
        IconName::Link,
    ),
    k("storage.k8s.io", "csidrivers", "CSIDrivers", IconName::Cpu),
];

const ACCESS: &[KnownKind] = &[
    k("", "serviceaccounts", "ServiceAccounts", IconName::User),
    k(
        "rbac.authorization.k8s.io",
        "roles",
        "Roles",
        IconName::Shield,
    ),
    k(
        "rbac.authorization.k8s.io",
        "rolebindings",
        "RoleBindings",
        IconName::Link,
    ),
    k(
        "rbac.authorization.k8s.io",
        "clusterroles",
        "ClusterRoles",
        IconName::Shield,
    ),
    k(
        "rbac.authorization.k8s.io",
        "clusterrolebindings",
        "ClusterRoleBindings",
        IconName::Link,
    ),
];

const CLUSTER: &[KnownKind] = &[
    k("", "nodes", "Nodes", IconName::Server),
    k("", "namespaces", "Namespaces", IconName::Folder),
    k(
        "apiextensions.k8s.io",
        "customresourcedefinitions",
        "CustomResourceDefinitions",
        IconName::Blocks,
    ),
    k(
        "apiregistration.k8s.io",
        "apiservices",
        "APIServices",
        IconName::Cloud,
    ),
    k(
        "scheduling.k8s.io",
        "priorityclasses",
        "PriorityClasses",
        IconName::ArrowUp,
    ),
    k(
        "node.k8s.io",
        "runtimeclasses",
        "RuntimeClasses",
        IconName::Cpu,
    ),
    k("coordination.k8s.io", "leases", "Leases", IconName::Lock),
    k(
        "admissionregistration.k8s.io",
        "mutatingwebhookconfigurations",
        "MutatingWebhooks",
        IconName::Zap,
    ),
    k(
        "admissionregistration.k8s.io",
        "validatingwebhookconfigurations",
        "ValidatingWebhooks",
        IconName::CircleCheck,
    ),
];

/// Default group order. Ids are what `explorer.group_order` refers to.
pub const GROUPS: &[GroupDef] = &[
    GroupDef {
        id: "overview",
        label: "Overview",
        collapsible: false,
        kinds: &[],
        views: overview_views,
    },
    GroupDef {
        id: "events",
        label: "Events",
        collapsible: false,
        kinds: EVENTS,
        views: no_views,
    },
    GroupDef {
        id: "workloads",
        label: "Workloads",
        collapsible: true,
        kinds: WORKLOADS,
        views: no_views,
    },
    GroupDef {
        id: "network",
        label: "Network",
        collapsible: true,
        kinds: NETWORK,
        views: no_views,
    },
    GroupDef {
        id: "config",
        label: "Config & Secrets",
        collapsible: true,
        kinds: CONFIG,
        views: no_views,
    },
    GroupDef {
        id: "storage",
        label: "Storage",
        collapsible: true,
        kinds: STORAGE,
        views: no_views,
    },
    GroupDef {
        id: "access",
        label: "Access Control",
        collapsible: true,
        kinds: ACCESS,
        views: no_views,
    },
    GroupDef {
        id: "cluster",
        label: "Cluster",
        collapsible: true,
        kinds: CLUSTER,
        views: no_views,
    },
    GroupDef {
        id: "administration",
        label: "Administration",
        collapsible: true,
        kinds: &[],
        views: administration_views,
    },
];

/// Id of the Custom Resources group.
pub const CUSTOM: &str = "custom";

/// Groups served by Kubernetes itself; everything else is a custom resource.
const BUILT_IN_GROUPS: &[&str] = &[
    "",
    "admissionregistration.k8s.io",
    "apiextensions.k8s.io",
    "apiregistration.k8s.io",
    "apps",
    "authentication.k8s.io",
    "authorization.k8s.io",
    "autoscaling",
    "batch",
    "certificates.k8s.io",
    "coordination.k8s.io",
    "discovery.k8s.io",
    "events.k8s.io",
    "flowcontrol.apiserver.k8s.io",
    "internal.apiserver.k8s.io",
    "metrics.k8s.io",
    "networking.k8s.io",
    "node.k8s.io",
    "policy",
    "rbac.authorization.k8s.io",
    "resource.k8s.io",
    "scheduling.k8s.io",
    "storage.k8s.io",
    "storagemigration.k8s.io",
];

/// Groups in the user's order, without hidden ones. `custom` is included (last by default).
pub fn ordered_groups(order: &[String], hidden: &[String]) -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = GROUPS.iter().map(|g| g.id).chain([CUSTOM]).collect();
    ids.sort_by_key(|id| {
        order
            .iter()
            .position(|o| o == id)
            .unwrap_or(order.len() + GROUPS.iter().position(|g| g.id == *id).unwrap_or(99))
    });
    ids.retain(|id| !hidden.iter().any(|h| h == id));
    ids
}

pub fn group(id: &str) -> Option<&'static GroupDef> {
    GROUPS.iter().find(|g| g.id == id)
}

/// A kind the tree shows: resolved against discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeKind {
    pub gvr: Gvr,
    pub kind: String,
    pub label: String,
    pub icon: IconName,
    pub namespaced: bool,
    /// The contributed group ([`TreeGroup::id`]) the row belongs to. Its rows open the kind's
    /// own list view ([`kubyl_core::ViewRegistry::list_view`]) instead of the generic table.
    pub via: Option<&'static str>,
}

fn tree_kind(info: &ApiResourceInfo, label: &str, icon: IconName) -> TreeKind {
    TreeKind {
        gvr: info.gvr.clone(),
        kind: info.gvk.kind.clone(),
        label: label.to_string(),
        icon,
        namespaced: info.namespaced,
        via: None,
    }
}

/// Shows extra text on a contributed group's row for a cluster (a version, a state).
pub type GroupBadge = Arc<dyn Fn(&ClusterId, &App) -> Option<SharedString>>;

/// A group another crate adds below a curated group of the cluster tree, e.g. "Argo CD" under
/// Administration. Its kinds are shown when the cluster serves them (the group disappears with
/// its CRDs) and open their registered list view. They stay under Custom Resources too.
#[derive(Clone)]
pub struct TreeGroup {
    /// Unique id, e.g. `argocd`.
    pub id: &'static str,
    /// The curated group it sits in, e.g. `administration`.
    pub parent: &'static str,
    pub label: &'static str,
    pub kinds: Vec<KnownKind>,
    pub badge: Option<GroupBadge>,
}

/// Groups contributed with [`register_tree_group`].
#[derive(Default)]
pub struct TreeGroups(pub Vec<TreeGroup>);

impl Global for TreeGroups {}

/// Adds a group to the cluster tree (from a feature crate's `init`).
pub fn register_tree_group(cx: &mut App, group: TreeGroup) {
    cx.default_global::<TreeGroups>().0.push(group);
}

/// Re-renders the tree, e.g. after a group's badge changed.
pub fn tree_groups_changed(cx: &mut App) {
    if cx.has_global::<TreeGroups>() {
        cx.update_global::<TreeGroups, _>(|_, _| {});
    }
}

/// The contributed groups below `parent`.
pub fn contributed_groups(parent: &str, cx: &App) -> Vec<TreeGroup> {
    cx.try_global::<TreeGroups>()
        .map(|g| g.0.iter().filter(|g| g.parent == parent).cloned().collect())
        .unwrap_or_default()
}

/// The kinds of a contributed group that the cluster serves.
pub fn contributed_kinds(group: &TreeGroup, discovery: &Discovery) -> Vec<TreeKind> {
    group
        .kinds
        .iter()
        .filter_map(|known| {
            find(discovery, known.group, known.resource).map(|info| TreeKind {
                via: Some(group.id),
                ..tree_kind(info, known.label, known.icon)
            })
        })
        .collect()
}

/// The preferred, listable resource for a group and plural name.
pub fn find<'a>(
    discovery: &'a Discovery,
    group: &str,
    resource: &str,
) -> Option<&'a ApiResourceInfo> {
    discovery
        .preferred()
        .find(|r| r.gvr.group == group && r.gvr.resource == resource && r.is_listable())
}

/// The kinds of a curated group that the cluster serves.
pub fn group_kinds(def: &GroupDef, discovery: &Discovery) -> Vec<TreeKind> {
    def.kinds
        .iter()
        .filter_map(|known| {
            find(discovery, known.group, known.resource)
                .map(|info| tree_kind(info, known.label, known.icon))
        })
        .collect()
}

/// A label for a discovered kind: `CertificateRequests` from kind `CertificateRequest` and
/// plural `certificaterequests`.
pub fn plural_label(kind: &str, plural: &str) -> String {
    // Keep the kind's capitalization for the part it shares with the plural
    // (`NetworkPolic` + `ies`).
    let common = kind
        .bytes()
        .zip(plural.bytes())
        .take_while(|(a, b)| a.is_ascii() && a.to_ascii_lowercase() == *b)
        .count();
    if common > 0 && kind.is_char_boundary(common) && plural.is_char_boundary(common) {
        format!("{}{}", &kind[..common], &plural[common..])
    } else {
        let mut chars = plural.chars();
        chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default()
    }
}

/// Custom resources by API group (sorted), excluding kinds the curated groups already show.
pub fn custom_groups(discovery: &Discovery) -> BTreeMap<String, Vec<TreeKind>> {
    let curated: Vec<(&str, &str)> = GROUPS
        .iter()
        .flat_map(|g| g.kinds.iter().map(|k| (k.group, k.resource)))
        .collect();
    let mut groups: BTreeMap<String, Vec<TreeKind>> = BTreeMap::new();
    for info in discovery.preferred().filter(|r| r.is_listable()) {
        let group = info.gvr.group.as_str();
        if BUILT_IN_GROUPS.contains(&group)
            || curated.contains(&(group, info.gvr.resource.as_str()))
            || info.gvr.resource.contains('/')
        {
            continue;
        }
        let label = plural_label(&info.gvk.kind, &info.gvr.resource);
        groups
            .entry(group.to_string())
            .or_default()
            .push(tree_kind(info, &label, IconName::File));
    }
    for kinds in groups.values_mut() {
        kinds.sort_by(|a, b| a.label.cmp(&b.label));
    }
    groups
}

/// Icon for a kind anywhere in the UI (tabs, details, favorites).
pub fn icon_for(group: &str, resource: &str) -> IconName {
    GROUPS
        .iter()
        .flat_map(|g| g.kinds.iter())
        .find(|k| k.group == group && k.resource == resource)
        .map(|k| k.icon)
        .unwrap_or(IconName::File)
}

/// Display label for a kind (`Pods`, `Certificates`).
pub fn label_for(group: &str, resource: &str, kind: Option<&str>) -> String {
    GROUPS
        .iter()
        .flat_map(|g| g.kinds.iter())
        .find(|k| k.group == group && k.resource == resource)
        .map(|k| k.label.to_string())
        .unwrap_or_else(|| match kind {
            Some(kind) => plural_label(kind, resource),
            None => plural_label("", resource),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_core::Gvk;

    fn resource(group: &str, kind: &str, plural: &str) -> ApiResourceInfo {
        ApiResourceInfo {
            gvk: Gvk::new(group, "v1", kind),
            gvr: Gvr::new(group, "v1", plural),
            singular: kind.to_lowercase(),
            namespaced: true,
            verbs: vec!["list".into(), "watch".into()],
            short_names: vec![],
            categories: vec![],
            subresources: vec![],
            preferred: true,
        }
    }

    #[test]
    fn groups_follow_the_configured_order() {
        let ids = ordered_groups(&["custom".into(), "workloads".into()], &["storage".into()]);
        assert_eq!(&ids[..3], ["custom", "workloads", "overview"]);
        assert!(!ids.contains(&"storage"));
        assert_eq!(ids.len(), GROUPS.len());
    }

    #[test]
    fn custom_resources_are_grouped_by_api_group() {
        let discovery = Discovery {
            groups: vec![],
            resources: vec![
                resource("", "Pod", "pods"),
                resource("cert-manager.io", "Certificate", "certificates"),
                resource(
                    "cert-manager.io",
                    "CertificateRequest",
                    "certificaterequests",
                ),
                resource("cert-manager.io", "Issuer", "issuers"),
                resource("gateway.networking.k8s.io", "Gateway", "gateways"),
                resource("gateway.networking.k8s.io", "GRPCRoute", "grpcroutes"),
            ],
            aggregated: true,
        };
        let groups = custom_groups(&discovery);
        let labels: Vec<_> = groups["cert-manager.io"]
            .iter()
            .map(|k| k.label.as_str())
            .collect();
        assert_eq!(labels, ["CertificateRequests", "Certificates", "Issuers"]);
        // Gateways are curated under Network; the rest of the group stays a custom resource.
        let gateway: Vec<_> = groups["gateway.networking.k8s.io"]
            .iter()
            .map(|k| k.label.as_str())
            .collect();
        assert_eq!(gateway, ["GRPCRoutes"]);
        assert!(!groups.contains_key(""));
        let workloads = group_kinds(group("workloads").unwrap(), &discovery);
        assert_eq!(workloads.len(), 1);
        assert_eq!(workloads[0].label, "Pods");
    }

    #[test]
    fn contributed_groups_show_served_kinds_only() {
        let group = TreeGroup {
            id: "argocd",
            parent: "administration",
            label: "Argo CD",
            kinds: vec![
                k(
                    "argoproj.io",
                    "applications",
                    "Applications",
                    IconName::Layers,
                ),
                k("argoproj.io", "appprojects", "Projects", IconName::Folder),
            ],
            badge: None,
        };
        let mut discovery = Discovery {
            groups: vec![],
            resources: vec![resource("argoproj.io", "Application", "applications")],
            aggregated: true,
        };
        let kinds = contributed_kinds(&group, &discovery);
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].label, "Applications");
        assert_eq!(kinds[0].via, Some("argocd"));
        // Still a custom resource.
        assert!(custom_groups(&discovery).contains_key("argoproj.io"));
        discovery.resources.clear();
        assert!(contributed_kinds(&group, &discovery).is_empty());
    }

    #[test]
    fn labels() {
        assert_eq!(
            plural_label("NetworkPolicy", "networkpolicies"),
            "NetworkPolicies"
        );
        assert_eq!(plural_label("Endpoints", "endpoints"), "Endpoints");
        assert_eq!(label_for("", "pods", None), "Pods");
        assert_eq!(label_for("x.io", "widgets", Some("Widget")), "Widgets");
    }
}
