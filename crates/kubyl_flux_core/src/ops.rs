//! Flux actions the way the `flux` CLI takes them, with the user's own Kubernetes access and no
//! CLI: every action is a JSON merge patch on the object (or a delete).
//!
//! - reconcile: `metadata.annotations["reconcile.fluxcd.io/requestedAt"] = <now>`; "with source"
//!   annotates the source first and, like `flux reconcile --with-source`, waits (at most
//!   [`SOURCE_WAIT`]) until its controller handled that request and the source is ready;
//! - HelmRelease force / reset: `reconcile.fluxcd.io/forceAt` / `resetAt` set to the same value
//!   as `requestedAt` (helm-controller only acts when they match);
//! - suspend: `spec.suspend = true`; resume: `spec.suspend = false` plus a reconcile request
//!   (what `flux resume` does);
//! - delete: a Kubernetes delete; what the controller's finalizer then removes is
//!   [`delete_effect`].

use std::time::Duration;

use kube::Client;
use kube::api::{Api, DeleteParams, DynamicObject, Patch, PatchParams};
use kube::discovery::ApiResource;
use serde_json::{Value, json};

use crate::inventory::{self, Entry};
use crate::kinds::FluxKind;
use crate::model::{FORCE_AT, FluxObject, ObjectRef, REQUESTED_AT, RESET_AT};

/// How long "with source" waits for the source's controller before giving up.
pub const SOURCE_WAIT: Duration = Duration::from_secs(120);
const SOURCE_POLL: Duration = Duration::from_secs(1);

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Reconcile,
    /// Reconcile the source first, then the object.
    ReconcileWithSource,
    /// HelmRelease: upgrade even without changes.
    Force,
    /// HelmRelease: reset the failure counters (after a Stalled install or upgrade).
    Reset,
    Suspend,
    Resume,
    Delete,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Reconcile => "Reconcile",
            Action::ReconcileWithSource => "Reconcile with source",
            Action::Force => "Force upgrade",
            Action::Reset => "Reset failures",
            Action::Suspend => "Suspend",
            Action::Resume => "Resume",
            Action::Delete => "Delete",
        }
    }

    /// The past tense for toasts.
    pub fn done(self, what: &str) -> String {
        match self {
            Action::Reconcile | Action::ReconcileWithSource => {
                format!("Requested a reconcile of {what}")
            }
            Action::Force => format!("Requested a forced upgrade of {what}"),
            Action::Reset => format!("Reset the failure counters of {what}"),
            Action::Suspend => format!("Suspended {what}"),
            Action::Resume => format!("Resumed {what}"),
            Action::Delete => format!("Deleting {what}"),
        }
    }

    /// Whether the action applies to `object` in its current state.
    pub fn applies_to(self, object: &FluxObject) -> bool {
        let kind = object.kind;
        match self {
            Action::Reconcile => object.reconcilable() && !object.suspended,
            Action::ReconcileWithSource => {
                object.reconcilable()
                    && !object.suspended
                    && object
                        .source
                        .as_ref()
                        .and_then(|s| s.kind)
                        .is_some_and(is_reconcilable_source)
            }
            Action::Force | Action::Reset => kind == FluxKind::HelmRelease && !object.suspended,
            Action::Suspend => object.suspendable() && !object.suspended,
            Action::Resume => object.suspendable() && object.suspended,
            Action::Delete => true,
        }
    }

    /// Whether it writes something that can't simply be taken back (asks first).
    pub fn needs_confirmation(self) -> bool {
        matches!(self, Action::Delete | Action::Suspend | Action::Force)
    }
}

/// Why an action failed, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpError {
    #[error("permission denied: {0}")]
    Forbidden(String),
    #[error("it doesn't exist anymore")]
    NotFound,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Api(String),
}

impl From<kube::Error> for OpError {
    fn from(err: kube::Error) -> Self {
        match err {
            kube::Error::Api(status) => match status.code {
                403 => OpError::Forbidden(status.message),
                404 => OpError::NotFound,
                _ => OpError::Api(status.message),
            },
            other => OpError::Api(other.to_string()),
        }
    }
}

/// `reconcile.fluxcd.io/requestedAt` (RFC 3339 with nanoseconds, like the CLI).
pub fn now() -> String {
    jiff::Timestamp::now().to_string()
}

/// The merge patch of an action (`None` for delete). `at` is the request's time.
pub fn patch(action: Action, at: &str) -> Option<Value> {
    Some(match action {
        Action::Reconcile | Action::ReconcileWithSource => {
            json!({"metadata": {"annotations": {REQUESTED_AT: at}}})
        }
        Action::Force => json!({"metadata": {"annotations": {REQUESTED_AT: at, FORCE_AT: at}}}),
        Action::Reset => json!({"metadata": {"annotations": {REQUESTED_AT: at, RESET_AT: at}}}),
        Action::Suspend => json!({"spec": {"suspend": true}}),
        Action::Resume => json!({
            "metadata": {"annotations": {REQUESTED_AT: at}},
            "spec": {"suspend": false}
        }),
        Action::Delete => return None,
    })
}

/// The source "reconcile with source" annotates first: the `sourceRef`, or for a HelmRelease
/// with `spec.chart` the HelmChart the controller made (`status.helmChart`), which pulls the
/// chart from its repository.
pub fn source_to_reconcile(object: &FluxObject) -> Option<ObjectRef> {
    if object.kind == FluxKind::HelmRelease
        && object.raw.pointer("/spec/chart").is_some()
        && let Some(chart) = object
            .raw
            .pointer("/status/helmChart")
            .and_then(Value::as_str)
        && let Some((namespace, name)) = chart.split_once('/')
    {
        return Some(ObjectRef::new(FluxKind::HelmChart, namespace, name));
    }
    object
        .source
        .clone()
        .filter(|s| s.kind.is_some_and(is_reconcilable_source))
}

fn is_reconcilable_source(kind: FluxKind) -> bool {
    kind.is_source() && kind.reconcilable_at("")
}

/// One object to act on.
#[derive(Clone, Debug)]
pub struct Target {
    pub resource: ApiResource,
    pub namespace: String,
    pub name: String,
}

fn api(client: Client, target: &Target) -> Api<DynamicObject> {
    Api::namespaced_with(client, &target.namespace, &target.resource)
}

/// Sends `action`'s patch (or the delete) to `target`.
pub async fn apply(
    client: Client,
    target: &Target,
    action: Action,
    at: &str,
) -> Result<(), OpError> {
    let api = api(client, target);
    match patch(action, at) {
        Some(patch) => {
            api.patch(&target.name, &PatchParams::default(), &Patch::Merge(&patch))
                .await?;
        }
        None => {
            api.delete(&target.name, &DeleteParams::default()).await?;
        }
    }
    Ok(())
}

/// Runs an action: the source first for "with source", then the object.
pub async fn run(
    client: Client,
    target: Target,
    source: Option<Target>,
    action: Action,
) -> Result<(), OpError> {
    let at = now();
    if action == Action::ReconcileWithSource {
        let Some(source) = source else {
            return Err(OpError::Invalid("its source wasn't found".into()));
        };
        apply(client.clone(), &source, Action::Reconcile, &at).await?;
        wait_for_source(client.clone(), &source, &at).await?;
    }
    apply(client, &target, action, &at).await
}

/// Waits until the source's controller handled the request `at` (bounded by [`SOURCE_WAIT`]).
async fn wait_for_source(client: Client, source: &Target, at: &str) -> Result<(), OpError> {
    let api = api(client, source);
    let deadline = tokio::time::Instant::now() + SOURCE_WAIT;
    loop {
        let object = api.get(&source.name).await?;
        let value = serde_json::to_value(&object).unwrap_or(Value::Null);
        if value.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true) {
            return Err(OpError::Invalid(format!(
                "its source {} is suspended; resume it first",
                source.name
            )));
        }
        if let Some(result) = source_handled(&value, at) {
            return result.map_err(OpError::Invalid);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(OpError::Invalid(format!(
                "{} {} didn't handle the reconcile request within {} s; the object wasn't reconciled",
                object
                    .types
                    .as_ref()
                    .map_or("the source", |t| t.kind.as_str()),
                source.name,
                SOURCE_WAIT.as_secs()
            )));
        }
        tokio::time::sleep(SOURCE_POLL).await;
    }
}

/// Whether a source handled the reconcile request `at` (`status.lastHandledReconcileAt`):
/// `None` while it hasn't, then `Ok` when it's ready, else why not (`flux reconcile
/// --with-source` stops there too).
pub fn source_handled(source: &Value, at: &str) -> Option<Result<(), String>> {
    let handled = source
        .pointer("/status/lastHandledReconcileAt")
        .and_then(Value::as_str);
    if handled != Some(at) {
        return None;
    }
    let ready = source
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c["type"].as_str() == Some("Ready")));
    match ready {
        Some(ready) if ready["status"].as_str() == Some("False") => Some(Err(format!(
            "the source failed: {}",
            ready["message"].as_str().unwrap_or("not ready")
        ))),
        _ => Some(Ok(())),
    }
}

/// What deleting a Kustomization or HelmRelease does to what it applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteEffect {
    /// kustomize-controller deletes these inventory entries (objects marked to stay excepted:
    /// see [`inventory::kept_on_delete`]). `wait`: `deletionPolicy: WaitForTermination`.
    Prunes { entries: Vec<Entry>, wait: bool },
    /// helm-controller uninstalls the Helm release (`release`), with these objects.
    Uninstalls {
        release: String,
        entries: Vec<Entry>,
    },
    /// Nothing it applied is removed; why.
    Keeps(String),
    /// Not a Kustomization or HelmRelease.
    NotApplicable,
}

/// What the controller's finalizer removes when `object` is deleted, the way
/// kustomize-controller (`deletionPolicy`, `prune`, suspended) and helm-controller (suspended,
/// installed at all) decide it.
pub fn delete_effect(object: &FluxObject) -> DeleteEffect {
    let entries = inventory::entries(&object.raw).unwrap_or_default();
    match object.kind {
        FluxKind::Kustomization => {
            if object.suspended {
                return DeleteEffect::Keeps(
                    "It's suspended: kustomize-controller skips garbage collection, so what it applied stays in the cluster, unmanaged."
                        .into(),
                );
            }
            let prune = object.raw.pointer("/spec/prune").and_then(Value::as_bool) == Some(true);
            let policy = object
                .raw
                .pointer("/spec/deletionPolicy")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .unwrap_or("MirrorPrune");
            let deletes = match policy {
                "MirrorPrune" => prune,
                "Delete" | "WaitForTermination" => true,
                _ => false,
            };
            if !deletes {
                return DeleteEffect::Keeps(match policy {
                    "MirrorPrune" => {
                        "Prune is off: what it applied stays in the cluster, unmanaged.".into()
                    }
                    "Orphan" => {
                        "deletionPolicy is Orphan: what it applied stays in the cluster, unmanaged."
                            .into()
                    }
                    other => format!(
                        "deletionPolicy {other} isn't one kustomize-controller deletes for: what it applied stays."
                    ),
                });
            }
            if entries.is_empty() {
                return DeleteEffect::Keeps("Nothing was applied yet: nothing to prune.".into());
            }
            DeleteEffect::Prunes {
                entries,
                wait: policy == "WaitForTermination",
            }
        }
        FluxKind::HelmRelease => {
            if object.suspended {
                return DeleteEffect::Keeps(
                    "It's suspended: helm-controller doesn't uninstall the release, which stays installed, unmanaged."
                        .into(),
                );
            }
            let installed = object
                .raw
                .pointer("/status/storageNamespace")
                .and_then(Value::as_str)
                .is_some_and(|ns| !ns.is_empty())
                || crate::details::helm_storage(object).is_some();
            if !installed {
                return DeleteEffect::Keeps(
                    "No release was installed yet: nothing to uninstall.".into(),
                );
            }
            DeleteEffect::Uninstalls {
                release: crate::details::HelmReleaseSpec::parse(object).release_name,
                entries,
            }
        }
        _ => DeleteEffect::NotApplicable,
    }
}

/// The delete dialog's note for `object` (`kept`: inventory entries known to stay).
pub fn delete_note(object: &FluxObject, kept: usize) -> String {
    let kept_note = if kept > 0 {
        format!(
            " {kept} marked to stay (prune or reconcile disabled, or ssa: Ignore) aren't deleted."
        )
    } else {
        " Objects marked kustomize.toolkit.fluxcd.io/prune: disabled (or reconcile: disabled, ssa: Ignore) stay."
            .to_string()
    };
    match delete_effect(object) {
        DeleteEffect::Prunes { entries, wait } => format!(
            "kustomize-controller deletes everything it applied ({}){}.{kept_note}",
            inventory::summary(&entries),
            if wait {
                " and waits until it's gone"
            } else {
                ""
            }
        ),
        DeleteEffect::Uninstalls { release, entries } => format!(
            "helm-controller uninstalls the Helm release {release}{}. Resources annotated helm.sh/resource-policy: keep stay.",
            if entries.is_empty() {
                String::new()
            } else {
                format!(" ({})", inventory::summary(&entries))
            }
        ),
        DeleteEffect::Keeps(why) => why,
        DeleteEffect::NotApplicable if object.kind.is_source() => {
            "Objects that use this source stop getting new revisions.".into()
        }
        DeleteEffect::NotApplicable => "Deletes the object.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use std::sync::Arc;

    #[test]
    fn patches_per_action() {
        let at = "2026-10-07T11:00:00.123456789Z";
        assert_eq!(
            patch(Action::Reconcile, at).unwrap(),
            json!({"metadata": {"annotations": {"reconcile.fluxcd.io/requestedAt": at}}})
        );
        assert_eq!(
            patch(Action::ReconcileWithSource, at),
            patch(Action::Reconcile, at)
        );
        assert_eq!(
            patch(Action::Force, at).unwrap(),
            json!({"metadata": {"annotations": {
                "reconcile.fluxcd.io/requestedAt": at, "reconcile.fluxcd.io/forceAt": at}}})
        );
        assert_eq!(
            patch(Action::Reset, at).unwrap(),
            json!({"metadata": {"annotations": {
                "reconcile.fluxcd.io/requestedAt": at, "reconcile.fluxcd.io/resetAt": at}}})
        );
        assert_eq!(
            patch(Action::Suspend, at).unwrap(),
            json!({"spec": {"suspend": true}})
        );
        assert_eq!(
            patch(Action::Resume, at).unwrap(),
            json!({"metadata": {"annotations": {"reconcile.fluxcd.io/requestedAt": at}},
                "spec": {"suspend": false}})
        );
        assert_eq!(patch(Action::Delete, at), None);
        // RFC 3339, parseable back.
        assert!(now().parse::<jiff::Timestamp>().is_ok());
    }

    #[test]
    fn which_actions_apply() {
        let parse = |v| FluxObject::parse(&Arc::new(v)).unwrap();
        let ks = parse(fixtures::kustomization_ready());
        assert!(Action::Reconcile.applies_to(&ks));
        assert!(Action::ReconcileWithSource.applies_to(&ks));
        assert!(Action::Suspend.applies_to(&ks) && !Action::Resume.applies_to(&ks));
        assert!(!Action::Force.applies_to(&ks));
        let paused = parse(fixtures::kustomization_suspended());
        assert!(!Action::Reconcile.applies_to(&paused) && Action::Resume.applies_to(&paused));
        let hr = parse(fixtures::helm_release_v2());
        assert!(Action::Force.applies_to(&hr) && Action::Reset.applies_to(&hr));
        let alert = parse(fixtures::alert());
        assert!(!Action::Reconcile.applies_to(&alert) && Action::Suspend.applies_to(&alert));
        let git = parse(fixtures::git_repository());
        assert!(Action::Reconcile.applies_to(&git));
        assert!(!Action::ReconcileWithSource.applies_to(&git));
    }

    #[test]
    fn sources_to_reconcile_first() {
        let parse = |v| FluxObject::parse(&Arc::new(v)).unwrap();
        let ks = parse(fixtures::kustomization_ready());
        assert_eq!(
            source_to_reconcile(&ks).unwrap().label("flux-demo"),
            "GitRepository/podinfo"
        );
        // A HelmRelease with spec.chart: its HelmChart.
        let hr = parse(fixtures::helm_release_v2());
        assert_eq!(
            source_to_reconcile(&hr).unwrap().label("flux-demo"),
            "HelmChart/flux-demo-podinfo-helm"
        );
        // With spec.chartRef: the OCIRepository.
        let oci = parse(fixtures::helm_release_chart_ref());
        assert_eq!(
            source_to_reconcile(&oci).unwrap().kind,
            Some(FluxKind::OCIRepository)
        );
    }

    fn parse(value: Value) -> FluxObject {
        FluxObject::parse(&Arc::new(value)).unwrap()
    }

    /// kustomize-controller's `finalizerShouldDeleteResources` and helm-controller's
    /// `reconcileDelete`.
    #[test]
    fn what_deleting_removes() {
        let prunes = |value: Value| match delete_effect(&parse(value)) {
            DeleteEffect::Prunes { entries, wait } => Some((entries.len(), wait)),
            _ => None,
        };
        assert_eq!(prunes(fixtures::kustomization_ready()), Some((3, false)));
        // Suspended: no garbage collection.
        let mut suspended = fixtures::kustomization_ready();
        suspended["spec"]["suspend"] = true.into();
        assert!(matches!(
            delete_effect(&parse(suspended.clone())),
            DeleteEffect::Keeps(why) if why.contains("suspended")
        ));
        // deletionPolicy: Delete prunes even with prune off; Orphan keeps with prune on.
        let mut delete = fixtures::kustomization_ready();
        delete["spec"]["prune"] = false.into();
        delete["spec"]["deletionPolicy"] = "Delete".into();
        assert_eq!(prunes(delete), Some((3, false)));
        let mut wait = fixtures::kustomization_ready();
        wait["spec"]["deletionPolicy"] = "WaitForTermination".into();
        assert_eq!(prunes(wait), Some((3, true)));
        let mut orphan = fixtures::kustomization_ready();
        orphan["spec"]["deletionPolicy"] = "Orphan".into();
        assert!(matches!(
            delete_effect(&parse(orphan)),
            DeleteEffect::Keeps(why) if why.contains("Orphan")
        ));
        let mut off = fixtures::kustomization_ready();
        off["spec"]["prune"] = false.into();
        assert!(matches!(
            delete_effect(&parse(off)),
            DeleteEffect::Keeps(why) if why.contains("Prune is off")
        ));
        // Nothing applied.
        assert!(matches!(
            delete_effect(&parse(fixtures::kustomization_failed())),
            DeleteEffect::Keeps(why) if why.contains("Nothing was applied")
        ));
        // HelmReleases: uninstalled unless suspended or never installed.
        assert!(matches!(
            delete_effect(&parse(fixtures::helm_release_v2())),
            DeleteEffect::Uninstalls { release, entries } if release == "podinfo-helm" && entries.len() == 2
        ));
        let mut hr = fixtures::helm_release_v2();
        hr["spec"]["suspend"] = true.into();
        assert!(matches!(delete_effect(&parse(hr)), DeleteEffect::Keeps(_)));
        let fresh = json!({"apiVersion": "helm.toolkit.fluxcd.io/v2", "kind": "HelmRelease",
            "metadata": {"name": "new", "namespace": "a"}, "spec": {}});
        assert!(
            matches!(delete_effect(&parse(fresh)), DeleteEffect::Keeps(why) if why.contains("No release"))
        );
        assert_eq!(
            delete_effect(&parse(fixtures::git_repository())),
            DeleteEffect::NotApplicable
        );
        // Notes.
        let note = delete_note(&parse(fixtures::kustomization_ready()), 0);
        assert!(note.contains("(1 Deployment, 1 Service, 1 HorizontalPodAutoscaler)"));
        assert!(note.contains("prune: disabled"));
        assert!(
            delete_note(&parse(fixtures::kustomization_ready()), 2).contains("2 marked to stay")
        );
        assert!(delete_note(&parse(suspended), 0).starts_with("It's suspended"));
        assert!(
            delete_note(&parse(fixtures::helm_release_v2()), 0).contains("resource-policy: keep")
        );
    }

    #[test]
    fn with_source_waits_for_the_handled_request() {
        let at = "2026-10-07T11:00:00Z";
        let mut source = fixtures::git_repository();
        assert_eq!(source_handled(&source, at), None);
        source["status"]["lastHandledReconcileAt"] = "2026-10-07T10:00:00Z".into();
        assert_eq!(source_handled(&source, at), None);
        source["status"]["lastHandledReconcileAt"] = at.into();
        assert_eq!(source_handled(&source, at), Some(Ok(())));
        source["status"]["conditions"][0]["status"] = "False".into();
        source["status"]["conditions"][0]["message"] = "auth failed".into();
        assert_eq!(
            source_handled(&source, at),
            Some(Err("the source failed: auth failed".into()))
        );
    }

    #[test]
    fn static_objects_take_no_reconcile() {
        let oci = parse(fixtures::helm_repository_oci());
        assert!(!Action::Reconcile.applies_to(&oci));
        assert!(Action::Suspend.applies_to(&oci));
        assert!(Action::Reconcile.applies_to(&parse(fixtures::alert_v1beta2())));
    }
}
