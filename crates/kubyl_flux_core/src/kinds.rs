//! The Flux kinds Kubyl knows: their API group, plural, which controller reconciles them, and
//! what they support (reconcile requests, suspend). Versions come from discovery: each kind is
//! read at the version the cluster prefers.

use std::fmt;

pub const SOURCE_GROUP: &str = "source.toolkit.fluxcd.io";
pub const KUSTOMIZE_GROUP: &str = "kustomize.toolkit.fluxcd.io";
pub const HELM_GROUP: &str = "helm.toolkit.fluxcd.io";
pub const IMAGE_GROUP: &str = "image.toolkit.fluxcd.io";
pub const NOTIFICATION_GROUP: &str = "notification.toolkit.fluxcd.io";

/// Every Flux API group.
pub const GROUPS: [&str; 5] = [
    SOURCE_GROUP,
    KUSTOMIZE_GROUP,
    HELM_GROUP,
    IMAGE_GROUP,
    NOTIFICATION_GROUP,
];

/// Whether `group` is one of Flux's (`*.toolkit.fluxcd.io`).
pub fn is_flux_group(group: &str) -> bool {
    group.ends_with(".toolkit.fluxcd.io")
}

/// The views a kind is listed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Kustomizations,
    HelmReleases,
    Sources,
    ImageAutomation,
    Notifications,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Kustomizations,
        Category::HelmReleases,
        Category::Sources,
        Category::ImageAutomation,
        Category::Notifications,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Kustomizations => "Kustomizations",
            Category::HelmReleases => "HelmReleases",
            Category::Sources => "Sources",
            Category::ImageAutomation => "Image Automation",
            Category::Notifications => "Notifications",
        }
    }

    pub fn kinds(self) -> Vec<FluxKind> {
        FluxKind::ALL
            .into_iter()
            .filter(|k| k.category() == self)
            .collect()
    }

    /// `ViewKind::Custom` id of the category's list view.
    pub fn view_id(self) -> &'static str {
        match self {
            Category::Kustomizations => "flux_kustomizations",
            Category::HelmReleases => "flux_helmreleases",
            Category::Sources => "flux_sources",
            Category::ImageAutomation => "flux_images",
            Category::Notifications => "flux_notifications",
        }
    }

    pub fn from_view_id(id: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.view_id() == id)
    }
}

/// A Flux kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FluxKind {
    Kustomization,
    HelmRelease,
    GitRepository,
    OCIRepository,
    HelmRepository,
    HelmChart,
    Bucket,
    ExternalArtifact,
    ImageRepository,
    ImagePolicy,
    ImageUpdateAutomation,
    Alert,
    Provider,
    Receiver,
}

impl FluxKind {
    pub const ALL: [FluxKind; 14] = [
        FluxKind::Kustomization,
        FluxKind::HelmRelease,
        FluxKind::GitRepository,
        FluxKind::OCIRepository,
        FluxKind::HelmRepository,
        FluxKind::HelmChart,
        FluxKind::Bucket,
        FluxKind::ExternalArtifact,
        FluxKind::ImageRepository,
        FluxKind::ImagePolicy,
        FluxKind::ImageUpdateAutomation,
        FluxKind::Alert,
        FluxKind::Provider,
        FluxKind::Receiver,
    ];

    /// The kinds a Kustomization's or HelmRelease's `sourceRef` can name.
    pub const SOURCES: [FluxKind; 6] = [
        FluxKind::GitRepository,
        FluxKind::OCIRepository,
        FluxKind::HelmRepository,
        FluxKind::HelmChart,
        FluxKind::Bucket,
        FluxKind::ExternalArtifact,
    ];

    pub fn kind(self) -> &'static str {
        match self {
            FluxKind::Kustomization => "Kustomization",
            FluxKind::HelmRelease => "HelmRelease",
            FluxKind::GitRepository => "GitRepository",
            FluxKind::OCIRepository => "OCIRepository",
            FluxKind::HelmRepository => "HelmRepository",
            FluxKind::HelmChart => "HelmChart",
            FluxKind::Bucket => "Bucket",
            FluxKind::ExternalArtifact => "ExternalArtifact",
            FluxKind::ImageRepository => "ImageRepository",
            FluxKind::ImagePolicy => "ImagePolicy",
            FluxKind::ImageUpdateAutomation => "ImageUpdateAutomation",
            FluxKind::Alert => "Alert",
            FluxKind::Provider => "Provider",
            FluxKind::Receiver => "Receiver",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            FluxKind::Kustomization => KUSTOMIZE_GROUP,
            FluxKind::HelmRelease => HELM_GROUP,
            FluxKind::GitRepository
            | FluxKind::OCIRepository
            | FluxKind::HelmRepository
            | FluxKind::HelmChart
            | FluxKind::Bucket
            | FluxKind::ExternalArtifact => SOURCE_GROUP,
            FluxKind::ImageRepository | FluxKind::ImagePolicy | FluxKind::ImageUpdateAutomation => {
                IMAGE_GROUP
            }
            FluxKind::Alert | FluxKind::Provider | FluxKind::Receiver => NOTIFICATION_GROUP,
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            FluxKind::Kustomization => "kustomizations",
            FluxKind::HelmRelease => "helmreleases",
            FluxKind::GitRepository => "gitrepositories",
            FluxKind::OCIRepository => "ocirepositories",
            FluxKind::HelmRepository => "helmrepositories",
            FluxKind::HelmChart => "helmcharts",
            FluxKind::Bucket => "buckets",
            FluxKind::ExternalArtifact => "externalartifacts",
            FluxKind::ImageRepository => "imagerepositories",
            FluxKind::ImagePolicy => "imagepolicies",
            FluxKind::ImageUpdateAutomation => "imageupdateautomations",
            FluxKind::Alert => "alerts",
            FluxKind::Provider => "providers",
            FluxKind::Receiver => "receivers",
        }
    }

    /// The label of its list (`GitRepositories`).
    pub fn label(self) -> &'static str {
        match self {
            FluxKind::Kustomization => "Kustomizations",
            FluxKind::HelmRelease => "HelmReleases",
            FluxKind::GitRepository => "GitRepositories",
            FluxKind::OCIRepository => "OCIRepositories",
            FluxKind::HelmRepository => "HelmRepositories",
            FluxKind::HelmChart => "HelmCharts",
            FluxKind::Bucket => "Buckets",
            FluxKind::ExternalArtifact => "ExternalArtifacts",
            FluxKind::ImageRepository => "ImageRepositories",
            FluxKind::ImagePolicy => "ImagePolicies",
            FluxKind::ImageUpdateAutomation => "ImageUpdateAutomations",
            FluxKind::Alert => "Alerts",
            FluxKind::Provider => "Providers",
            FluxKind::Receiver => "Receivers",
        }
    }

    /// A short name for compact references (`ks/podinfo`), like the `flux` CLI's.
    pub fn short(self) -> &'static str {
        match self {
            FluxKind::Kustomization => "ks",
            FluxKind::HelmRelease => "hr",
            FluxKind::GitRepository => "gitrepo",
            FluxKind::OCIRepository => "ocirepo",
            FluxKind::HelmRepository => "helmrepo",
            FluxKind::HelmChart => "helmchart",
            FluxKind::Bucket => "bucket",
            FluxKind::ExternalArtifact => "artifact",
            FluxKind::ImageRepository => "imagerepo",
            FluxKind::ImagePolicy => "imagepolicy",
            FluxKind::ImageUpdateAutomation => "imageupdate",
            FluxKind::Alert => "alert",
            FluxKind::Provider => "provider",
            FluxKind::Receiver => "receiver",
        }
    }

    pub fn category(self) -> Category {
        match self {
            FluxKind::Kustomization => Category::Kustomizations,
            FluxKind::HelmRelease => Category::HelmReleases,
            FluxKind::GitRepository
            | FluxKind::OCIRepository
            | FluxKind::HelmRepository
            | FluxKind::HelmChart
            | FluxKind::Bucket
            | FluxKind::ExternalArtifact => Category::Sources,
            FluxKind::ImageRepository | FluxKind::ImagePolicy | FluxKind::ImageUpdateAutomation => {
                Category::ImageAutomation
            }
            FluxKind::Alert | FluxKind::Provider | FluxKind::Receiver => Category::Notifications,
        }
    }

    /// The controller (Deployment in Flux's namespace) that reconciles the kind.
    pub fn controller(self) -> &'static str {
        match self {
            FluxKind::Kustomization => "kustomize-controller",
            FluxKind::HelmRelease => "helm-controller",
            FluxKind::GitRepository
            | FluxKind::OCIRepository
            | FluxKind::HelmRepository
            | FluxKind::HelmChart
            | FluxKind::Bucket
            | FluxKind::ExternalArtifact => "source-controller",
            FluxKind::ImageRepository | FluxKind::ImagePolicy => "image-reflector-controller",
            FluxKind::ImageUpdateAutomation => "image-automation-controller",
            FluxKind::Alert | FluxKind::Provider | FluxKind::Receiver => "notification-controller",
        }
    }

    /// Whether objects of the kind at API `version` (`v1beta3`; empty: unknown) have a status
    /// with a Ready condition. Alerts and Providers from `v1beta3` on are static: the
    /// notification controller reads them when events arrive; at `v1beta1`/`v1beta2` (Flux
    /// 2.0 and 2.1) it still reconciled them. A HelmRepository of `type: oci` is static too,
    /// which only its spec tells ([`crate::model::FluxObject::is_static`]).
    pub fn has_status_at(self, version: &str) -> bool {
        match self {
            FluxKind::Alert | FluxKind::Provider => matches!(version, "v1beta1" | "v1beta2"),
            _ => true,
        }
    }

    /// Whether `reconcile.fluxcd.io/requestedAt` makes the controller act on objects of the kind
    /// at API `version` (what `flux reconcile` supports). Static objects never act on it.
    pub fn reconcilable_at(self, version: &str) -> bool {
        self.has_status_at(version)
            && self != FluxKind::ExternalArtifact
            && !self.is_old_image_policy(version)
    }

    /// Whether the kind has `spec.suspend` at API `version` (empty: unknown).
    pub fn suspendable_at(self, version: &str) -> bool {
        self != FluxKind::ExternalArtifact && !self.is_old_image_policy(version)
    }

    /// ImagePolicies got `spec.suspend` and reconcile requests with `image.toolkit.fluxcd.io/v1`
    /// (Flux 2.7). Kubyl reads each kind at the preferred version, so an ImagePolicy read at
    /// `v1beta1`/`v1beta2` comes from an older image-reflector-controller, which has neither
    /// (patching `spec.suspend` would be pruned silently, a request never handled).
    fn is_old_image_policy(self, version: &str) -> bool {
        self == FluxKind::ImagePolicy && matches!(version, "v1beta1" | "v1beta2")
    }

    /// Whether it's a source (has an artifact others consume).
    pub fn is_source(self) -> bool {
        self.category() == Category::Sources
    }

    pub fn from_kind(kind: &str) -> Option<FluxKind> {
        FluxKind::ALL.into_iter().find(|k| k.kind() == kind)
    }

    pub fn from_group_kind(group: &str, kind: &str) -> Option<FluxKind> {
        FluxKind::from_kind(kind).filter(|k| k.group() == group)
    }

    pub fn from_plural(group: &str, plural: &str) -> Option<FluxKind> {
        FluxKind::ALL
            .into_iter()
            .find(|k| k.group() == group && k.plural() == plural)
    }

    /// The kind of an object (from its `apiVersion` and `kind`).
    pub fn of(object: &serde_json::Value) -> Option<FluxKind> {
        let kind = object.get("kind")?.as_str()?;
        let api_version = object.get("apiVersion")?.as_str()?;
        let group = api_version.split_once('/').map(|(g, _)| g).unwrap_or("");
        FluxKind::from_group_kind(group, kind)
    }
}

impl fmt::Display for FluxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip() {
        for kind in FluxKind::ALL {
            assert_eq!(FluxKind::from_kind(kind.kind()), Some(kind));
            assert_eq!(
                FluxKind::from_plural(kind.group(), kind.plural()),
                Some(kind)
            );
            assert!(is_flux_group(kind.group()));
            assert!(GROUPS.contains(&kind.group()));
            assert!(kind.category().kinds().contains(&kind));
        }
        assert_eq!(
            FluxKind::from_group_kind("kustomize.config.k8s.io", "Kustomization"),
            None
        );
        let object = serde_json::json!({"apiVersion": "helm.toolkit.fluxcd.io/v2beta1", "kind": "HelmRelease"});
        assert_eq!(FluxKind::of(&object), Some(FluxKind::HelmRelease));
        assert!(
            !FluxKind::Alert.reconcilable_at("v1beta3")
                && FluxKind::Alert.suspendable_at("v1beta3")
        );
        // ImagePolicies before image v1 (Flux 2.7) had no suspend and ignored requests.
        assert!(!FluxKind::ImagePolicy.reconcilable_at("v1beta2"));
        assert!(!FluxKind::ImagePolicy.suspendable_at("v1beta2"));
        assert!(
            FluxKind::ImagePolicy.reconcilable_at("v1")
                && FluxKind::ImagePolicy.suspendable_at("v1")
        );
        // Flux 2.0/2.1 notification objects still had a status.
        assert!(FluxKind::Provider.reconcilable_at("v1beta2"));
        assert!(!FluxKind::Provider.has_status_at("v1"));
        assert!(FluxKind::GitRepository.reconcilable_at("v1"));
        assert!(!FluxKind::ExternalArtifact.reconcilable_at("v1"));
        assert!(!FluxKind::ExternalArtifact.suspendable_at("v1"));
        assert_eq!(
            Category::from_view_id("flux_sources"),
            Some(Category::Sources)
        );
    }
}
