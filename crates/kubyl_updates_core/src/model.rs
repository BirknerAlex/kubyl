//! What an update provider reports: the current version, the targets it offers, history, a
//! running update's progress, the control plane and node pools, add-ons. Provider-independent
//! and plain data (the view renders it; pre-flight checks read it).

use jiff::Timestamp;
use kubyl_base::Gvr;
use serde_json::Value;

/// How a cluster is updated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    OpenShift,
    Eks,
    Gke,
    Aks,
    K3s,
    Rke2,
    ClusterApi,
    /// kubeadm, kind, MicroShift, anything Kubyl can't update: read-only.
    SelfManaged,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenShift => "OpenShift",
            ProviderKind::Eks => "Amazon EKS",
            ProviderKind::Gke => "Google GKE",
            ProviderKind::Aks => "Azure AKS",
            ProviderKind::K3s => "k3s",
            ProviderKind::Rke2 => "RKE2",
            ProviderKind::ClusterApi => "Cluster API",
            ProviderKind::SelfManaged => "self-managed",
        }
    }

    /// Cloud providers need credentials outside the kubeconfig and a cargo feature.
    pub fn is_cloud(self) -> bool {
        matches!(
            self,
            ProviderKind::Eks | ProviderKind::Gke | ProviderKind::Aks
        )
    }
}

/// A condition of the cluster's update state (ClusterVersion's `Upgradeable`, `Failing`…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub kind: String,
    /// `None`: `Unknown`.
    pub status: Option<bool>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub since: Option<Timestamp>,
}

impl Condition {
    pub fn is(&self, kind: &str, status: bool) -> bool {
        self.kind == kind && self.status == Some(status)
    }
}

/// How long the current version is supported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Support {
    pub text: String,
    /// Extended or ending support: shown in yellow.
    pub warning: bool,
}

/// The version the cluster runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Current {
    /// `4.17.8`, `1.30`, `v1.33.4+k3s1`.
    pub version: String,
    /// The Kubernetes version when it differs from `version` (OpenShift): `v1.30.6`.
    pub kubernetes: Option<String>,
    /// `eks.12`, the GKE/AKS patch.
    pub platform: Option<String>,
    pub channel: Option<String>,
    /// Channels the provider offers for this version.
    pub channels: Vec<String>,
    pub cluster_id: Option<String>,
    pub support: Option<Support>,
    pub conditions: Vec<Condition>,
}

impl Current {
    pub fn condition(&self, kind: &str) -> Option<&Condition> {
        self.conditions.iter().find(|c| c.kind == kind)
    }
}

/// How a target is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TargetKind {
    /// The one Kubyl suggests: the newest recommended update.
    Recommended,
    /// Offered and recommended.
    Available,
    /// Offered with known risks (OpenShift's conditional updates): accepting them is a step.
    Conditional,
    /// Can't be started now; `Target::blocked` says why.
    Blocked,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            TargetKind::Recommended => "recommended",
            TargetKind::Available => "available",
            TargetKind::Conditional => "conditional",
            TargetKind::Blocked => "blocked",
        }
    }
}

/// A known risk of a conditional update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Risk {
    pub name: String,
    pub message: String,
    pub url: Option<String>,
}

/// A version the cluster can be updated to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub version: String,
    pub kind: TargetKind,
    /// The release image (OpenShift), passed back when starting.
    pub image: Option<String>,
    /// Release notes / errata.
    pub url: Option<String>,
    pub channels: Vec<String>,
    pub risks: Vec<Risk>,
    /// Why it's blocked (or not recommended).
    pub blocked: Vec<String>,
    /// It changes the minor version (a y-stream update on OpenShift).
    pub minor: bool,
}

impl Target {
    pub fn new(version: impl Into<String>, kind: TargetKind) -> Self {
        Self {
            version: version.into(),
            kind,
            image: None,
            url: None,
            channels: Vec::new(),
            risks: Vec::new(),
            blocked: Vec::new(),
            minor: false,
        }
    }

    pub fn startable(&self) -> bool {
        self.kind != TargetKind::Blocked
    }
}

/// One past (or the running) update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub version: String,
    /// `Completed` or `Partial`.
    pub state: String,
    pub started: Option<Timestamp>,
    pub completed: Option<Timestamp>,
    pub verified: bool,
    /// Risks accepted to start it.
    pub accepted_risks: Option<String>,
}

impl HistoryEntry {
    pub fn completed(&self) -> bool {
        self.state == "Completed"
    }
}

/// A running update.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub target: String,
    /// 0–100, when the provider says.
    pub percent: Option<f32>,
    pub message: String,
    pub started: Option<Timestamp>,
    /// The update is stuck: why.
    pub failing: Option<String>,
}

/// A component the update rolls through (an OpenShift ClusterOperator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub version: Option<String>,
    /// Already at the version the cluster updates to (or runs).
    pub updated: bool,
    pub available: Option<bool>,
    pub progressing: Option<bool>,
    pub degraded: Option<bool>,
    pub message: Option<String>,
}

/// What a pool is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolKind {
    ControlPlane,
    NodePool,
    MachineConfigPool,
    /// A system-upgrade-controller Plan.
    Plan,
    MachineDeployment,
    /// Nodes grouped by role (self-managed clusters).
    Nodes,
}

/// A pool's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolState {
    Idle,
    Queued,
    Updating,
    Degraded,
    Paused,
}

/// An object a row or check links to (the view adds the cluster).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectLink {
    pub gvr: Gvr,
    pub namespace: Option<String>,
    pub name: String,
}

impl ObjectLink {
    pub fn new(gvr: Gvr, namespace: Option<&str>, name: impl Into<String>) -> Self {
        Self {
            gvr,
            namespace: namespace.map(str::to_string),
            name: name.into(),
        }
    }
}

/// The control plane, a node pool, a MachineConfigPool, a Plan…
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub id: String,
    pub name: String,
    pub kind: PoolKind,
    pub version: Option<String>,
    /// The version it's moving to.
    pub target_version: Option<String>,
    pub nodes: Option<usize>,
    pub updated: Option<usize>,
    pub ready: Option<usize>,
    pub degraded: usize,
    /// The node being drained or updated now.
    pub draining: Option<String>,
    /// `surge 1 · maxUnavailable 0`.
    pub surge: Option<String>,
    pub state: PoolState,
    pub message: Option<String>,
    /// The provider can update this pool on its own.
    pub updatable: bool,
    pub object: Option<ObjectLink>,
}

impl Pool {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: PoolKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            version: None,
            target_version: None,
            nodes: None,
            updated: None,
            ready: None,
            degraded: 0,
            draining: None,
            surge: None,
            state: PoolState::Idle,
            message: None,
            updatable: false,
            object: None,
        }
    }
}

/// A managed add-on (EKS add-ons, GKE/AKS components).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddOn {
    pub name: String,
    pub version: String,
    /// Whether the installed version supports the target (`None`: unknown).
    pub compatible: Option<bool>,
    /// The version recommended for the target.
    pub recommended: Option<String>,
    pub status: Option<String>,
    pub updatable: bool,
}

/// What a provider can write at all (before the cluster's read-only setting).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Writes {
    /// Start an update of the control plane (OpenShift: the whole cluster).
    pub control_plane: bool,
    pub pools: bool,
    pub channel: bool,
    pub addons: bool,
    /// Why Kubyl can't update this cluster (self-managed, missing credentials…).
    pub reason: Option<String>,
}

impl Writes {
    pub fn any(&self) -> bool {
        self.control_plane || self.pools || self.channel || self.addons
    }

    pub fn none(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

/// A note from the provider shown at the top (e.g. credentials missing, feature not built).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub warning: bool,
    pub title: String,
    pub text: String,
    /// A command that fixes it (`aws sso login --profile prod`), offered to copy.
    pub command: Option<String>,
    pub url: Option<String>,
}

/// Everything a provider reports in one read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// `OpenShift (ClusterVersion)`, `Amazon EKS (AWS API · profile prod · eu-west-1)`.
    pub provider: String,
    pub current: Current,
    /// Newest first.
    pub targets: Vec<Target>,
    /// Newest first.
    pub history: Vec<HistoryEntry>,
    pub progress: Option<Progress>,
    pub components: Vec<Component>,
    pub pools: Vec<Pool>,
    pub addons: Vec<AddOn>,
    pub notes: Vec<Note>,
    pub writes: Writes,
    /// Links for "how to update this cluster" (self-managed).
    pub docs: Vec<(String, String)>,
}

impl Status {
    pub fn target(&self, version: &str) -> Option<&Target> {
        self.targets.iter().find(|t| t.version == version)
    }

    /// The target Kubyl suggests: the recommended one, else the newest startable one.
    pub fn suggested(&self) -> Option<&Target> {
        self.targets
            .iter()
            .find(|t| t.kind == TargetKind::Recommended)
            .or_else(|| self.targets.iter().find(|t| t.startable()))
    }

    pub fn updating(&self) -> bool {
        self.progress.is_some()
    }
}

/// What a write updates.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// The whole cluster (OpenShift) or its control plane.
    ControlPlane,
    /// One pool by id.
    Pool(String),
    /// Every pool that isn't at the target yet (k3s Plans, Cluster API MachineDeployments).
    AllPools,
    AddOn(String),
    /// The update channel.
    Channel(String),
}

/// What a write does, shown before it runs, and what the provider needs to run it.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub scope: Scope,
    /// `Update ocp-prod to 4.17.12`, `Change the channel to stable-4.18`.
    pub title: String,
    pub from: String,
    pub to: String,
    /// `z-stream, recommended`, `minor update`.
    pub kind_label: String,
    /// What changes: `spec.desiredUpdate = 4.17.12 (like oc adm upgrade --to 4.17.12)`.
    pub changes: Vec<String>,
    /// Risks the user accepts one by one.
    pub risks: Vec<Risk>,
    pub notes: Vec<String>,
    /// Whether the write updates something (a channel change doesn't update the cluster).
    pub irreversible: bool,
    /// The provider's own request (object refs, patches).
    pub request: Value,
}
