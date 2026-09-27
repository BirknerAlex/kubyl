//! Kubernetes APIs that were removed, and in which version: the bundled table the removed-API
//! checks match Helm manifests and last-applied configurations against.
//!
//! Source: the Kubernetes deprecation guide
//! (<https://kubernetes.io/docs/reference/using-api/deprecation-guide/>), compiled on
//! 2026-09-26 through v1.32; no later removal was scheduled then. Update the table with every
//! Kubernetes minor release (the test below keeps it sorted and free of duplicates).

/// One removed group/version of a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Removed {
    pub group: &'static str,
    pub version: &'static str,
    pub kind: &'static str,
    /// The plural resource name (for listing objects of the kind).
    pub resource: &'static str,
    /// The Kubernetes minor that stopped serving it.
    pub removed_in: (u64, u64),
    /// What to use instead (`networking.k8s.io/v1`), or why nothing replaces it.
    pub replacement: &'static str,
    /// Objects of the kind are stored (reviews aren't): worth scanning in the cluster.
    pub stored: bool,
}

impl Removed {
    /// `extensions/v1beta1`.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.to_string()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// `1.22`.
    pub fn removed_label(&self) -> String {
        format!("{}.{}", self.removed_in.0, self.removed_in.1)
    }
}

const fn r(
    group: &'static str,
    version: &'static str,
    kind: &'static str,
    resource: &'static str,
    removed_in: (u64, u64),
    replacement: &'static str,
) -> Removed {
    Removed {
        group,
        version,
        kind,
        resource,
        removed_in,
        replacement,
        stored: true,
    }
}

/// A kind that's never stored (reviews).
const fn review(
    group: &'static str,
    version: &'static str,
    kind: &'static str,
    resource: &'static str,
    removed_in: (u64, u64),
    replacement: &'static str,
) -> Removed {
    Removed {
        stored: false,
        ..r(group, version, kind, resource, removed_in, replacement)
    }
}

/// Every removal, by version, group, version and kind.
#[rustfmt::skip]
pub const REMOVED: &[Removed] = &[
    // v1.16
    r("apps", "v1beta1", "Deployment", "deployments", (1, 16), "apps/v1"),
    r("apps", "v1beta1", "ReplicaSet", "replicasets", (1, 16), "apps/v1"),
    r("apps", "v1beta1", "StatefulSet", "statefulsets", (1, 16), "apps/v1"),
    r("apps", "v1beta2", "DaemonSet", "daemonsets", (1, 16), "apps/v1"),
    r("apps", "v1beta2", "Deployment", "deployments", (1, 16), "apps/v1"),
    r("apps", "v1beta2", "ReplicaSet", "replicasets", (1, 16), "apps/v1"),
    r("apps", "v1beta2", "StatefulSet", "statefulsets", (1, 16), "apps/v1"),
    r("extensions", "v1beta1", "DaemonSet", "daemonsets", (1, 16), "apps/v1"),
    r("extensions", "v1beta1", "Deployment", "deployments", (1, 16), "apps/v1"),
    r("extensions", "v1beta1", "NetworkPolicy", "networkpolicies", (1, 16), "networking.k8s.io/v1"),
    r("extensions", "v1beta1", "PodSecurityPolicy", "podsecuritypolicies", (1, 16), "policy/v1beta1 (itself removed in 1.25)"),
    r("extensions", "v1beta1", "ReplicaSet", "replicasets", (1, 16), "apps/v1"),
    // v1.22
    r("admissionregistration.k8s.io", "v1beta1", "MutatingWebhookConfiguration", "mutatingwebhookconfigurations", (1, 22), "admissionregistration.k8s.io/v1"),
    r("admissionregistration.k8s.io", "v1beta1", "ValidatingWebhookConfiguration", "validatingwebhookconfigurations", (1, 22), "admissionregistration.k8s.io/v1"),
    r("apiextensions.k8s.io", "v1beta1", "CustomResourceDefinition", "customresourcedefinitions", (1, 22), "apiextensions.k8s.io/v1"),
    r("apiregistration.k8s.io", "v1beta1", "APIService", "apiservices", (1, 22), "apiregistration.k8s.io/v1"),
    review("authentication.k8s.io", "v1beta1", "TokenReview", "tokenreviews", (1, 22), "authentication.k8s.io/v1"),
    review("authorization.k8s.io", "v1beta1", "LocalSubjectAccessReview", "localsubjectaccessreviews", (1, 22), "authorization.k8s.io/v1"),
    review("authorization.k8s.io", "v1beta1", "SelfSubjectAccessReview", "selfsubjectaccessreviews", (1, 22), "authorization.k8s.io/v1"),
    review("authorization.k8s.io", "v1beta1", "SelfSubjectRulesReview", "selfsubjectrulesreviews", (1, 22), "authorization.k8s.io/v1"),
    review("authorization.k8s.io", "v1beta1", "SubjectAccessReview", "subjectaccessreviews", (1, 22), "authorization.k8s.io/v1"),
    r("certificates.k8s.io", "v1beta1", "CertificateSigningRequest", "certificatesigningrequests", (1, 22), "certificates.k8s.io/v1"),
    r("coordination.k8s.io", "v1beta1", "Lease", "leases", (1, 22), "coordination.k8s.io/v1"),
    r("extensions", "v1beta1", "Ingress", "ingresses", (1, 22), "networking.k8s.io/v1"),
    r("networking.k8s.io", "v1beta1", "Ingress", "ingresses", (1, 22), "networking.k8s.io/v1"),
    r("networking.k8s.io", "v1beta1", "IngressClass", "ingressclasses", (1, 22), "networking.k8s.io/v1"),
    r("rbac.authorization.k8s.io", "v1beta1", "ClusterRole", "clusterroles", (1, 22), "rbac.authorization.k8s.io/v1"),
    r("rbac.authorization.k8s.io", "v1beta1", "ClusterRoleBinding", "clusterrolebindings", (1, 22), "rbac.authorization.k8s.io/v1"),
    r("rbac.authorization.k8s.io", "v1beta1", "Role", "roles", (1, 22), "rbac.authorization.k8s.io/v1"),
    r("rbac.authorization.k8s.io", "v1beta1", "RoleBinding", "rolebindings", (1, 22), "rbac.authorization.k8s.io/v1"),
    r("scheduling.k8s.io", "v1beta1", "PriorityClass", "priorityclasses", (1, 22), "scheduling.k8s.io/v1"),
    r("storage.k8s.io", "v1beta1", "CSIDriver", "csidrivers", (1, 22), "storage.k8s.io/v1"),
    r("storage.k8s.io", "v1beta1", "CSINode", "csinodes", (1, 22), "storage.k8s.io/v1"),
    r("storage.k8s.io", "v1beta1", "StorageClass", "storageclasses", (1, 22), "storage.k8s.io/v1"),
    r("storage.k8s.io", "v1beta1", "VolumeAttachment", "volumeattachments", (1, 22), "storage.k8s.io/v1"),
    // v1.25
    r("autoscaling", "v2beta1", "HorizontalPodAutoscaler", "horizontalpodautoscalers", (1, 25), "autoscaling/v2"),
    r("batch", "v1beta1", "CronJob", "cronjobs", (1, 25), "batch/v1"),
    r("discovery.k8s.io", "v1beta1", "EndpointSlice", "endpointslices", (1, 25), "discovery.k8s.io/v1"),
    r("events.k8s.io", "v1beta1", "Event", "events", (1, 25), "events.k8s.io/v1"),
    r("node.k8s.io", "v1beta1", "RuntimeClass", "runtimeclasses", (1, 25), "node.k8s.io/v1"),
    r("policy", "v1beta1", "PodDisruptionBudget", "poddisruptionbudgets", (1, 25), "policy/v1"),
    r("policy", "v1beta1", "PodSecurityPolicy", "podsecuritypolicies", (1, 25), "none: Pod Security Admission replaces it"),
    // v1.26
    r("autoscaling", "v2beta2", "HorizontalPodAutoscaler", "horizontalpodautoscalers", (1, 26), "autoscaling/v2"),
    r("flowcontrol.apiserver.k8s.io", "v1beta1", "FlowSchema", "flowschemas", (1, 26), "flowcontrol.apiserver.k8s.io/v1"),
    r("flowcontrol.apiserver.k8s.io", "v1beta1", "PriorityLevelConfiguration", "prioritylevelconfigurations", (1, 26), "flowcontrol.apiserver.k8s.io/v1"),
    // v1.27
    r("storage.k8s.io", "v1beta1", "CSIStorageCapacity", "csistoragecapacities", (1, 27), "storage.k8s.io/v1"),
    // v1.29
    r("flowcontrol.apiserver.k8s.io", "v1beta2", "FlowSchema", "flowschemas", (1, 29), "flowcontrol.apiserver.k8s.io/v1"),
    r("flowcontrol.apiserver.k8s.io", "v1beta2", "PriorityLevelConfiguration", "prioritylevelconfigurations", (1, 29), "flowcontrol.apiserver.k8s.io/v1"),
    // v1.32
    r("flowcontrol.apiserver.k8s.io", "v1beta3", "FlowSchema", "flowschemas", (1, 32), "flowcontrol.apiserver.k8s.io/v1"),
    r("flowcontrol.apiserver.k8s.io", "v1beta3", "PriorityLevelConfiguration", "prioritylevelconfigurations", (1, 32), "flowcontrol.apiserver.k8s.io/v1"),
];

/// The removal of `apiVersion` + `kind`, if it was removed.
pub fn lookup(api_version: &str, kind: &str) -> Option<&'static Removed> {
    let (group, version) = api_version.split_once('/').unwrap_or(("", api_version));
    REMOVED
        .iter()
        .find(|r| r.group == group && r.version == version && r.kind == kind)
}

/// The removal of a group, version and plural resource (what API server metrics report).
pub fn lookup_resource(group: &str, version: &str, resource: &str) -> Option<&'static Removed> {
    REMOVED
        .iter()
        .find(|r| r.group == group && r.version == version && r.resource == resource)
}

/// Removals after `from` up to and including `to` (minor keys): what an update from `from`
/// to `to` stops serving.
pub fn removed_between(from: (u64, u64), to: (u64, u64)) -> impl Iterator<Item = &'static Removed> {
    REMOVED
        .iter()
        .filter(move |r| r.removed_in > from && r.removed_in <= to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_unique() {
        let keys: Vec<_> = REMOVED
            .iter()
            .map(|r| (r.removed_in, r.group, r.version, r.kind))
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            keys, sorted,
            "keep REMOVED sorted by version, group, version, kind"
        );
        for r in REMOVED {
            assert!(r.resource.chars().all(|c| c.is_ascii_lowercase()));
            assert!(!r.replacement.is_empty());
        }
    }

    #[test]
    fn lookups() {
        let ingress = lookup("extensions/v1beta1", "Ingress").unwrap();
        assert_eq!(ingress.removed_label(), "1.22");
        assert_eq!(ingress.replacement, "networking.k8s.io/v1");
        assert!(lookup("networking.k8s.io/v1", "Ingress").is_none());
        assert!(lookup("v1", "Pod").is_none());
        assert_eq!(
            lookup_resource("flowcontrol.apiserver.k8s.io", "v1beta3", "flowschemas")
                .unwrap()
                .removed_in,
            (1, 32)
        );
        let between: Vec<_> = removed_between((1, 31), (1, 32)).map(|r| r.kind).collect();
        assert_eq!(between, ["FlowSchema", "PriorityLevelConfiguration"]);
        assert_eq!(removed_between((1, 32), (1, 38)).count(), 0);
    }
}
