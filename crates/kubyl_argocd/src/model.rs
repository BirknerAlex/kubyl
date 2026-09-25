//! Argo CD objects as Kubyl reads them: `Application`, `ApplicationSet` and `AppProject`
//! (`argoproj.io/v1alpha1`), parsed leniently from the watch caches' JSON. Unknown fields are
//! ignored and missing ones default, so objects from older and newer Argo CD releases load.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The API group of every Argo CD kind.
pub const GROUP: &str = "argoproj.io";
/// `metadata.annotations` key that asks the controller to refresh an app.
pub const REFRESH_ANNOTATION: &str = "argocd.argoproj.io/refresh";
/// Deletes an app's resources before the app (foreground cascade).
pub const FINALIZER: &str = "resources-finalizer.argocd.argoproj.io";
/// The background-cascade variant of [`FINALIZER`].
pub const FINALIZER_BACKGROUND: &str = "resources-finalizer.argocd.argoproj.io/background";
/// Annotation-based resource tracking (Argo CD's default since 3.0).
pub const TRACKING_ANNOTATION: &str = "argocd.argoproj.io/tracking-id";
/// Label-based resource tracking (the pre-3.0 default).
pub const TRACKING_LABEL: &str = "app.kubernetes.io/instance";
/// The in-cluster destination server.
pub const IN_CLUSTER: &str = "https://kubernetes.default.svc";

// ----- Status values -----

/// Sync status of an app or a resource.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SyncStatus {
    Synced,
    OutOfSync,
    #[default]
    Unknown,
}

impl SyncStatus {
    pub const ALL: [SyncStatus; 3] = [
        SyncStatus::Synced,
        SyncStatus::OutOfSync,
        SyncStatus::Unknown,
    ];

    pub fn parse(value: &str) -> Self {
        match value {
            "Synced" => SyncStatus::Synced,
            "OutOfSync" => SyncStatus::OutOfSync,
            _ => SyncStatus::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SyncStatus::Synced => "Synced",
            SyncStatus::OutOfSync => "OutOfSync",
            SyncStatus::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Health of an app or a resource, Argo CD's six states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Health {
    Healthy,
    Progressing,
    Degraded,
    Suspended,
    Missing,
    #[default]
    Unknown,
}

impl Health {
    pub const ALL: [Health; 6] = [
        Health::Healthy,
        Health::Progressing,
        Health::Degraded,
        Health::Suspended,
        Health::Missing,
        Health::Unknown,
    ];

    pub fn parse(value: &str) -> Self {
        match value {
            "Healthy" => Health::Healthy,
            "Progressing" => Health::Progressing,
            "Degraded" => Health::Degraded,
            "Suspended" => Health::Suspended,
            "Missing" => Health::Missing,
            _ => Health::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Health::Healthy => "Healthy",
            Health::Progressing => "Progressing",
            Health::Degraded => "Degraded",
            Health::Suspended => "Suspended",
            Health::Missing => "Missing",
            Health::Unknown => "Unknown",
        }
    }

    /// Worse first, for aggregating child health (Argo CD's order).
    pub fn severity(self) -> u8 {
        match self {
            Health::Healthy => 0,
            Health::Suspended => 1,
            Health::Progressing => 2,
            Health::Missing => 3,
            Health::Degraded => 4,
            Health::Unknown => 5,
        }
    }
}

impl fmt::Display for Health {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Phase of the current or last operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationPhase {
    Running,
    Terminating,
    Failed,
    Error,
    Succeeded,
}

impl OperationPhase {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "Running" => OperationPhase::Running,
            "Terminating" => OperationPhase::Terminating,
            "Failed" => OperationPhase::Failed,
            "Error" => OperationPhase::Error,
            "Succeeded" => OperationPhase::Succeeded,
            _ => return None,
        })
    }

    pub fn is_completed(self) -> bool {
        matches!(
            self,
            OperationPhase::Failed | OperationPhase::Error | OperationPhase::Succeeded
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            OperationPhase::Running => "Running",
            OperationPhase::Terminating => "Terminating",
            OperationPhase::Failed => "Failed",
            OperationPhase::Error => "Error",
            OperationPhase::Succeeded => "Succeeded",
        }
    }
}

// ----- Application -----

/// `metadata` fields Kubyl uses.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Meta {
    pub name: String,
    pub namespace: Option<String>,
    pub uid: String,
    pub resource_version: String,
    pub creation_timestamp: Option<String>,
    pub deletion_timestamp: Option<String>,
    pub labels: std::collections::BTreeMap<String, String>,
    pub annotations: std::collections::BTreeMap<String, String>,
    pub finalizers: Vec<String>,
    pub owner_references: Vec<OwnerReference>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OwnerReference {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub uid: String,
}

/// A source of manifests: a Git path, a Helm chart, or a `ref` for multi-source values.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Source {
    #[serde(rename = "repoURL")]
    pub repo_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart: Option<String>,
    /// Multi-source apps: a source referenced by others' value files (`$values/…`).
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool-specific settings (helm, kustomize, directory, plugin), kept as they are.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helm: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kustomize: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<Value>,
}

impl Source {
    /// `HEAD` when no revision is set, like Argo CD.
    pub fn target(&self) -> &str {
        self.target_revision
            .as_deref()
            .filter(|r| !r.is_empty())
            .unwrap_or("HEAD")
    }

    /// `apps/checkout-api`, `ingress-nginx 4.12.1` (a chart), or the repository for a `ref`.
    pub fn what(&self) -> String {
        match (&self.chart, &self.path) {
            (Some(chart), _) => format!("{chart} {}", self.target()),
            (None, Some(path)) if !path.is_empty() => path.clone(),
            _ if self.reference.is_some() => {
                format!("${}", self.reference.as_deref().unwrap_or(""))
            }
            _ => ".".into(),
        }
    }

    /// `github.com/acme/payments-deploy` (scheme, credentials and `.git` dropped).
    pub fn repo_short(&self) -> String {
        short_repo(&self.repo_url)
    }

    /// The source's tool, from its settings (Argo CD reports the detected one in the status).
    pub fn is_helm(&self) -> bool {
        self.chart.is_some() || self.helm.is_some()
    }
}

/// `github.com/acme/deploy` from `https://user@github.com/acme/deploy.git` or
/// `git@github.com:acme/deploy.git`.
pub fn short_repo(url: &str) -> String {
    let mut rest = url.trim();
    for scheme in ["https://", "http://", "ssh://", "git://", "oci://"] {
        if let Some(stripped) = rest.strip_prefix(scheme) {
            rest = stripped;
            break;
        }
    }
    // Credentials never show (`user:token@host`).
    if let Some((_, host)) = rest.split_once('@') {
        rest = host;
    }
    let mut out = rest.replacen(':', "/", usize::from(!url.contains("://")));
    if let Some(stripped) = out.strip_suffix(".git") {
        out = stripped.to_string();
    }
    out.trim_end_matches('/').to_string()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Destination {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

impl Destination {
    /// The cluster Argo CD itself runs in.
    pub fn is_in_cluster(&self) -> bool {
        self.server.as_deref().map(|s| s.trim_end_matches('/')) == Some(IN_CLUSTER)
            || self.name.as_deref() == Some("in-cluster")
    }

    /// `in-cluster`, the cluster name, or the server URL.
    pub fn cluster_label(&self) -> String {
        if self.is_in_cluster() {
            return "in-cluster".into();
        }
        self.name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| {
                self.server
                    .as_deref()
                    .map(|s| s.trim_start_matches("https://").to_string())
            })
            .unwrap_or_else(|| "—".into())
    }

    /// `in-cluster · payments`.
    pub fn label(&self) -> String {
        match self.namespace.as_deref().filter(|n| !n.is_empty()) {
            Some(ns) => format!("{} · {ns}", self.cluster_label()),
            None => self.cluster_label(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Automated {
    pub prune: bool,
    pub self_heal: bool,
    pub allow_empty: bool,
    /// Argo CD 3.1+: `automated: {enabled: false}` keeps the settings but turns auto-sync off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncPolicy {
    pub automated: Option<Automated>,
    pub sync_options: Vec<String>,
    pub retry: Option<Value>,
}

impl SyncPolicy {
    /// Like Argo CD's `IsAutomatedSyncEnabled`.
    pub fn auto_sync(&self) -> bool {
        self.automated
            .as_ref()
            .is_some_and(|a| a.enabled.unwrap_or(true))
    }

    pub fn prune(&self) -> bool {
        self.auto_sync() && self.automated.as_ref().is_some_and(|a| a.prune)
    }

    pub fn self_heal(&self) -> bool {
        self.auto_sync() && self.automated.as_ref().is_some_and(|a| a.self_heal)
    }

    /// `retry 5 · 5s×2`.
    pub fn retry_label(&self) -> Option<String> {
        let retry = self.retry.as_ref()?;
        let limit = retry.get("limit").and_then(Value::as_i64);
        let backoff = retry.get("backoff");
        let duration = backoff
            .and_then(|b| b.get("duration"))
            .and_then(Value::as_str);
        let factor = backoff
            .and_then(|b| b.get("factor"))
            .and_then(Value::as_i64);
        let mut out = match limit {
            Some(limit) if limit < 0 => "retry forever".to_string(),
            Some(limit) => format!("retry {limit}"),
            None => "retry".to_string(),
        };
        match (duration, factor) {
            (Some(d), Some(f)) => out.push_str(&format!(" · {d}×{f}")),
            (Some(d), None) => out.push_str(&format!(" · {d}")),
            _ => {}
        }
        Some(out)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSpec {
    pub project: String,
    pub source: Option<Source>,
    pub sources: Vec<Source>,
    pub destination: Destination,
    pub sync_policy: Option<SyncPolicy>,
    pub revision_history_limit: Option<i64>,
}

impl AppSpec {
    /// Every source: `sources` for multi-source apps, else `source`.
    pub fn all_sources(&self) -> Vec<Source> {
        if !self.sources.is_empty() {
            self.sources.clone()
        } else {
            self.source.clone().into_iter().collect()
        }
    }

    pub fn is_multi_source(&self) -> bool {
        !self.sources.is_empty()
    }

    pub fn policy(&self) -> SyncPolicy {
        self.sync_policy.clone().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Initiator {
    pub username: Option<String>,
    pub automated: bool,
}

impl Initiator {
    /// `alice`, or `automated`.
    pub fn label(&self) -> String {
        match &self.username {
            Some(user) if !user.is_empty() => user.clone(),
            _ if self.automated => "automated".into(),
            _ => "unknown".into(),
        }
    }
}

/// One entry of `status.history`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub revision: String,
    pub revisions: Vec<String>,
    pub source: Option<Source>,
    pub sources: Vec<Source>,
    pub deployed_at: Option<String>,
    pub deploy_started_at: Option<String>,
    pub initiated_by: Option<Initiator>,
}

impl HistoryEntry {
    /// The sources it deployed (one for single-source apps).
    pub fn all_sources(&self) -> Vec<Source> {
        if !self.sources.is_empty() {
            self.sources.clone()
        } else {
            self.source
                .clone()
                .filter(|s| !s.repo_url.is_empty())
                .into_iter()
                .collect()
        }
    }

    /// Revisions per source (one for single-source apps).
    pub fn all_revisions(&self) -> Vec<String> {
        if !self.revisions.is_empty() {
            self.revisions.clone()
        } else if self.revision.is_empty() {
            Vec::new()
        } else {
            vec![self.revision.clone()]
        }
    }

    /// Argo CD refuses to roll back to entries without their source (deployed by 0.11).
    pub fn can_roll_back(&self) -> bool {
        !self.all_sources().is_empty()
    }

    pub fn deployed(&self) -> Option<Timestamp> {
        self.deployed_at.as_deref().and_then(|t| t.parse().ok())
    }
}

/// A resource the app manages (`status.resources`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ManagedResource {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub status: Option<String>,
    /// Argo CD 3.x no longer persists resource health here by default
    /// (`controller.resource.health.persist`); Kubyl then derives it from live objects.
    pub health: Option<HealthStatus>,
    pub hook: bool,
    pub requires_pruning: bool,
    pub sync_wave: Option<i64>,
}

impl ManagedResource {
    pub fn sync(&self) -> SyncStatus {
        SyncStatus::parse(self.status.as_deref().unwrap_or_default())
    }

    /// `apps/v1`, or `v1` for the core group.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// Identity for matching live objects and sync results.
    pub fn key(&self) -> ResourceKey {
        ResourceKey {
            group: self.group.clone(),
            kind: self.kind.clone(),
            namespace: self.namespace.clone(),
            name: self.name.clone(),
        }
    }
}

/// `group/kind/namespace/name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceKey {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HealthStatus {
    pub status: String,
    pub message: Option<String>,
}

impl HealthStatus {
    pub fn health(&self) -> Health {
        Health::parse(&self.status)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Condition {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
    pub last_transition_time: Option<String>,
}

impl Condition {
    /// Errors (`ComparisonError`, `SyncError`, `InvalidSpecError`…) as opposed to warnings.
    pub fn is_error(&self) -> bool {
        self.kind.ends_with("Error")
    }
}

/// One resource of a sync result.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceResult {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    /// `Synced`, `SyncFailed`, `Pruned`, `PruneSkipped`.
    pub status: Option<String>,
    pub message: Option<String>,
    pub hook_type: Option<String>,
    pub hook_phase: Option<String>,
    pub sync_phase: Option<String>,
}

impl ResourceResult {
    pub fn failed(&self) -> bool {
        self.status.as_deref() == Some("SyncFailed")
            || matches!(self.hook_phase.as_deref(), Some("Failed" | "Error"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncResult {
    pub resources: Vec<ResourceResult>,
    pub revision: String,
    pub revisions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OperationState {
    pub phase: String,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub operation: Option<Value>,
    pub sync_result: Option<SyncResult>,
    pub retry_count: i64,
}

impl OperationState {
    pub fn phase(&self) -> Option<OperationPhase> {
        OperationPhase::parse(&self.phase)
    }

    /// Who started it.
    pub fn initiator(&self) -> Option<Initiator> {
        let value = self.operation.as_ref()?.get("initiatedBy")?.clone();
        serde_json::from_value(value).ok()
    }

    pub fn finished(&self) -> Option<Timestamp> {
        self.finished_at.as_deref().and_then(|t| t.parse().ok())
    }

    pub fn started(&self) -> Option<Timestamp> {
        self.started_at.as_deref().and_then(|t| t.parse().ok())
    }

    /// The sync revision it deployed.
    pub fn revision(&self) -> Option<String> {
        let result = self.sync_result.as_ref()?;
        if !result.revision.is_empty() {
            Some(result.revision.clone())
        } else {
            result.revisions.first().cloned()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncInfo {
    pub status: String,
    pub revision: String,
    pub revisions: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Summary {
    pub images: Vec<String>,
    #[serde(rename = "externalURLs")]
    pub external_urls: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppStatus {
    pub sync: SyncInfo,
    pub health: HealthStatus,
    pub history: Vec<HistoryEntry>,
    pub operation_state: Option<OperationState>,
    pub resources: Vec<ManagedResource>,
    pub conditions: Vec<Condition>,
    pub summary: Summary,
    pub reconciled_at: Option<String>,
    pub source_type: Option<String>,
    pub source_types: Vec<String>,
    pub controller_namespace: Option<String>,
}

/// An `Application`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Application {
    pub metadata: Meta,
    pub spec: AppSpec,
    pub status: AppStatus,
    /// A requested operation the controller hasn't finished yet.
    pub operation: Option<Value>,
}

/// What the app is doing right now, for the progress chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    /// `operation` is set but the controller hasn't picked it up.
    Requested,
    /// Running, with resources done so far and the total (0 when unknown).
    Syncing {
        done: usize,
        total: usize,
    },
    Terminating,
    /// Being deleted (its finalizer deletes the resources first).
    Deleting,
    /// A refresh was requested (`argocd.argoproj.io/refresh`).
    Refreshing,
}

impl Activity {
    /// `Syncing 12/40`, `Terminating…`.
    pub fn label(&self) -> String {
        match self {
            Activity::Requested => "Sync requested".into(),
            Activity::Syncing { done, total } if *total > 0 => {
                format!("Syncing {}/{total}", (*done).min(*total))
            }
            Activity::Syncing { .. } => "Syncing…".into(),
            Activity::Terminating => "Terminating…".into(),
            Activity::Deleting => "Deleting…".into(),
            Activity::Refreshing => "Refreshing…".into(),
        }
    }
}

impl Application {
    pub fn parse(object: &Value) -> Option<Self> {
        serde_json::from_value(object.clone()).ok()
    }

    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    pub fn namespace(&self) -> &str {
        self.metadata.namespace.as_deref().unwrap_or_default()
    }

    /// `argocd/guestbook`.
    pub fn qualified_name(&self) -> String {
        format!("{}/{}", self.namespace(), self.name())
    }

    pub fn sync(&self) -> SyncStatus {
        SyncStatus::parse(&self.status.sync.status)
    }

    pub fn health(&self) -> Health {
        self.status.health.health()
    }

    pub fn policy(&self) -> SyncPolicy {
        self.spec.policy()
    }

    /// The synced revision(s): `3f9c2a1`, or one per source.
    pub fn synced_revisions(&self) -> Vec<String> {
        if !self.status.sync.revisions.is_empty() {
            self.status.sync.revisions.clone()
        } else if self.status.sync.revision.is_empty() {
            Vec::new()
        } else {
            vec![self.status.sync.revision.clone()]
        }
    }

    /// History, newest first.
    pub fn history_newest_first(&self) -> Vec<HistoryEntry> {
        let mut history = self.status.history.clone();
        history.sort_by_key(|h| std::cmp::Reverse(h.id));
        history
    }

    /// The entry of the running deployment: the newest one.
    pub fn current_history_id(&self) -> Option<i64> {
        self.status.history.iter().map(|h| h.id).max()
    }

    pub fn is_deleting(&self) -> bool {
        self.metadata.deletion_timestamp.is_some()
    }

    /// Deleting it also deletes its resources (a resources finalizer is set).
    pub fn cascades(&self) -> bool {
        self.metadata
            .finalizers
            .iter()
            .any(|f| f == FINALIZER || f.starts_with(&format!("{FINALIZER}/")))
    }

    /// Resource results of the running (or last) operation.
    pub fn operation_state(&self) -> Option<&OperationState> {
        self.status.operation_state.as_ref()
    }

    /// Whether an operation is requested or running.
    pub fn operation_in_progress(&self) -> bool {
        self.operation.is_some()
            || self
                .operation_state()
                .and_then(OperationState::phase)
                .is_some_and(|p| !p.is_completed())
    }

    pub fn activity(&self) -> Option<Activity> {
        if self.is_deleting() {
            return Some(Activity::Deleting);
        }
        let state = self.operation_state();
        match state.and_then(OperationState::phase) {
            Some(OperationPhase::Terminating) => return Some(Activity::Terminating),
            Some(OperationPhase::Running) => {
                let done = state
                    .and_then(|s| s.sync_result.as_ref())
                    .map(|r| r.resources.len())
                    .unwrap_or(0);
                let total = self.status.resources.len().max(done);
                return Some(Activity::Syncing { done, total });
            }
            _ => {}
        }
        if self.operation.is_some() {
            return Some(Activity::Requested);
        }
        if self.metadata.annotations.contains_key(REFRESH_ANNOTATION) {
            return Some(Activity::Refreshing);
        }
        None
    }

    /// The last finished operation: its phase and when it finished.
    pub fn last_result(&self) -> Option<(OperationPhase, Option<Timestamp>)> {
        let state = self.operation_state()?;
        let phase = state.phase()?;
        phase.is_completed().then(|| (phase, state.finished()))
    }

    /// Resources that aren't in sync.
    pub fn out_of_sync(&self) -> Vec<&ManagedResource> {
        self.status
            .resources
            .iter()
            .filter(|r| r.sync() == SyncStatus::OutOfSync)
            .collect()
    }

    /// Owned by an ApplicationSet.
    pub fn application_set(&self) -> Option<&str> {
        self.metadata
            .owner_references
            .iter()
            .find(|o| o.kind == "ApplicationSet" && o.api_version.starts_with(GROUP))
            .map(|o| o.name.as_str())
    }
}

/// Short form of a revision: 7 characters of a Git SHA, anything else as it is.
pub fn short_revision(revision: &str) -> String {
    if is_sha(revision) {
        revision[..7].to_string()
    } else {
        revision.to_string()
    }
}

/// A Git commit SHA (40 hex characters, or an abbreviation of at least 7).
pub fn is_sha(revision: &str) -> bool {
    (7..=64).contains(&revision.len()) && revision.bytes().all(|b| b.is_ascii_hexdigit())
}

// ----- ApplicationSet -----

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSetCondition {
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub message: String,
    pub reason: String,
}

impl AppSetCondition {
    /// Conditions that say something is wrong (`ErrorOccurred: True`, `ParametersGenerated:
    /// False`, `ResourcesUpToDate: False`).
    pub fn is_problem(&self) -> bool {
        match self.kind.as_str() {
            "ErrorOccurred" => self.status == "True",
            _ => self.status == "False",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSetSpec {
    pub generators: Vec<Value>,
    pub sync_policy: Option<Value>,
    pub go_template: bool,
    pub strategy: Option<Value>,
    pub template: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSetStatus {
    pub conditions: Vec<AppSetCondition>,
    pub resources: Vec<Value>,
}

/// An `ApplicationSet`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ApplicationSet {
    pub metadata: Meta,
    pub spec: AppSetSpec,
    pub status: AppSetStatus,
}

impl ApplicationSet {
    pub fn parse(object: &Value) -> Option<Self> {
        serde_json::from_value(object.clone()).ok()
    }

    /// `list (2) + git directories` …
    pub fn generators_summary(&self) -> String {
        let parts: Vec<String> = self.spec.generators.iter().map(generator_summary).collect();
        if parts.is_empty() {
            "no generators".into()
        } else {
            parts.join(" + ")
        }
    }

    /// `create-update`, `sync`… plus "keeps resources" when deletion preserves them.
    pub fn policy_summary(&self) -> String {
        let Some(policy) = &self.spec.sync_policy else {
            return "create-delete (default)".into();
        };
        let mut out = policy
            .get("applicationsSync")
            .and_then(Value::as_str)
            .unwrap_or("sync")
            .to_string();
        if policy
            .get("preserveResourcesOnDeletion")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            out.push_str(" · keeps resources on deletion");
        }
        out
    }

    /// Progressive sync (`strategy.type: RollingSync`).
    pub fn rolling_sync(&self) -> bool {
        self.spec
            .strategy
            .as_ref()
            .and_then(|s| s.get("type"))
            .and_then(Value::as_str)
            == Some("RollingSync")
    }
}

/// One generator in a few words: `list (2)`, `clusters (env=prod)`, `git directories
/// apps/*`, `matrix (git × clusters)`, `pull requests (github acme/deploy)`.
pub fn generator_summary(generator: &Value) -> String {
    let Some(map) = generator.as_object() else {
        return "?".into();
    };
    let Some((kind, body)) = map.iter().find(|(k, _)| *k != "selector") else {
        return "?".into();
    };
    match kind.as_str() {
        "list" => {
            let n = body
                .get("elements")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("list ({n})")
        }
        "clusters" => {
            let labels = body
                .pointer("/selector/matchLabels")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .filter(|s| !s.is_empty());
            match labels {
                Some(labels) => format!("clusters ({labels})"),
                None => "clusters (all)".into(),
            }
        }
        "git" => {
            let repo = body
                .get("repoURL")
                .and_then(Value::as_str)
                .map(short_repo)
                .unwrap_or_default();
            let what = if let Some(dirs) = body.get("directories").and_then(Value::as_array) {
                let paths: Vec<&str> = dirs
                    .iter()
                    .filter(|d| !d.get("exclude").and_then(Value::as_bool).unwrap_or(false))
                    .filter_map(|d| d.get("path").and_then(Value::as_str))
                    .collect();
                format!("directories {}", paths.join(", "))
            } else if let Some(files) = body.get("files").and_then(Value::as_array) {
                let paths: Vec<&str> = files
                    .iter()
                    .filter_map(|f| f.get("path").and_then(Value::as_str))
                    .collect();
                format!("files {}", paths.join(", "))
            } else {
                String::new()
            };
            format!("git {what} ({repo})").replace("  ", " ")
        }
        "matrix" | "merge" => {
            let inner: Vec<String> = body
                .get("generators")
                .and_then(Value::as_array)
                .map(|g| {
                    g.iter()
                        .map(|g| {
                            generator_summary(g)
                                .split_whitespace()
                                .next()
                                .unwrap_or("?")
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();
            let sep = if kind == "matrix" { " × " } else { " + " };
            format!("{kind} ({})", inner.join(sep))
        }
        "pullRequest" => {
            let provider = body
                .as_object()
                .and_then(|m| {
                    m.iter()
                        .find(|(k, _)| !matches!(k.as_str(), "requeueAfterSeconds" | "filters"))
                })
                .map(|(provider, spec)| {
                    let owner = spec
                        .get("owner")
                        .or_else(|| spec.get("project"))
                        .or_else(|| spec.get("organization"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let repo = spec
                        .get("repo")
                        .or_else(|| spec.get("repository"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    format!("{provider} {owner}/{repo}")
                })
                .unwrap_or_default();
            format!("pull requests ({provider})")
        }
        "scmProvider" => {
            let provider = body
                .as_object()
                .and_then(|m| {
                    m.keys()
                        .find(|k| {
                            !matches!(
                                k.as_str(),
                                "cloneProtocol"
                                    | "filters"
                                    | "requeueAfterSeconds"
                                    | "template"
                                    | "values"
                            )
                        })
                        .cloned()
                })
                .unwrap_or_default();
            format!("SCM provider ({provider})")
        }
        "clusterDecisionResource" => "cluster decision resource".into(),
        "plugin" => {
            let name = body
                .pointer("/configMapRef/name")
                .and_then(Value::as_str)
                .unwrap_or("plugin");
            format!("plugin ({name})")
        }
        other => other.to_string(),
    }
}

// ----- AppProject -----

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GroupKind {
    pub group: String,
    pub kind: String,
}

impl GroupKind {
    /// `apps/Deployment`, `*/*`, `Namespace` for the core group.
    pub fn label(&self) -> String {
        if self.group.is_empty() {
            self.kind.clone()
        } else {
            format!("{}/{}", self.group, self.kind)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectRole {
    pub name: String,
    pub description: Option<String>,
    pub policies: Vec<String>,
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SyncWindow {
    /// `allow` or `deny`.
    pub kind: String,
    /// A cron schedule (5 fields).
    pub schedule: String,
    /// A Go duration (`1h`, `30m`, `1h30m`).
    pub duration: String,
    pub applications: Vec<String>,
    pub namespaces: Vec<String>,
    pub clusters: Vec<String>,
    pub manual_sync: bool,
    pub time_zone: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProjectSpec {
    pub description: Option<String>,
    pub source_repos: Vec<String>,
    pub source_namespaces: Vec<String>,
    pub destinations: Vec<Destination>,
    pub cluster_resource_whitelist: Vec<GroupKind>,
    pub cluster_resource_blacklist: Vec<GroupKind>,
    pub namespace_resource_whitelist: Vec<GroupKind>,
    pub namespace_resource_blacklist: Vec<GroupKind>,
    pub roles: Vec<ProjectRole>,
    pub sync_windows: Vec<SyncWindow>,
}

/// An `AppProject`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Project {
    pub metadata: Meta,
    pub spec: ProjectSpec,
}

impl Project {
    pub fn parse(object: &Value) -> Option<Self> {
        serde_json::from_value(object.clone()).ok()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// The guestbook from `script/argocd-dev.sh` as Argo CD 3.5 reports it.
    pub(crate) fn guestbook() -> Value {
        json!({
            "apiVersion": "argoproj.io/v1alpha1",
            "kind": "Application",
            "metadata": {
                "name": "guestbook", "namespace": "argocd", "uid": "u1",
                "finalizers": ["resources-finalizer.argocd.argoproj.io"]
            },
            "spec": {
                "project": "kubyl-demo",
                "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps.git",
                           "path": "guestbook", "targetRevision": "HEAD"},
                "destination": {"server": "https://kubernetes.default.svc", "namespace": "guestbook"},
                "syncPolicy": {"syncOptions": ["CreateNamespace=true"]}
            },
            "status": {
                "sync": {"status": "Synced", "revision": "8088f4c0d970abb09e250248cc97e35623447cb5"},
                "health": {"status": "Healthy"},
                "history": [
                    {"id": 0, "revision": "68657670d9131dc5bc5f538b14c1de3377d74591",
                     "deployedAt": "2026-09-25T19:20:13Z", "initiatedBy": {"username": "argocd-dev.sh"},
                     "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps.git",
                                "path": "guestbook", "targetRevision": "HEAD"}},
                    {"id": 1, "revision": "8088f4c0d970abb09e250248cc97e35623447cb5",
                     "deployedAt": "2026-09-25T19:20:14Z", "initiatedBy": {"automated": true},
                     "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps.git",
                                "path": "guestbook", "targetRevision": "HEAD"}}
                ],
                "operationState": {
                    "phase": "Succeeded", "message": "successfully synced (all tasks run)",
                    "startedAt": "2026-09-25T19:20:14Z", "finishedAt": "2026-09-25T19:20:14Z",
                    "operation": {"initiatedBy": {"username": "argocd-dev.sh"}, "sync": {"revision": "HEAD"}},
                    "syncResult": {"revision": "8088f4c0d970abb09e250248cc97e35623447cb5",
                                   "resources": [{"kind": "Service", "name": "guestbook-ui", "namespace": "guestbook", "status": "Synced"}]}
                },
                "resources": [
                    {"kind": "Service", "name": "guestbook-ui", "namespace": "guestbook", "status": "Synced", "version": "v1"},
                    {"group": "apps", "kind": "Deployment", "name": "guestbook-ui", "namespace": "guestbook", "status": "OutOfSync", "version": "v1"}
                ],
                "controllerNamespace": "argocd",
                "sourceType": "Directory"
            }
        })
    }

    #[test]
    fn parses_an_application() {
        let app = Application::parse(&guestbook()).unwrap();
        assert_eq!(app.qualified_name(), "argocd/guestbook");
        assert_eq!(app.sync(), SyncStatus::Synced);
        assert_eq!(app.health(), Health::Healthy);
        assert!(!app.policy().auto_sync());
        assert!(app.cascades());
        assert_eq!(app.spec.destination.label(), "in-cluster · guestbook");
        assert!(app.spec.destination.is_in_cluster());
        let sources = app.spec.all_sources();
        assert_eq!(
            sources[0].repo_short(),
            "github.com/argoproj/argocd-example-apps"
        );
        assert_eq!(sources[0].what(), "guestbook");
        assert_eq!(sources[0].target(), "HEAD");
        assert_eq!(app.history_newest_first()[0].id, 1);
        assert_eq!(app.current_history_id(), Some(1));
        assert_eq!(app.out_of_sync().len(), 1);
        assert_eq!(app.out_of_sync()[0].api_version(), "apps/v1");
        let (phase, finished) = app.last_result().unwrap();
        assert_eq!(phase, OperationPhase::Succeeded);
        assert!(finished.is_some());
        assert_eq!(
            app.status.history[1].initiated_by.clone().unwrap().label(),
            "automated"
        );
        assert!(app.status.history[0].can_roll_back());
        assert_eq!(app.activity(), None);
    }

    #[test]
    fn activity_follows_the_operation() {
        let mut object = guestbook();
        object["status"]["operationState"]["phase"] = json!("Running");
        let app = Application::parse(&object).unwrap();
        assert_eq!(
            app.activity(),
            Some(Activity::Syncing { done: 1, total: 2 })
        );
        assert_eq!(app.activity().unwrap().label(), "Syncing 1/2");
        assert!(app.operation_in_progress());

        object["status"]["operationState"]["phase"] = json!("Succeeded");
        object["operation"] = json!({"sync": {"revision": "HEAD"}});
        let app = Application::parse(&object).unwrap();
        assert_eq!(app.activity(), Some(Activity::Requested));

        object["operation"] = Value::Null;
        object["metadata"]["annotations"] = json!({REFRESH_ANNOTATION: "hard"});
        assert_eq!(
            Application::parse(&object).unwrap().activity(),
            Some(Activity::Refreshing)
        );
        object["metadata"]["deletionTimestamp"] = json!("2026-09-25T19:20:14Z");
        assert_eq!(
            Application::parse(&object).unwrap().activity(),
            Some(Activity::Deleting)
        );
    }

    #[test]
    fn auto_sync_honors_enabled() {
        let policy: SyncPolicy =
            serde_json::from_value(json!({"automated": {"prune": true, "selfHeal": true}}))
                .unwrap();
        assert!(policy.auto_sync() && policy.prune() && policy.self_heal());
        let policy: SyncPolicy =
            serde_json::from_value(json!({"automated": {"enabled": false, "prune": true}}))
                .unwrap();
        assert!(!policy.auto_sync() && !policy.prune());
        let policy: SyncPolicy = serde_json::from_value(json!({
            "retry": {"limit": 5, "backoff": {"duration": "5s", "factor": 2}}
        }))
        .unwrap();
        assert_eq!(policy.retry_label().unwrap(), "retry 5 · 5s×2");
    }

    #[test]
    fn statuses_parse_leniently() {
        assert_eq!(SyncStatus::parse("OutOfSync"), SyncStatus::OutOfSync);
        assert_eq!(SyncStatus::parse(""), SyncStatus::Unknown);
        assert_eq!(Health::parse("Suspended"), Health::Suspended);
        assert_eq!(Health::parse("weird"), Health::Unknown);
        assert!(Health::Degraded.severity() > Health::Progressing.severity());
        // A new field or a wrong type elsewhere doesn't lose the app.
        let mut object = guestbook();
        object["status"]["somethingNew"] = json!({"x": 1});
        assert!(Application::parse(&object).is_some());
    }

    #[test]
    fn revisions_and_repos() {
        assert_eq!(
            short_revision("8088f4c0d970abb09e250248cc97e35623447cb5"),
            "8088f4c"
        );
        assert_eq!(short_revision("v1.2.3"), "v1.2.3");
        assert_eq!(short_revision("main"), "main");
        assert!(!is_sha("cafe"));
        assert_eq!(
            short_repo("git@github.com:acme/deploy.git"),
            "github.com/acme/deploy"
        );
        assert_eq!(
            short_repo("https://user:secret@gitlab.example.com/g/p.git"),
            "gitlab.example.com/g/p"
        );
        assert_eq!(
            short_repo("ssh://git@bitbucket.org/ws/repo.git"),
            "bitbucket.org/ws/repo"
        );
        assert_eq!(
            short_repo("https://charts.jetstack.io"),
            "charts.jetstack.io"
        );
    }

    #[test]
    fn multi_source_history() {
        let entry: HistoryEntry = serde_json::from_value(json!({
            "id": 3,
            "revisions": ["72.6.2", "abc1234def"],
            "sources": [
                {"repoURL": "https://prometheus-community.github.io/helm-charts", "chart": "kube-prometheus-stack", "targetRevision": "72.6.2"},
                {"repoURL": "https://github.com/acme/platform.git", "ref": "values"}
            ]
        }))
        .unwrap();
        assert_eq!(entry.all_revisions().len(), 2);
        assert_eq!(
            entry.all_sources()[0].what(),
            "kube-prometheus-stack 72.6.2"
        );
        assert_eq!(entry.all_sources()[1].what(), "$values");
        assert!(entry.can_roll_back());
        let old: HistoryEntry =
            serde_json::from_value(json!({"id": 1, "revision": "abc"})).unwrap();
        assert!(!old.can_roll_back());
    }

    #[test]
    fn application_set_summaries() {
        let set = ApplicationSet::parse(&json!({
            "metadata": {"name": "envs", "namespace": "argocd"},
            "spec": {
                "generators": [
                    {"list": {"elements": [{"env": "dev"}, {"env": "prod"}]}},
                    {"matrix": {"generators": [
                        {"git": {"repoURL": "https://github.com/acme/apps.git", "directories": [{"path": "apps/*"}]}},
                        {"clusters": {"selector": {"matchLabels": {"env": "prod"}}}}
                    ]}},
                    {"pullRequest": {"github": {"owner": "acme", "repo": "deploy"}, "requeueAfterSeconds": 60}}
                ],
                "syncPolicy": {"applicationsSync": "create-update", "preserveResourcesOnDeletion": true},
                "strategy": {"type": "RollingSync"}
            },
            "status": {"conditions": [
                {"type": "ErrorOccurred", "status": "False", "message": "ok", "reason": "ApplicationSetUpToDate"},
                {"type": "ParametersGenerated", "status": "False", "message": "bad", "reason": "x"}
            ]}
        }))
        .unwrap();
        assert_eq!(
            set.generators_summary(),
            "list (2) + matrix (git × clusters) + pull requests (github acme/deploy)"
        );
        assert_eq!(
            set.policy_summary(),
            "create-update · keeps resources on deletion"
        );
        assert!(set.rolling_sync());
        let problems: Vec<_> = set
            .status
            .conditions
            .iter()
            .filter(|c| c.is_problem())
            .collect();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, "ParametersGenerated");
        assert_eq!(
            generator_summary(
                &json!({"git": {"repoURL": "git@github.com:acme/x.git", "files": [{"path": "envs/*.json"}]}})
            ),
            "git files envs/*.json (github.com/acme/x)"
        );
    }

    #[test]
    fn projects() {
        let project = Project::parse(&json!({
            "metadata": {"name": "kubyl-demo", "namespace": "argocd"},
            "spec": {
                "sourceRepos": ["*"],
                "destinations": [{"server": "https://kubernetes.default.svc", "namespace": "guestbook*"}],
                "clusterResourceWhitelist": [{"group": "", "kind": "Namespace"}],
                "roles": [{"name": "read-only", "policies": ["p, proj:kubyl-demo:read-only, applications, get, kubyl-demo/*, allow"]}],
                "syncWindows": [{"kind": "deny", "schedule": "0 22 * * *", "duration": "8h", "applications": ["*"], "manualSync": true}]
            }
        }))
        .unwrap();
        assert_eq!(
            project.spec.destinations[0].label(),
            "in-cluster · guestbook*"
        );
        assert_eq!(
            project.spec.cluster_resource_whitelist[0].label(),
            "Namespace"
        );
        assert_eq!(project.spec.roles[0].policies.len(), 1);
        assert_eq!(project.spec.sync_windows[0].kind, "deny");
    }
}
