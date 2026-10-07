//! A Flux object as Kubyl reads it: the status fields every Flux kind shares (conditions,
//! suspend, revisions, observed generation, interval, reconcile requests) and the state they add
//! up to. Fields are read defensively: Flux moved some between API versions (`v2beta1`
//! HelmReleases have no `history`, `v1beta2` sources keep `artifact.checksum`), and a field that
//! isn't there is simply absent.

use std::sync::Arc;

use serde_json::Value;

use crate::kinds::FluxKind;

/// `metadata.annotations` key that asks a controller to reconcile now (`flux reconcile`).
pub const REQUESTED_AT: &str = "reconcile.fluxcd.io/requestedAt";
/// HelmRelease: upgrade even without changes (`flux reconcile hr --force`).
pub const FORCE_AT: &str = "reconcile.fluxcd.io/forceAt";
/// HelmRelease: reset the install and upgrade failure counters (`flux reconcile hr --reset`).
pub const RESET_AT: &str = "reconcile.fluxcd.io/resetAt";
/// Labels kustomize-controller puts on what it applies.
pub const KUSTOMIZE_NAME_LABEL: &str = "kustomize.toolkit.fluxcd.io/name";
pub const KUSTOMIZE_NAMESPACE_LABEL: &str = "kustomize.toolkit.fluxcd.io/namespace";
/// Labels helm-controller puts on what it installs.
pub const HELM_NAME_LABEL: &str = "helm.toolkit.fluxcd.io/name";
pub const HELM_NAMESPACE_LABEL: &str = "helm.toolkit.fluxcd.io/namespace";

/// What a Flux object's status adds up to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum State {
    Ready,
    Reconciling,
    Suspended,
    Failed,
    Stalled,
    Unknown,
}

impl State {
    pub const ALL: [State; 6] = [
        State::Ready,
        State::Reconciling,
        State::Failed,
        State::Stalled,
        State::Suspended,
        State::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            State::Ready => "Ready",
            State::Reconciling => "Reconciling",
            State::Suspended => "Suspended",
            State::Failed => "Failed",
            State::Stalled => "Stalled",
            State::Unknown => "Unknown",
        }
    }

    /// Failed and stalled objects need someone to look at them.
    pub fn is_problem(self) -> bool {
        matches!(self, State::Failed | State::Stalled)
    }
}

/// A status condition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Condition {
    pub kind: String,
    /// `True`, `False` or `Unknown`.
    pub status: String,
    pub reason: String,
    pub message: String,
    pub last_transition: Option<String>,
    pub observed_generation: Option<i64>,
}

impl Condition {
    pub fn parse(value: &Value) -> Option<Condition> {
        Some(Condition {
            kind: value.get("type")?.as_str()?.to_string(),
            status: str_of(value, "status").unwrap_or_else(|| "Unknown".into()),
            reason: str_of(value, "reason").unwrap_or_default(),
            message: str_of(value, "message").unwrap_or_default(),
            last_transition: str_of(value, "lastTransitionTime"),
            observed_generation: value.get("observedGeneration").and_then(Value::as_i64),
        })
    }

    pub fn is_true(&self) -> bool {
        self.status == "True"
    }

    pub fn is_false(&self) -> bool {
        self.status == "False"
    }
}

/// A reference to another Flux object (`sourceRef`, `dependsOn`, `providerRef`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectRef {
    pub kind: Option<FluxKind>,
    /// The kind as written (also when Kubyl doesn't know it).
    pub kind_name: String,
    pub namespace: String,
    pub name: String,
}

impl ObjectRef {
    pub fn new(kind: FluxKind, namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            kind: Some(kind),
            kind_name: kind.kind().to_string(),
            namespace: namespace.into(),
            name: name.into(),
        }
    }

    /// Parses `{kind, name, namespace}`; the namespace defaults to `namespace` (the referring
    /// object's) and the kind to `default_kind`.
    pub fn parse(value: &Value, namespace: &str, default_kind: Option<FluxKind>) -> Option<Self> {
        let name = value.get("name")?.as_str()?.to_string();
        let kind_name = str_of(value, "kind")
            .or_else(|| default_kind.map(|k| k.kind().to_string()))
            .unwrap_or_default();
        Some(ObjectRef {
            kind: FluxKind::from_kind(&kind_name),
            kind_name,
            namespace: str_of(value, "namespace").unwrap_or_else(|| namespace.to_string()),
            name,
        })
    }

    /// `GitRepository/podinfo`, with the namespace when it differs from `own_namespace`.
    pub fn label(&self, own_namespace: &str) -> String {
        if self.namespace == own_namespace {
            format!("{}/{}", self.kind_name, self.name)
        } else {
            format!("{}/{}/{}", self.kind_name, self.namespace, self.name)
        }
    }

    /// `namespace/name`.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

/// A source's artifact.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Artifact {
    pub revision: String,
    /// `sha256:…` (`checksum` in `v1beta2`).
    pub digest: Option<String>,
    pub last_update: Option<String>,
    pub size: Option<i64>,
    /// The commit the OCI artifact was built from (`org.opencontainers.image.revision`).
    pub source_revision: Option<String>,
    /// The repository the OCI artifact was built from (`org.opencontainers.image.source`).
    pub source_url: Option<String>,
}

/// A Flux object of any kind, parsed from its JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct FluxObject {
    pub kind: FluxKind,
    pub api_version: String,
    pub namespace: String,
    pub name: String,
    pub uid: Option<String>,
    pub created: Option<String>,
    pub generation: Option<i64>,
    pub observed_generation: Option<i64>,
    pub deleting: bool,
    pub suspended: bool,
    pub interval: Option<String>,
    pub conditions: Vec<Condition>,
    pub last_applied_revision: Option<String>,
    pub last_attempted_revision: Option<String>,
    pub artifact: Option<Artifact>,
    /// Where it gets its artifact from (Kustomization, HelmRelease, HelmChart,
    /// ImageUpdateAutomation, ImagePolicy's ImageRepository).
    pub source: Option<ObjectRef>,
    pub depends_on: Vec<ObjectRef>,
    /// `reconcile.fluxcd.io/requestedAt` and the last one the controller handled.
    pub requested_at: Option<String>,
    pub last_handled: Option<String>,
    pub raw: Arc<Value>,
}

impl FluxObject {
    pub fn parse(object: &Arc<Value>) -> Option<FluxObject> {
        let kind = FluxKind::of(object)?;
        Self::parse_as(kind, object)
    }

    /// Parses an object whose kind is known (lists: watches return items without `kind`).
    pub fn parse_as(kind: FluxKind, object: &Arc<Value>) -> Option<FluxObject> {
        let meta = object.get("metadata")?;
        let name = str_of(meta, "name")?;
        let namespace = str_of(meta, "namespace").unwrap_or_default();
        let spec = object.get("spec").unwrap_or(&Value::Null);
        let status = object.get("status").unwrap_or(&Value::Null);
        let conditions = status
            .get("conditions")
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(Condition::parse).collect())
            .unwrap_or_default();
        let artifact = status.get("artifact").and_then(|a| {
            Some(Artifact {
                revision: str_of(a, "revision")?,
                digest: str_of(a, "digest").or_else(|| str_of(a, "checksum")),
                last_update: str_of(a, "lastUpdateTime"),
                size: a.get("size").and_then(Value::as_i64),
                source_revision: a
                    .pointer("/metadata/org.opencontainers.image.revision")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_url: a
                    .pointer("/metadata/org.opencontainers.image.source")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        });
        let source = match kind {
            FluxKind::Kustomization | FluxKind::HelmChart | FluxKind::ImageUpdateAutomation => spec
                .get("sourceRef")
                .and_then(|r| ObjectRef::parse(r, &namespace, None)),
            FluxKind::HelmRelease => helm_release_source(spec, &namespace),
            FluxKind::ImagePolicy => spec
                .get("imageRepositoryRef")
                .and_then(|r| ObjectRef::parse(r, &namespace, Some(FluxKind::ImageRepository))),
            FluxKind::Alert => spec
                .get("providerRef")
                .and_then(|r| ObjectRef::parse(r, &namespace, Some(FluxKind::Provider))),
            _ => None,
        };
        let depends_on = spec
            .get("dependsOn")
            .and_then(Value::as_array)
            .map(|deps| {
                deps.iter()
                    .filter_map(|d| ObjectRef::parse(d, &namespace, Some(kind)))
                    .collect()
            })
            .unwrap_or_default();
        Some(FluxObject {
            kind,
            api_version: str_of(object, "apiVersion").unwrap_or_default(),
            uid: str_of(meta, "uid"),
            created: str_of(meta, "creationTimestamp"),
            generation: meta.get("generation").and_then(Value::as_i64),
            observed_generation: status.get("observedGeneration").and_then(Value::as_i64),
            deleting: meta.get("deletionTimestamp").is_some(),
            suspended: spec.get("suspend").and_then(Value::as_bool) == Some(true),
            interval: str_of(spec, "interval"),
            conditions,
            last_applied_revision: str_of(status, "lastAppliedRevision")
                .or_else(|| helm_history_revision(status)),
            last_attempted_revision: str_of(status, "lastAttemptedRevision"),
            artifact,
            source,
            depends_on,
            requested_at: meta
                .pointer("/annotations")
                .and_then(|a| a.get(REQUESTED_AT))
                .and_then(Value::as_str)
                .map(str::to_string),
            last_handled: str_of(status, "lastHandledReconcileAt"),
            namespace,
            name,
            raw: object.clone(),
        })
    }

    pub fn condition(&self, kind: &str) -> Option<&Condition> {
        self.conditions.iter().find(|c| c.kind == kind)
    }

    pub fn ready(&self) -> Option<&Condition> {
        self.condition("Ready")
    }

    /// The API version (`v1beta3`) without the group.
    pub fn version(&self) -> &str {
        self.api_version
            .rsplit_once('/')
            .map_or(self.api_version.as_str(), |(_, v)| v)
    }

    /// No controller reconciles it, so it has no status: Alerts and Providers from `v1beta3` on,
    /// and HelmRepositories of `type: oci` (source-controller empties their status and leaves
    /// them alone since Flux 2.3; helm-controller pulls their charts directly).
    pub fn is_static(&self) -> bool {
        !self.kind.has_status_at(self.version()) || self.is_oci_helm_repository()
    }

    fn is_oci_helm_repository(&self) -> bool {
        self.kind == FluxKind::HelmRepository
            && self.raw.pointer("/spec/type").and_then(Value::as_str) == Some("oci")
    }

    /// Whether a reconcile request makes the controller act (not for static objects).
    pub fn reconcilable(&self) -> bool {
        self.kind.reconcilable_at(self.version()) && !self.is_static()
    }

    /// Whether `spec.suspend` exists at its API version.
    pub fn suspendable(&self) -> bool {
        self.kind.suspendable_at(self.version())
    }

    /// A reconcile was requested that the controller hasn't handled yet.
    pub fn reconcile_pending(&self) -> bool {
        self.reconcilable()
            && !self.suspended
            && self
                .requested_at
                .as_ref()
                .is_some_and(|r| self.last_handled.as_ref() != Some(r))
    }

    /// Whether objects that depend on it may go ahead, the way kustomize-controller and
    /// helm-controller check `dependsOn`: it has conditions, its controller has seen its latest
    /// generation, and Ready is True. Suspended or still reconciling objects that are Ready
    /// don't hold their dependents back.
    pub fn ready_for_dependents(&self) -> bool {
        !self.conditions.is_empty()
            && self.generation.unwrap_or(0) == self.observed_generation.unwrap_or(0)
            && self.ready().is_some_and(Condition::is_true)
    }

    /// The object's state, the way `flux get` and kstatus read it.
    pub fn state(&self) -> State {
        if self.suspended {
            return State::Suspended;
        }
        if self.is_static() {
            return State::Ready;
        }
        if self.condition("Stalled").is_some_and(Condition::is_true) {
            return State::Stalled;
        }
        if self.reconcile_pending() {
            return State::Reconciling;
        }
        let ready = self.ready();
        if ready.is_some_and(Condition::is_false) {
            return State::Failed;
        }
        if self
            .condition("Reconciling")
            .is_some_and(Condition::is_true)
        {
            return State::Reconciling;
        }
        match ready {
            Some(ready) if ready.is_true() => {
                // The controller hasn't seen the latest spec yet.
                if let (Some(generation), Some(observed)) =
                    (self.generation, self.observed_generation)
                    && observed >= 0
                    && observed < generation
                {
                    State::Reconciling
                } else {
                    State::Ready
                }
            }
            Some(_) => State::Reconciling,
            None => State::Unknown,
        }
    }

    /// Ready's message (or why the state is what it is).
    pub fn message(&self) -> String {
        if self.is_oci_helm_repository() {
            return "Static object: source-controller doesn't reconcile OCI HelmRepositories; \
                    helm-controller pulls their charts directly."
                .into();
        }
        if self.is_static() {
            return "Static object: the notification controller reads it when events arrive."
                .into();
        }
        if self.reconcile_pending() {
            return "Reconcile requested; waiting for the controller.".into();
        }
        if let Some(stalled) = self.condition("Stalled").filter(|c| c.is_true()) {
            return stalled.message.clone();
        }
        match self.ready() {
            Some(ready) if !ready.message.is_empty() => ready.message.clone(),
            _ if self.suspended => "Suspended: the controller skips it.".into(),
            _ => "No status yet.".into(),
        }
    }

    /// The revision it's at: applied for Kustomizations and HelmReleases, the artifact's for
    /// sources, the latest image for ImagePolicies, the last pushed commit for automations.
    pub fn revision(&self) -> Option<String> {
        if let Some(applied) = &self.last_applied_revision {
            return Some(applied.clone());
        }
        if let Some(artifact) = &self.artifact {
            return Some(artifact.revision.clone());
        }
        let status = self.raw.get("status")?;
        match self.kind {
            FluxKind::ImagePolicy => latest_image(status),
            FluxKind::ImageUpdateAutomation => str_of(status, "lastPushCommit"),
            FluxKind::ImageRepository => status
                .pointer("/lastScanResult/tagCount")
                .and_then(Value::as_i64)
                .map(|n| format!("{n} tags")),
            _ => None,
        }
    }

    /// When it last reconciled: Ready's transition, else the artifact's update.
    pub fn last_reconcile(&self) -> Option<String> {
        self.ready()
            .and_then(|r| r.last_transition.clone())
            .or_else(|| self.artifact.as_ref().and_then(|a| a.last_update.clone()))
            .or_else(|| {
                self.raw
                    .pointer("/status/lastScanResult/scanTime")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    /// `namespace/name`.
    pub fn key(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }

    pub fn as_ref(&self) -> ObjectRef {
        ObjectRef::new(self.kind, &self.namespace, &self.name)
    }
}

/// A HelmRelease's chart source: `spec.chart.spec.sourceRef` (HelmRepository, GitRepository,
/// Bucket) or `spec.chartRef` (OCIRepository, HelmChart).
fn helm_release_source(spec: &Value, namespace: &str) -> Option<ObjectRef> {
    spec.pointer("/chart/spec/sourceRef")
        .or_else(|| spec.get("chartRef"))
        .and_then(|r| ObjectRef::parse(r, namespace, None))
}

/// `v2` HelmReleases keep the chart version in `status.history[0]` (no `lastAppliedRevision`).
fn helm_history_revision(status: &Value) -> Option<String> {
    let latest = status.get("history")?.as_array()?.first()?;
    // Kustomizations' history entries have `metadata.revision`, not a chart version.
    str_of(latest, "chartVersion")
}

/// `ghcr.io/stefanprodan/podinfo:6.15.0` (v1: `latestRef`, before: `latestImage`).
pub fn latest_image(status: &Value) -> Option<String> {
    if let Some(latest) = status.get("latestRef") {
        let name = str_of(latest, "name")?;
        let tag = str_of(latest, "tag");
        let digest = str_of(latest, "digest");
        return Some(match (tag, digest) {
            (Some(tag), _) => format!("{name}:{tag}"),
            (None, Some(digest)) => format!("{name}@{digest}"),
            (None, None) => name,
        });
    }
    str_of(status, "latestImage")
}

/// A short revision for tables: `master@sha1:3e0ff8a`, `latest@sha256:87815bbd`, `6.15.0`.
/// Old formats (`master/3e0ff8a…`, a bare SHA) are shortened too.
pub fn short_revision(revision: &str) -> String {
    let shorten = |hash: &str, len: usize| hash.chars().take(len).collect::<String>();
    // `<ref>@<algo>:<hash>` (Flux 2.x) or `<algo>:<hash>`.
    let (prefix, digest) = match revision.rsplit_once('@') {
        Some((prefix, digest)) => (Some(prefix), digest),
        None => (None, revision),
    };
    let digest = match digest.split_once(':') {
        Some((algo, hash)) if matches!(algo, "sha1" | "sha256" | "sha384" | "sha512") => {
            let len = if algo == "sha1" { 7 } else { 8 };
            format!("{algo}:{}", shorten(hash, len))
        }
        _ => {
            // `master/3e0ff8a…` (Flux < 2.0) or a bare commit.
            if let Some((branch, sha)) = digest.rsplit_once('/')
                && is_hex(sha)
                && sha.len() >= 12
            {
                return format!("{branch}/{}", shorten(sha, 7));
            }
            if is_hex(digest) && digest.len() >= 12 {
                shorten(digest, 7)
            } else {
                digest.to_string()
            }
        }
    };
    match prefix {
        Some(prefix) => format!("{prefix}@{digest}"),
        None => digest,
    }
}

/// The commit of a revision (`master@sha1:<sha>`, `master/<sha>`, a bare SHA), for links.
pub fn commit_of(revision: &str) -> Option<String> {
    let tail = revision.rsplit_once('@').map_or(revision, |(_, d)| d);
    let sha = tail
        .strip_prefix("sha1:")
        .or_else(|| tail.rsplit_once('/').map(|(_, s)| s))
        .unwrap_or(tail);
    (sha.len() >= 7 && sha.len() <= 64 && is_hex(sha) && !tail.starts_with("sha256:"))
        .then(|| sha.to_string())
}

fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_hexdigit())
}

pub(crate) fn str_of(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn states_of_the_fixtures() {
        let state = |v: Value| FluxObject::parse(&Arc::new(v)).unwrap().state();
        assert_eq!(state(fixtures::kustomization_ready()), State::Ready);
        assert_eq!(state(fixtures::kustomization_failed()), State::Failed);
        assert_eq!(state(fixtures::kustomization_suspended()), State::Suspended);
        assert_eq!(state(fixtures::kustomization_waiting()), State::Failed);
        assert_eq!(state(fixtures::helm_release_v2()), State::Ready);
        assert_eq!(state(fixtures::helm_release_v2beta1()), State::Ready);
        assert_eq!(state(fixtures::helm_release_stalled()), State::Stalled);
        assert_eq!(state(fixtures::git_repository()), State::Ready);
        assert_eq!(state(fixtures::git_repository_v1beta2()), State::Ready);
        assert_eq!(state(fixtures::oci_repository()), State::Ready);
        assert_eq!(state(fixtures::helm_repository()), State::Ready);
        assert_eq!(state(fixtures::image_policy()), State::Ready);
        assert_eq!(state(fixtures::image_policy_v1beta2()), State::Ready);
        assert_eq!(state(fixtures::alert()), State::Ready);
        assert_eq!(state(fixtures::alert_v1beta2()), State::Failed);
        assert_eq!(state(fixtures::receiver()), State::Ready);
        assert_eq!(state(fixtures::bucket()), State::Ready);
        assert_eq!(state(fixtures::helm_chart()), State::Ready);
        assert_eq!(state(fixtures::external_artifact()), State::Ready);
        assert_eq!(state(fixtures::image_update_automation()), State::Ready);
        assert_eq!(state(fixtures::helm_repository_oci()), State::Ready);
        // Never reconciled: no conditions at all.
        assert_eq!(
            state(
                serde_json::json!({"apiVersion": "kustomize.toolkit.fluxcd.io/v1", "kind": "Kustomization",
                "metadata": {"name": "x", "namespace": "y"}})
            ),
            State::Unknown
        );
    }

    #[test]
    fn reconciling_while_the_controller_catches_up() {
        // A newer spec than the controller has seen.
        let mut object = fixtures::kustomization_ready();
        object["metadata"]["generation"] = 3.into();
        let parsed = FluxObject::parse(&Arc::new(object.clone())).unwrap();
        assert_eq!(parsed.state(), State::Reconciling);
        // A reconcile request it hasn't handled.
        let mut object = fixtures::kustomization_ready();
        object["metadata"]["annotations"] =
            serde_json::json!({REQUESTED_AT: "2026-10-07T11:00:00Z"});
        let parsed = FluxObject::parse(&Arc::new(object.clone())).unwrap();
        assert!(parsed.reconcile_pending());
        assert_eq!(parsed.state(), State::Reconciling);
        object["status"]["lastHandledReconcileAt"] = "2026-10-07T11:00:00Z".into();
        let parsed = FluxObject::parse(&Arc::new(object)).unwrap();
        assert_eq!(parsed.state(), State::Ready);
        // Reconciling=True with Ready=Unknown: in progress.
        let object = serde_json::json!({"apiVersion": "kustomize.toolkit.fluxcd.io/v1", "kind": "Kustomization",
            "metadata": {"name": "x", "namespace": "y"},
            "status": {"conditions": [
                {"type": "Reconciling", "status": "True", "reason": "Progressing", "message": "Building"},
                {"type": "Ready", "status": "Unknown", "reason": "Progressing", "message": "Reconciliation in progress"}]}});
        let parsed = FluxObject::parse(&Arc::new(object)).unwrap();
        assert_eq!(parsed.state(), State::Reconciling);
        assert_eq!(parsed.message(), "Reconciliation in progress");
    }

    #[test]
    fn fields_per_kind_and_version() {
        let parse = |v: Value| FluxObject::parse(&Arc::new(v)).unwrap();
        let ks = parse(fixtures::kustomization_ready());
        assert_eq!(
            ks.source.as_ref().unwrap().label("flux-demo"),
            "GitRepository/podinfo"
        );
        assert_eq!(ks.interval.as_deref(), Some("10m"));
        assert_eq!(
            ks.revision().map(|r| short_revision(&r)).as_deref(),
            Some("master@sha1:3e0ff8a")
        );
        let hr = parse(fixtures::helm_release_v2());
        assert_eq!(hr.revision().as_deref(), Some("6.15.0"));
        assert_eq!(
            hr.source.as_ref().unwrap().kind,
            Some(FluxKind::HelmRepository)
        );
        let old = parse(fixtures::helm_release_v2beta1());
        assert_eq!(old.revision().as_deref(), Some("6.5.4"));
        let chart_ref = parse(fixtures::helm_release_chart_ref());
        assert_eq!(
            chart_ref.source.as_ref().unwrap().label("apps"),
            "OCIRepository/flux-system/podinfo"
        );
        let git = parse(fixtures::git_repository());
        let artifact = git.artifact.as_ref().unwrap();
        assert!(artifact.digest.as_ref().unwrap().starts_with("sha256:"));
        let old_git = parse(fixtures::git_repository_v1beta2());
        assert_eq!(
            old_git.artifact.as_ref().unwrap().digest.as_deref(),
            Some("1f7a0e2c9b")
        );
        assert_eq!(short_revision(&old_git.revision().unwrap()), "main/9f3c2a1");
        let oci = parse(fixtures::oci_repository());
        assert_eq!(
            oci.artifact.as_ref().unwrap().source_revision.as_deref(),
            Some("6.15.0@sha1:dd507173b7b75b2312a36cabe0de5f09c1ce69c8")
        );
        let policy = parse(fixtures::image_policy());
        assert_eq!(
            policy.revision().as_deref(),
            Some("ghcr.io/stefanprodan/podinfo:6.15.0")
        );
        let old_policy = parse(fixtures::image_policy_v1beta2());
        assert_eq!(
            old_policy.revision().as_deref(),
            Some("ghcr.io/stefanprodan/podinfo:6.5.0")
        );
        let alert = parse(fixtures::alert());
        assert_eq!(
            alert.source.as_ref().unwrap().kind,
            Some(FluxKind::Provider)
        );
        let waiting = parse(fixtures::kustomization_waiting());
        assert_eq!(waiting.depends_on[0].key(), "flux-demo/broken");
        assert_eq!(waiting.depends_on[0].kind, Some(FluxKind::Kustomization));
    }

    #[test]
    fn short_revisions() {
        assert_eq!(
            short_revision("master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2"),
            "master@sha1:3e0ff8a"
        );
        assert_eq!(
            short_revision(
                "latest@sha256:87815bbd58f5bfd5ad9deabb76dbd0615562cd0df288759c6e50ee54b2860b0c"
            ),
            "latest@sha256:87815bbd"
        );
        assert_eq!(
            short_revision(
                "sha256:e7dc68a4dec90a35c2c6d8cdfedb7eaaee17fde45dced5898289df85069ec089"
            ),
            "sha256:e7dc68a4"
        );
        assert_eq!(short_revision("6.15.0"), "6.15.0");
        assert_eq!(
            short_revision("main/9f3c2a1b2c3d4e5f60718293a4b5c6d7e8f90123"),
            "main/9f3c2a1"
        );
        assert_eq!(
            short_revision("refs/tags/v1.2.0@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2"),
            "refs/tags/v1.2.0@sha1:3e0ff8a"
        );
        assert_eq!(
            commit_of("master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2").as_deref(),
            Some("3e0ff8ae123b710bc91de1315cba0f996a8896c2")
        );
        assert_eq!(
            commit_of("main/9f3c2a1b2c3d").as_deref(),
            Some("9f3c2a1b2c3d")
        );
        assert_eq!(commit_of("sha256:e7dc68a4dec90a35"), None);
        assert_eq!(commit_of("6.15.0"), None);
    }

    #[test]
    fn static_objects_take_no_reconcile_requests() {
        let parse = |v: Value| FluxObject::parse(&Arc::new(v)).unwrap();
        let mut oci = fixtures::helm_repository_oci();
        oci["metadata"]["annotations"] = serde_json::json!({REQUESTED_AT: "review"});
        let oci = parse(oci);
        assert!(oci.is_static() && !oci.reconcilable());
        // A request nobody handles doesn't make it "Reconciling" forever.
        assert!(!oci.reconcile_pending());
        assert_eq!(oci.state(), State::Ready);
        assert!(oci.message().contains("OCI HelmRepositories"));
        assert!(parse(fixtures::helm_repository()).reconcilable());
        assert!(parse(fixtures::alert()).is_static());
        assert_eq!(parse(fixtures::alert_v1beta2()).version(), "v1beta2");
        assert!(parse(fixtures::alert_v1beta2()).reconcilable());
        assert!(!parse(fixtures::external_artifact()).reconcilable());
        // Flux < 2.7 ImagePolicies: no requests, no suspend; a stale annotation isn't pending.
        let mut old_policy = fixtures::image_policy_v1beta2();
        old_policy["metadata"]["annotations"] = serde_json::json!({REQUESTED_AT: "x"});
        let old_policy = parse(old_policy);
        assert!(!old_policy.reconcilable() && !old_policy.suspendable());
        assert_eq!(old_policy.state(), State::Ready);
        assert!(parse(fixtures::image_policy()).suspendable());
        assert_eq!(
            parse(fixtures::bucket())
                .revision()
                .map(|r| short_revision(&r))
                .as_deref(),
            Some("sha256:7c2bd8f1")
        );
    }

    #[test]
    fn malformed_status_is_read_defensively() {
        let parse = |status: Value| {
            FluxObject::parse(&Arc::new(serde_json::json!({
                "apiVersion": "kustomize.toolkit.fluxcd.io/v1", "kind": "Kustomization",
                "metadata": {"name": "x", "namespace": "y", "generation": 2},
                "status": status})))
            .unwrap()
        };
        // Conditions that aren't an array: none.
        let odd =
            parse(serde_json::json!({"conditions": {"type": "Ready"}, "observedGeneration": "2"}));
        assert!(odd.conditions.is_empty());
        assert_eq!(odd.observed_generation, None);
        assert_eq!(odd.state(), State::Unknown);
        // A condition without a type is skipped; one without a status is Unknown.
        let partial = parse(serde_json::json!({"conditions": [
            {"status": "False", "message": "no type"},
            {"type": "Ready", "message": "no status"}]}));
        assert_eq!(partial.conditions.len(), 1);
        assert_eq!(partial.ready().unwrap().status, "Unknown");
        assert_eq!(partial.state(), State::Reconciling);
        // Status that isn't an object at all.
        assert_eq!(
            parse(Value::String("broken".into())).state(),
            State::Unknown
        );
    }

    /// Flux 2.0–2.2 versions: a `v1beta2` Kustomization and a `v2beta2` HelmRelease (history
    /// snapshots and failure counters, `lastAttemptedRevision`, no `lastAppliedRevision`).
    #[test]
    fn older_kustomization_and_helm_release_versions() {
        let ks = FluxObject::parse(&Arc::new(serde_json::json!({
            "apiVersion": "kustomize.toolkit.fluxcd.io/v1beta2", "kind": "Kustomization",
            "metadata": {"name": "apps", "namespace": "flux-system", "generation": 4},
            "spec": {"interval": "5m", "path": "./apps", "prune": true,
                "sourceRef": {"kind": "GitRepository", "name": "flux-system"}},
            "status": {"observedGeneration": 4,
                "lastAppliedRevision": "main@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
                "conditions": [{"type": "Ready", "status": "True", "reason": "ReconciliationSucceeded",
                    "message": "Applied revision: main@sha1:3e0ff8a"}]}})))
        .unwrap();
        assert_eq!(ks.state(), State::Ready);
        assert_eq!(short_revision(&ks.revision().unwrap()), "main@sha1:3e0ff8a");
        assert!(ks.reconcilable() && ks.suspendable());
        let hr = FluxObject::parse(&Arc::new(serde_json::json!({
            "apiVersion": "helm.toolkit.fluxcd.io/v2beta2", "kind": "HelmRelease",
            "metadata": {"name": "redis", "namespace": "data", "generation": 2},
            "spec": {"interval": "10m", "chart": {"spec": {"chart": "redis",
                "sourceRef": {"kind": "HelmRepository", "name": "bitnami"}}}},
            "status": {"observedGeneration": 2, "lastAttemptedRevision": "18.1.0",
                "storageNamespace": "data", "upgradeFailures": 1,
                "history": [{"chartName": "redis", "chartVersion": "18.1.0", "name": "redis",
                    "namespace": "data", "status": "deployed", "version": 3}],
                "conditions": [{"type": "Ready", "status": "True", "reason": "UpgradeSucceeded",
                    "message": "Helm upgrade succeeded"}]}})))
        .unwrap();
        assert_eq!(hr.state(), State::Ready);
        assert_eq!(hr.revision().as_deref(), Some("18.1.0"));
        assert_eq!(
            crate::details::helm_storage(&hr),
            Some(("data".into(), "sh.helm.release.v1.redis.v3".into()))
        );
        assert_eq!(crate::details::failures(&hr).upgrade, 1);
    }

    /// The last result is a failure even when the controller hasn't seen the newest spec yet
    /// (`flux get` shows the same); dependents aren't released either.
    #[test]
    fn failed_with_an_older_observed_generation() {
        let mut object = fixtures::kustomization_failed();
        object["metadata"]["generation"] = 2.into();
        object["status"]["observedGeneration"] = 1.into();
        let parsed = FluxObject::parse(&Arc::new(object)).unwrap();
        assert_eq!(parsed.state(), State::Failed);
        assert!(!parsed.ready_for_dependents());
        let mut ready = fixtures::kustomization_ready();
        ready["metadata"]["generation"] = 3.into();
        let parsed = FluxObject::parse(&Arc::new(ready)).unwrap();
        assert_eq!(parsed.state(), State::Reconciling);
        assert!(!parsed.ready_for_dependents());
    }
}
