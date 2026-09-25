//! Kubernetes mode: Argo CD actions done by patching the `Application` with the user's own
//! Kubernetes access, the way the Argo CD API server (and `argocd … --core`) does it:
//!
//! - refresh: the `argocd.argoproj.io/refresh` annotation (`normal` / `hard`);
//! - sync and rollback: the `operation` field, built like `Server.Sync` / `Server.Rollback`
//!   (identical in Argo CD 3.4 and 3.5), with the previous `status.operationState` cleared;
//! - terminate: `status.operationState.phase = Terminating`;
//! - auto-sync, prune, self-heal: `spec.syncPolicy.automated`;
//! - delete: the resources finalizer set (cascading) or removed (non-cascading), then delete.
//!
//! Writes carry the object's `resourceVersion`, so a concurrent change fails with a conflict and
//! is retried on the fresh object, never overwritten.

use kube::Client;
use kube::api::{Api, DeleteParams, DynamicObject, Patch, PatchParams};
use kube::discovery::ApiResource;
use serde_json::{Value, json};

use crate::model::{FINALIZER, FINALIZER_BACKGROUND, REFRESH_ANNOTATION};

/// Argo CD's foreground-policy variant of the resources finalizer.
const FINALIZER_FOREGROUND: &str = "resources-finalizer.argocd.argoproj.io/foreground";
/// Conflicts retried before giving up.
const RETRIES: usize = 5;

/// Why an action failed, in words for the user.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OpError {
    /// Kubernetes RBAC (or Argo CD's, in API mode) said no.
    #[error("permission denied: {0}")]
    Forbidden(String),
    #[error("another operation is already in progress")]
    AnotherOperation,
    #[error("no operation is in progress")]
    NoOperation,
    #[error("the application doesn't exist anymore")]
    NotFound,
    #[error("the application is being deleted")]
    Deleting,
    #[error("rollback needs auto-sync off")]
    AutoSyncOn,
    #[error("{0}")]
    Invalid(String),
    /// Another write kept winning.
    #[error("the application kept changing; try again")]
    Conflict,
    #[error("{0}")]
    Api(String),
}

impl From<kube::Error> for OpError {
    fn from(err: kube::Error) -> Self {
        match err {
            kube::Error::Api(status) => match status.code {
                403 => OpError::Forbidden(status.message),
                404 => OpError::NotFound,
                409 => OpError::Conflict,
                _ => OpError::Api(status.message),
            },
            other => OpError::Api(other.to_string()),
        }
    }
}

/// Which app.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppTarget {
    pub namespace: String,
    pub name: String,
}

impl AppTarget {
    pub fn new(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

fn api(client: Client, resource: &ApiResource, target: &AppTarget) -> Api<DynamicObject> {
    Api::namespaced_with(client, &target.namespace, resource)
}

async fn get(api: &Api<DynamicObject>, target: &AppTarget) -> Result<Value, OpError> {
    let object = api.get(&target.name).await?;
    serde_json::to_value(object).map_err(|e| OpError::Api(e.to_string()))
}

fn resource_version(app: &Value) -> Value {
    app.pointer("/metadata/resourceVersion")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Applies `build(fresh object)` as a JSON merge patch that is conditional on the object's
/// resource version, re-reading and retrying on conflicts.
async fn patch_fresh(
    client: Client,
    resource: &ApiResource,
    target: &AppTarget,
    build: impl Fn(&Value) -> Result<Option<Value>, OpError>,
) -> Result<(), OpError> {
    let api = api(client, resource, target);
    for _ in 0..RETRIES {
        let app = get(&api, target).await?;
        let Some(mut patch) = build(&app)? else {
            return Ok(());
        };
        patch["metadata"]["resourceVersion"] = resource_version(&app);
        match api
            .patch(&target.name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
        {
            Ok(_) => return Ok(()),
            Err(kube::Error::Api(status)) if status.code == 409 => continue,
            Err(err) => return Err(err.into()),
        }
    }
    Err(OpError::Conflict)
}

// ----- Refresh -----

/// Asks the controller to compare the app again (`hard`: also regenerate manifests, skipping
/// the repo server's cache).
pub async fn refresh(
    client: Client,
    resource: ApiResource,
    target: AppTarget,
    hard: bool,
) -> Result<(), OpError> {
    let patch = json!({
        "metadata": {"annotations": {REFRESH_ANNOTATION: if hard { "hard" } else { "normal" }}}
    });
    api(client, &resource, &target)
        .patch(&target.name, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

// ----- Sync and rollback -----

/// A resource to sync selectively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncResource {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
}

/// What the Sync dialog asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncRequest {
    /// Revision per source (index = source position); `None` = the source's target revision.
    pub revisions: Vec<Option<String>>,
    pub prune: bool,
    pub dry_run: bool,
    /// Sync strategy `apply` (skip hooks) instead of `hook`.
    pub apply_only: bool,
    pub force: bool,
    /// The sync options to use; `None` = the app's own (`spec.syncPolicy.syncOptions`), like
    /// the API server. The dialog starts from the app's own and passes its edited list.
    pub sync_options: Option<Vec<String>>,
    /// Selective sync; empty = everything.
    pub resources: Vec<SyncResource>,
}

/// `Replace=true` / `ServerSideApply=true` in the options, turned on or off.
pub fn set_option(options: &mut Vec<String>, key: &str, on: bool) {
    options.retain(|o| o.split_once('=').map(|(k, _)| k) != Some(key));
    if on {
        options.push(format!("{key}=true"));
    }
}

/// Whether an option is on (`Replace=true`).
pub fn has_option(options: &[String], key: &str) -> bool {
    options.iter().any(|o| {
        o.split_once('=')
            .is_some_and(|(k, v)| k == key && v.eq_ignore_ascii_case("true"))
    })
}

fn auto_sync_on(app: &Value) -> bool {
    match app.pointer("/spec/syncPolicy/automated") {
        Some(Value::Object(automated)) => automated
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        _ => false,
    }
}

fn sources_of(app: &Value) -> Vec<Value> {
    match app.pointer("/spec/sources").and_then(Value::as_array) {
        Some(sources) if !sources.is_empty() => sources.clone(),
        _ => app.pointer("/spec/source").cloned().into_iter().collect(),
    }
}

fn target_revision(source: &Value) -> String {
    source
        .get("targetRevision")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
        .unwrap_or("HEAD")
        .to_string()
}

fn initiator(username: &str) -> Value {
    json!({"username": username})
}

/// The `operation` for a sync, like Argo CD's `Server.Sync`.
pub fn sync_operation(
    app: &Value,
    request: &SyncRequest,
    username: &str,
) -> Result<Value, OpError> {
    if app.pointer("/metadata/deletionTimestamp").is_some() {
        return Err(OpError::Deleting);
    }
    let multi = app
        .pointer("/spec/sources")
        .and_then(Value::as_array)
        .is_some_and(|s| !s.is_empty());
    let sources = sources_of(app);
    let revision_for = |index: usize, source: &Value| -> String {
        request
            .revisions
            .get(index)
            .cloned()
            .flatten()
            .filter(|r| !r.trim().is_empty())
            .map(|r| r.trim().to_string())
            .unwrap_or_else(|| target_revision(source))
    };
    // Argo CD refuses another revision than the target while auto-sync is on.
    if auto_sync_on(app) && !request.dry_run {
        for (index, source) in sources.iter().enumerate() {
            let revision = revision_for(index, source);
            if revision != target_revision(source) {
                return Err(OpError::Invalid(format!(
                    "Cannot sync to {revision}: auto-sync is on and set to {}",
                    target_revision(source)
                )));
            }
        }
    }
    let mut sync = json!({});
    if multi {
        let revisions: Vec<String> = sources
            .iter()
            .enumerate()
            .map(|(i, s)| revision_for(i, s))
            .collect();
        sync["sources"] = Value::Array(sources.clone());
        sync["revisions"] = json!(revisions);
    } else if let Some(source) = sources.first() {
        sync["source"] = source.clone();
        sync["revision"] = json!(revision_for(0, source));
    }
    if request.prune {
        sync["prune"] = json!(true);
    }
    if request.dry_run {
        sync["dryRun"] = json!(true);
    }
    if request.apply_only {
        sync["syncStrategy"] = json!({"apply": {"force": request.force}});
    } else if request.force {
        sync["syncStrategy"] = json!({"hook": {"force": true}});
    }
    let options = match &request.sync_options {
        Some(options) => options.clone(),
        None => app
            .pointer("/spec/syncPolicy/syncOptions")
            .and_then(Value::as_array)
            .map(|o| {
                o.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    };
    if !options.is_empty() {
        sync["syncOptions"] = json!(options);
    }
    if !request.resources.is_empty() {
        sync["resources"] = Value::Array(
            request
                .resources
                .iter()
                .map(|r| {
                    json!({"group": r.group, "kind": r.kind, "namespace": r.namespace, "name": r.name})
                })
                .collect(),
        );
    }
    let mut operation = json!({"sync": sync, "initiatedBy": initiator(username)});
    if let Some(retry) = app.pointer("/spec/syncPolicy/retry") {
        operation["retry"] = retry.clone();
    }
    Ok(operation)
}

/// The `operation` for a rollback to history entry `id`, like Argo CD's `Server.Rollback`.
pub fn rollback_operation(
    app: &Value,
    id: i64,
    prune: bool,
    dry_run: bool,
    username: &str,
) -> Result<Value, OpError> {
    if app.pointer("/metadata/deletionTimestamp").is_some() {
        return Err(OpError::Deleting);
    }
    if auto_sync_on(app) {
        return Err(OpError::AutoSyncOn);
    }
    let history = app
        .pointer("/status/history")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let Some(entry) = history
        .iter()
        .find(|h| h.get("id").and_then(Value::as_i64) == Some(id))
    else {
        return Err(OpError::Invalid(format!(
            "the application has no deployment with id {id}"
        )));
    };
    let source = entry.get("source").filter(|s| {
        s.get("repoURL")
            .and_then(Value::as_str)
            .is_some_and(|r| !r.is_empty())
    });
    let sources = entry
        .get("sources")
        .and_then(Value::as_array)
        .filter(|s| !s.is_empty());
    if source.is_none() && sources.is_none() {
        return Err(OpError::Invalid(
            "cannot roll back to a revision deployed with Argo CD 0.11 or older; sync to its revision instead".into(),
        ));
    }
    let mut sync = json!({"syncStrategy": {"apply": {}}});
    if let Some(revision) = entry
        .get("revision")
        .and_then(Value::as_str)
        .filter(|r| !r.is_empty())
    {
        sync["revision"] = json!(revision);
    }
    if let Some(revisions) = entry
        .get("revisions")
        .filter(|r| r.as_array().is_some_and(|r| !r.is_empty()))
    {
        sync["revisions"] = revisions.clone();
    }
    match (sources, source) {
        (Some(sources), _) => sync["sources"] = Value::Array(sources.clone()),
        (None, Some(source)) => sync["source"] = source.clone(),
        (None, None) => {}
    }
    if prune {
        sync["prune"] = json!(true);
    }
    if dry_run {
        sync["dryRun"] = json!(true);
    }
    if let Some(options) = app
        .pointer("/spec/syncPolicy/syncOptions")
        .filter(|o| o.as_array().is_some_and(|o| !o.is_empty()))
    {
        sync["syncOptions"] = options.clone();
    }
    Ok(json!({"sync": sync, "initiatedBy": initiator(username)}))
}

/// Starts an operation built from the fresh object (like `argo.SetAppOperation`): fails if one
/// is already requested, clears the last `operationState` so the controller starts anew.
pub async fn start_operation(
    client: Client,
    resource: ApiResource,
    target: AppTarget,
    build: impl Fn(&Value) -> Result<Value, OpError>,
) -> Result<(), OpError> {
    patch_fresh(client, &resource, &target, |app| {
        if app.get("operation").is_some_and(|o| !o.is_null()) {
            return Err(OpError::AnotherOperation);
        }
        let operation = build(app)?;
        Ok(Some(json!({
            "operation": operation,
            "status": {"operationState": null},
        })))
    })
    .await
}

/// Terminates the running operation (`status.operationState.phase = Terminating`).
pub async fn terminate(
    client: Client,
    resource: ApiResource,
    target: AppTarget,
) -> Result<(), OpError> {
    patch_fresh(client, &resource, &target, |app| {
        let running = app.get("operation").is_some_and(|o| !o.is_null())
            && app
                .pointer("/status/operationState")
                .is_some_and(|s| !s.is_null());
        if !running {
            return Err(OpError::NoOperation);
        }
        Ok(Some(
            json!({"status": {"operationState": {"phase": "Terminating"}}}),
        ))
    })
    .await
}

// ----- Sync policy -----

/// A change to `spec.syncPolicy.automated`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyChange {
    AutoSync(bool),
    Prune(bool),
    SelfHeal(bool),
}

/// The merge patch for a policy change (`None`: nothing to do).
pub fn policy_patch(app: &Value, change: PolicyChange) -> Result<Option<Value>, OpError> {
    let automated = app
        .pointer("/spec/syncPolicy/automated")
        .filter(|a| a.is_object());
    // Apps that use `enabled` (Argo CD 3.1+) keep their prune/self-heal settings when off.
    let uses_enabled = automated.is_some_and(|a| a.get("enabled").is_some());
    let on = auto_sync_on(app);
    let patch = match change {
        PolicyChange::AutoSync(true) if on => return Ok(None),
        PolicyChange::AutoSync(true) if uses_enabled => {
            json!({"spec": {"syncPolicy": {"automated": {"enabled": true}}}})
        }
        PolicyChange::AutoSync(true) => {
            json!({"spec": {"syncPolicy": {"automated": {"prune": false, "selfHeal": false}}}})
        }
        PolicyChange::AutoSync(false) if !on => return Ok(None),
        PolicyChange::AutoSync(false) if uses_enabled => {
            json!({"spec": {"syncPolicy": {"automated": {"enabled": false}}}})
        }
        PolicyChange::AutoSync(false) => json!({"spec": {"syncPolicy": {"automated": null}}}),
        PolicyChange::Prune(_) | PolicyChange::SelfHeal(_) if !on => {
            return Err(OpError::Invalid("turn on auto-sync first".into()));
        }
        PolicyChange::Prune(value) => {
            json!({"spec": {"syncPolicy": {"automated": {"prune": value}}}})
        }
        PolicyChange::SelfHeal(value) => {
            json!({"spec": {"syncPolicy": {"automated": {"selfHeal": value}}}})
        }
    };
    Ok(Some(patch))
}

pub async fn set_policy(
    client: Client,
    resource: ApiResource,
    target: AppTarget,
    change: PolicyChange,
) -> Result<(), OpError> {
    patch_fresh(client, &resource, &target, |app| policy_patch(app, change)).await
}

// ----- Delete -----

/// How deleting an app treats its resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cascade {
    /// Delete the resources first (foreground), then the app.
    Foreground,
    /// Delete the app, and its resources in the background.
    Background,
    /// Keep the resources; only the app goes.
    None,
}

/// The finalizers after choosing `cascade`, or `None` if they stay as they are (Argo CD's
/// `Server.Delete`). Argo CD deletes with the first propagation finalizer it finds, so there's
/// only ever one, like its `SetCascadedDeletion` (the plain one and `/foreground` both mean
/// foreground).
pub fn finalizers_for(current: &[String], cascade: Cascade, deleting: bool) -> Option<Vec<String>> {
    let cascading =
        |f: &String| f == FINALIZER || f == FINALIZER_BACKGROUND || f == FINALIZER_FOREGROUND;
    match cascade {
        Cascade::Foreground | Cascade::Background => {
            let (wanted, accepted): (&str, &[&str]) = if cascade == Cascade::Background {
                (FINALIZER_BACKGROUND, &[FINALIZER_BACKGROUND])
            } else {
                (FINALIZER, &[FINALIZER, FINALIZER_FOREGROUND])
            };
            let policies: Vec<&String> = current.iter().filter(|f| cascading(f)).collect();
            let already = policies.len() == 1 && accepted.contains(&policies[0].as_str());
            // Kubernetes forbids adding finalizers to an object that is being deleted.
            if already || deleting {
                return None;
            }
            let mut next: Vec<String> = current.iter().filter(|f| !cascading(f)).cloned().collect();
            next.push(wanted.to_string());
            Some(next)
        }
        Cascade::None => {
            if !current.iter().any(cascading) {
                return None;
            }
            Some(current.iter().filter(|f| !cascading(f)).cloned().collect())
        }
    }
}

/// Deletes the app: sets or removes the resources finalizer for `cascade`, then deletes.
pub async fn delete(
    client: Client,
    resource: ApiResource,
    target: AppTarget,
    cascade: Cascade,
) -> Result<(), OpError> {
    patch_fresh(client.clone(), &resource, &target, |app| {
        let current: Vec<String> = app
            .pointer("/metadata/finalizers")
            .and_then(Value::as_array)
            .map(|f| {
                f.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let deleting = app.pointer("/metadata/deletionTimestamp").is_some();
        Ok(finalizers_for(&current, cascade, deleting)
            .map(|finalizers| json!({"metadata": {"finalizers": finalizers}})))
    })
    .await?;
    match api(client, &resource, &target)
        .delete(&target.name, &DeleteParams::default())
        .await
    {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(status)) if status.code == 404 => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::guestbook;

    #[test]
    fn sync_operation_like_the_api_server() {
        let app = guestbook();
        // No options given: the app's own, like the API server.
        let request = SyncRequest {
            prune: true,
            ..Default::default()
        };
        let op = sync_operation(&app, &request, "alice").unwrap();
        assert_eq!(
            op,
            json!({
                "sync": {
                    "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps.git",
                               "path": "guestbook", "targetRevision": "HEAD"},
                    "revision": "HEAD",
                    "prune": true,
                    "syncOptions": ["CreateNamespace=true"]
                },
                "initiatedBy": {"username": "alice"}
            })
        );
        let request = SyncRequest {
            revisions: vec![Some("68657670".into())],
            dry_run: true,
            apply_only: true,
            force: true,
            resources: vec![SyncResource {
                group: "apps".into(),
                kind: "Deployment".into(),
                namespace: "guestbook".into(),
                name: "guestbook-ui".into(),
            }],
            sync_options: Some(Vec::new()),
            ..Default::default()
        };
        let op = sync_operation(&app, &request, "alice").unwrap();
        assert_eq!(op["sync"]["revision"], "68657670");
        assert_eq!(op["sync"]["dryRun"], true);
        assert_eq!(
            op["sync"]["syncStrategy"],
            json!({"apply": {"force": true}})
        );
        assert_eq!(op["sync"]["resources"][0]["kind"], "Deployment");
        assert!(op["sync"].get("syncOptions").is_none());
    }

    #[test]
    fn sync_respects_auto_sync_and_multi_source() {
        let mut app = guestbook();
        app["spec"]["syncPolicy"]["automated"] = json!({"prune": true});
        app["spec"]["syncPolicy"]["retry"] = json!({"limit": 2});
        let other = SyncRequest {
            revisions: vec![Some("v1".into())],
            ..Default::default()
        };
        assert!(matches!(
            sync_operation(&app, &other, "a"),
            Err(OpError::Invalid(_))
        ));
        // A dry run may use another revision.
        let dry = SyncRequest {
            dry_run: true,
            ..other
        };
        let op = sync_operation(&app, &dry, "a").unwrap();
        assert_eq!(op["retry"], json!({"limit": 2}));

        let mut multi = guestbook();
        multi["spec"]["source"] = Value::Null;
        multi["spec"]["sources"] = json!([
            {"repoURL": "https://charts.example.com", "chart": "web", "targetRevision": "1.2.0"},
            {"repoURL": "https://github.com/acme/values.git", "ref": "values"}
        ]);
        let request = SyncRequest {
            revisions: vec![None, Some("abc1234".into())],
            ..Default::default()
        };
        let op = sync_operation(&multi, &request, "a").unwrap();
        assert!(op["sync"].get("source").is_none());
        assert_eq!(op["sync"]["revisions"], json!(["1.2.0", "abc1234"]));
        assert_eq!(op["sync"]["sources"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn rollback_operation_like_the_api_server() {
        let app = guestbook();
        let op = rollback_operation(&app, 0, false, false, "bob").unwrap();
        assert_eq!(
            op,
            json!({
                "sync": {
                    "revision": "68657670d9131dc5bc5f538b14c1de3377d74591",
                    "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps.git",
                               "path": "guestbook", "targetRevision": "HEAD"},
                    "syncStrategy": {"apply": {}},
                    "syncOptions": ["CreateNamespace=true"]
                },
                "initiatedBy": {"username": "bob"}
            })
        );
        assert!(matches!(
            rollback_operation(&app, 7, false, false, "bob"),
            Err(OpError::Invalid(_))
        ));
        let mut auto = app.clone();
        auto["spec"]["syncPolicy"]["automated"] = json!({});
        assert_eq!(
            rollback_operation(&auto, 0, false, false, "bob"),
            Err(OpError::AutoSyncOn)
        );
        // `automated: {enabled: false}` is off.
        auto["spec"]["syncPolicy"]["automated"] = json!({"enabled": false});
        assert!(rollback_operation(&auto, 0, true, true, "bob").is_ok());
    }

    #[test]
    fn policy_patches() {
        let mut app = guestbook();
        assert_eq!(
            policy_patch(&app, PolicyChange::AutoSync(true)).unwrap(),
            Some(
                json!({"spec": {"syncPolicy": {"automated": {"prune": false, "selfHeal": false}}}})
            )
        );
        assert_eq!(
            policy_patch(&app, PolicyChange::AutoSync(false)).unwrap(),
            None
        );
        assert!(policy_patch(&app, PolicyChange::Prune(true)).is_err());
        app["spec"]["syncPolicy"]["automated"] = json!({"prune": true});
        assert_eq!(
            policy_patch(&app, PolicyChange::AutoSync(false)).unwrap(),
            Some(json!({"spec": {"syncPolicy": {"automated": null}}}))
        );
        assert_eq!(
            policy_patch(&app, PolicyChange::SelfHeal(true)).unwrap(),
            Some(json!({"spec": {"syncPolicy": {"automated": {"selfHeal": true}}}}))
        );
        app["spec"]["syncPolicy"]["automated"] = json!({"enabled": true, "prune": true});
        assert_eq!(
            policy_patch(&app, PolicyChange::AutoSync(false)).unwrap(),
            Some(json!({"spec": {"syncPolicy": {"automated": {"enabled": false}}}}))
        );
    }

    #[test]
    fn delete_finalizers() {
        let none: Vec<String> = vec![];
        assert_eq!(
            finalizers_for(&none, Cascade::Foreground, false),
            Some(vec![FINALIZER.to_string()])
        );
        assert_eq!(
            finalizers_for(&[FINALIZER.to_string()], Cascade::Foreground, false),
            None
        );
        assert_eq!(finalizers_for(&none, Cascade::Foreground, true), None);
        assert_eq!(
            finalizers_for(&none, Cascade::Background, false),
            Some(vec![FINALIZER_BACKGROUND.to_string()])
        );
        let mixed = vec![FINALIZER.to_string(), "other/finalizer".to_string()];
        assert_eq!(
            finalizers_for(&mixed, Cascade::None, false),
            Some(vec!["other/finalizer".to_string()])
        );
        assert_eq!(finalizers_for(&none, Cascade::None, false), None);

        // One propagation finalizer: switching replaces it (Argo CD uses the first it finds).
        assert_eq!(
            finalizers_for(&mixed, Cascade::Background, false),
            Some(vec![
                "other/finalizer".to_string(),
                FINALIZER_BACKGROUND.to_string()
            ])
        );
        let background = vec![FINALIZER_BACKGROUND.to_string()];
        assert_eq!(
            finalizers_for(&background, Cascade::Background, false),
            None
        );
        assert_eq!(
            finalizers_for(&background, Cascade::Foreground, false),
            Some(vec![FINALIZER.to_string()])
        );
        let foreground = vec![FINALIZER_FOREGROUND.to_string()];
        assert_eq!(
            finalizers_for(&foreground, Cascade::Foreground, false),
            None
        );
        let both = vec![FINALIZER.to_string(), FINALIZER_BACKGROUND.to_string()];
        assert_eq!(
            finalizers_for(&both, Cascade::Foreground, false),
            Some(vec![FINALIZER.to_string()])
        );
    }

    #[test]
    fn options() {
        let mut options = vec![
            "CreateNamespace=true".to_string(),
            "Replace=false".to_string(),
        ];
        assert!(!has_option(&options, "Replace"));
        set_option(&mut options, "Replace", true);
        set_option(&mut options, "ServerSideApply", true);
        assert!(has_option(&options, "Replace") && has_option(&options, "ServerSideApply"));
        set_option(&mut options, "ServerSideApply", false);
        assert_eq!(options, ["CreateNamespace=true", "Replace=true"]);
    }
}
