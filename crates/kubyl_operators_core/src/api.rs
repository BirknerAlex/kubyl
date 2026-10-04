//! What other crates read about installed operators.

use crate::olm::join::OperatorStatus;

/// One installed operator (OLM v0 CSV), with its version constraints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledOperator {
    /// The display name (`Strimzi`).
    pub name: String,
    /// The package it's subscribed to, if any.
    pub package: Option<String>,
    pub namespace: String,
    /// The ClusterServiceVersion.
    pub csv: String,
    pub version: Option<String>,
    pub channel: Option<String>,
    /// `spec.minKubeVersion` (OLM refuses to install below it).
    pub min_kube_version: Option<String>,
    /// An upper Kubernetes version the author declared (an `olm.maxKubeVersion` property or
    /// operatorhub.io's `operatorhub.io/ui-metadata-max-k8s-version`). Not an OLM field and
    /// not enforced: advisory.
    pub max_kube_version: Option<String>,
    /// The `olm.maxOpenShiftVersion` property: OpenShift blocks minor upgrades past it.
    pub max_openshift_version: Option<String>,
    pub status: OperatorStatus,
}

/// The answer of `kubyl_operators::api::installed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    NotConnected,
    /// The watches haven't listed yet: observe `kubyl_operators::Olm::global` and ask again.
    Loading,
    /// The cluster doesn't serve OLM.
    NoOlm,
    Ready(Vec<InstalledOperator>),
    /// OLM is there but can't be read (403…).
    Problem(String),
}
