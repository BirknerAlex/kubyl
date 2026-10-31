//! The change tools: apply a manifest, patch, delete, scale and restart.
//!
//! A change tool never acts on its own. It checks the request against the live cluster (a
//! server-side dry run for manifests and patches, so a broken change is an error the agent
//! sees, not a prompt the user sees), turns it into a [`Proposal`] with a diff of masked YAML,
//! and acts only when the [`Approver`] says the user approved. Secrets are never written
//! through these tools, so no credential has to pass through an agent. Applies use server-side
//! apply with field manager `kubyl` and never force over another manager's fields.

use std::sync::Arc;
use std::time::Duration;

use kube::api::{Api, DeleteParams, DynamicObject, Patch, PatchParams, Preconditions};
use kube::discovery::ApiResource;
use kubyl_kube_core::discovery::ApiResourceInfo;
use kubyl_resources_core::ops::FIELD_MANAGER;
use kubyl_resources_core::{format, redact, store};
use kubyl_yaml_core::apply::{self, DocResult, Options, Outcome, Prepared};
use kubyl_yaml_core::{parse, render};
use serde_json::{Value, json};

use crate::approval::{Approver, ChangeDoc, Decision, Proposal};
use crate::tools::{
    ToolContext, ToolOutput, api_for, client, int_arg, kube_error, refuse_helm_release, required,
    resolve, str_arg, typed_json,
};

/// The names of the change tools.
pub const CHANGE_TOOLS: &[&str] = &[
    "apply_manifest",
    "patch_resource",
    "delete_resource",
    "scale_resource",
    "restart_resource",
];

/// How long checking a change or carrying it out may take (waiting for the user is separate).
const STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// Most objects one `apply_manifest` call may touch.
const MAX_DOCS: usize = 20;

pub fn is_change_tool(name: &str) -> bool {
    CHANGE_TOOLS.contains(&name)
}

/// The change tools for `tools/list`.
pub fn definitions() -> Vec<Value> {
    let tool = |name: &str,
                description: &str,
                properties: Value,
                required: &[&str],
                destructive: bool| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": destructive, "openWorldHint": false },
        })
    };
    let kind = json!({"type": "string", "description": "Kind, plural or short name: `deploy`, `Deployment`, `certificates.cert-manager.io`."});
    let namespace = json!({"type": "string", "description": "Namespace. Defaults to the namespace Kubyl shows."});
    let name = json!({"type": "string", "description": "Object name."});
    vec![
        tool(
            "apply_manifest",
            "Create or update objects from YAML (server-side apply, like `kubectl apply --server-side`). Kubyl checks it with a dry run, shows the user the diff and applies it only after they approve. Secrets can't be written through Kubyl.",
            json!({
                "yaml": {"type": "string", "description": "One or more YAML documents (at most 20)."},
                "namespace": {"type": "string", "description": "Namespace for namespaced objects that don't set one."},
            }),
            &["yaml"],
            false,
        ),
        tool(
            "patch_resource",
            "Change fields of one object with a patch. Checked with a dry run; applied after the user approves the diff.",
            json!({
                "kind": kind, "name": name, "namespace": namespace,
                "patch": {"type": "string", "description": "The patch as JSON: an object for `merge` and `strategic`, an RFC 6902 array for `json`."},
                "patch_type": {"type": "string", "enum": ["merge", "strategic", "json"], "description": "Default `merge` (JSON merge patch)."},
            }),
            &["kind", "name", "patch"],
            false,
        ),
        tool(
            "delete_resource",
            "Delete one object. The user sees what goes away and must approve.",
            json!({ "kind": kind, "name": name, "namespace": namespace }),
            &["kind", "name"],
            true,
        ),
        tool(
            "scale_resource",
            "Set the replica count of a workload (anything with a `scale` subresource). The user must approve.",
            json!({
                "kind": kind, "name": name, "namespace": namespace,
                "replicas": {"type": "integer", "minimum": 0, "maximum": 10000},
            }),
            &["kind", "name", "replicas"],
            false,
        ),
        tool(
            "restart_resource",
            "Rolling restart of a Deployment, StatefulSet or DaemonSet, like `kubectl rollout restart`. The user must approve.",
            json!({ "kind": kind, "name": name, "namespace": namespace }),
            &["kind", "name"],
            false,
        ),
    ]
}

/// What a checked change will do once approved.
#[derive(Debug)]
enum Action {
    Apply(Vec<Prepared>),
    Patch {
        resource: ApiResource,
        namespace: Option<String>,
        name: String,
        patch_type: String,
        patch: Value,
        /// The version the user reviewed: the patch fails with a conflict if the object moved on.
        resource_version: Option<String>,
    },
    Delete {
        resource: ApiResource,
        namespace: Option<String>,
        name: String,
        /// The object the user reviewed: a replacement with the same name is left alone.
        uid: Option<String>,
    },
    Scale {
        resource: ApiResource,
        namespace: Option<String>,
        name: String,
        replicas: u32,
        /// The version the user reviewed (a conflict if the object moved on).
        resource_version: Option<String>,
    },
    Restart {
        resource: ApiResource,
        namespace: Option<String>,
        name: String,
        /// The version the user reviewed (a conflict if the object moved on).
        resource_version: Option<String>,
    },
}

enum Checked {
    /// Nothing would change.
    Nothing(String),
    Change(Box<(Proposal, Action)>),
}

/// Runs change tool `name`: checks, asks `approver`, acts. Never panics on bad input.
pub async fn run(
    context: &ToolContext,
    approver: &Arc<dyn Approver>,
    name: &str,
    args: &Value,
) -> ToolOutput {
    let output = match change(context, approver, name, args).await {
        Ok(text) => ToolOutput::ok(text),
        Err(message) => ToolOutput::error(message),
    };
    crate::tools::finish(context, false, output)
}

async fn change(
    context: &ToolContext,
    approver: &Arc<dyn Approver>,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let checked = tokio::time::timeout(STEP_TIMEOUT, check(context, name, args))
        .await
        .map_err(|_| {
            format!(
                "Checking {name} took longer than {}s.",
                STEP_TIMEOUT.as_secs()
            )
        })??;
    let (proposal, action) = match checked {
        Checked::Nothing(text) => return Ok(text),
        Checked::Change(change) => *change,
    };
    let title = proposal.title.clone();
    match approver.approve(proposal).await {
        Decision::Approved => {}
        Decision::Rejected => {
            return Err(format!(
                "The user did not approve this change in Kubyl, so nothing was changed ({title}). Don't repeat it; ask them what they would prefer."
            ));
        }
    }
    let client = client(context)?;
    tokio::time::timeout(STEP_TIMEOUT, execute(client, action))
        .await
        .map_err(|_| {
            format!(
                "{title} took longer than {}s; check the cluster for its state.",
                STEP_TIMEOUT.as_secs()
            )
        })?
}

async fn check(context: &ToolContext, name: &str, args: &Value) -> Result<Checked, String> {
    match name {
        "apply_manifest" => check_apply(context, args).await,
        "patch_resource" => check_patch(context, args).await,
        "delete_resource" => check_delete(context, args).await,
        "scale_resource" => check_scale(context, args).await,
        "restart_resource" => check_restart(context, args).await,
        other => Err(format!("Kubyl has no change tool named `{other}`.")),
    }
}

fn proposal(context: &ToolContext, title: String) -> Result<Proposal, String> {
    let cluster = context
        .cluster
        .clone()
        .ok_or_else(|| format!("{} isn't connected in Kubyl.", context.cluster_name))?;
    Ok(Proposal {
        cluster,
        cluster_name: context.cluster_name.clone(),
        production: context.caps.production,
        title,
        docs: Vec::new(),
        warnings: Vec::new(),
    })
}

/// `deployment shop/web`.
fn label(info: &ApiResourceInfo, namespace: Option<&str>, name: &str) -> String {
    let kind = info.gvk.kind.to_lowercase();
    match namespace {
        Some(ns) => format!("{kind} {ns}/{name}"),
        None => format!("{kind} {name}"),
    }
}

fn target<'a>(
    context: &'a ToolContext,
    args: &'a Value,
) -> Result<(&'a ApiResourceInfo, Option<&'a str>, &'a str), String> {
    let info = resolve(context, required(args, "kind")?)?;
    let name = required(args, "name")?;
    let namespace = if info.namespaced {
        Some(
            str_arg(args, "namespace")
                .or(context.namespace.as_deref())
                .ok_or_else(|| format!("`namespace` is required for {}.", info.gvr))?,
        )
    } else {
        None
    };
    Ok((info, namespace, name))
}

fn refuse_secret(info: &ApiResourceInfo, what: &str) -> Result<(), String> {
    if info.gvk.group.is_empty() && info.gvk.kind == "Secret" {
        return Err(format!(
            "Kubyl doesn't {what} Secrets for agents: values would have to pass through the agent. Ask the user to do it in Kubyl or with kubectl."
        ));
    }
    Ok(())
}

/// An object without the fields the server sets and without status: what a diff compares.
fn comparable(mut object: Value, info_kind: (&str, &str)) -> Value {
    if object.get("kind").is_none() {
        object["kind"] = Value::String(info_kind.1.to_string());
    }
    if object.get("apiVersion").is_none() {
        object["apiVersion"] = Value::String(info_kind.0.to_string());
    }
    render::strip_server_fields(&mut object);
    object
}

/// A comparable object as masked YAML.
fn masked_yaml(mut object: Value) -> String {
    redact::mask_object(&mut object);
    format::to_yaml(&object)
}

fn display_yaml(object: Value, info_kind: (&str, &str)) -> String {
    masked_yaml(comparable(object, info_kind))
}

fn live_json(object: Option<DynamicObject>, api_version: &str, kind: &str) -> Option<Value> {
    Some(comparable(store::to_json(&object?)?, (api_version, kind)))
}

fn kind_warnings(kind: &str, action: &str, out: &mut Vec<String>) {
    let warning = match (kind, action) {
        ("Namespace", "delete") => "Deleting a namespace deletes every object in it.",
        ("CustomResourceDefinition", "delete") => {
            "Deleting a CRD deletes every custom resource of that kind."
        }
        ("PersistentVolumeClaim" | "PersistentVolume", "delete") => {
            "Deleting a volume can delete its data, depending on the reclaim policy."
        }
        ("Node", "delete") => "Deleting a node object doesn't drain it; its pods are orphaned.",
        (
            "Namespace"
            | "CustomResourceDefinition"
            | "ClusterRole"
            | "ClusterRoleBinding"
            | "ValidatingWebhookConfiguration"
            | "MutatingWebhookConfiguration",
            "apply" | "patch",
        ) => "This changes something cluster-wide.",
        _ => return,
    };
    out.push(warning.to_string());
}

// ----- apply_manifest -----

async fn check_apply(context: &ToolContext, args: &Value) -> Result<Checked, String> {
    let text = required(args, "yaml")?;
    let discovery = context
        .discovery
        .as_ref()
        .ok_or_else(|| format!("{} isn't connected in Kubyl.", context.cluster_name))?;
    let client = client(context)?;
    let parsed = parse::parse(text);
    let namespace = str_arg(args, "namespace").or(context.namespace.as_deref());
    let docs = apply::prepare(&parsed, discovery, namespace, None).map_err(|problems| {
        let lines: Vec<String> = problems.iter().map(|p| p.message.clone()).collect();
        format!("The YAML can't be applied: {}", lines.join("; "))
    })?;
    if docs.len() > MAX_DOCS {
        return Err(format!(
            "At most {MAX_DOCS} objects per call; this has {}.",
            docs.len()
        ));
    }
    for doc in &docs {
        if doc.gvk.group.is_empty() && doc.gvk.kind == "Secret" {
            return Err(format!(
                "{} is a Secret. Kubyl doesn't write Secrets for agents: values would have to pass through the agent.",
                doc.label()
            ));
        }
        if redact::is_helm_release(&doc.object) {
            return Err(format!(
                "{} is Helm's release storage; use Helm to change a release.",
                doc.label()
            ));
        }
    }

    let dry = apply::apply_all(
        client.clone(),
        docs.clone(),
        Options {
            dry_run: true,
            force: false,
        },
    )
    .await;
    let mut proposal = proposal(
        context,
        match docs.as_slice() {
            [one] => format!("Apply {} {}", one.gvk.kind.to_lowercase(), doc_name(one)),
            many => format!("Apply {} objects", many.len()),
        },
    )?;
    let mut changed = Vec::new();
    for (doc, result) in docs.into_iter().zip(dry) {
        let api_version = doc.gvk.api_version();
        let new = comparable(dry_run_result(&doc, result)?, (&api_version, &doc.gvk.kind));
        let live = doc_api(&client, &doc)
            .get_opt(&doc.name)
            .await
            .map_err(kube_error)?;
        // The manifest may not say it is Helm's storage, but the object it replaces can be.
        if let Some(value) = live.as_ref().and_then(store::to_json)
            && redact::is_helm_release(&value)
        {
            return Err(format!(
                "{} is Helm's release storage; use Helm to change a release.",
                doc.label()
            ));
        }
        let old = live_json(live, &api_version, &doc.gvk.kind);
        // Compare the real values: a changed password masks to the same text on both sides, and
        // it still has to be shown (and approved) as a change.
        if old.as_ref() == Some(&new) {
            continue;
        }
        kind_warnings(
            &doc.gvk.kind,
            if old.is_some() { "patch" } else { "apply" },
            &mut proposal.warnings,
        );
        let (old_text, new_text) = (old.clone().map(masked_yaml), masked_yaml(new));
        if old_text.as_deref() == Some(new_text.as_str()) {
            proposal.warnings.push(format!(
                "A credential-like value of {} changes; it is masked here, so the diff looks the same.",
                doc_name(&doc)
            ));
        }
        proposal.docs.push(ChangeDoc {
            label: format!("{} {}", doc.gvk.kind.to_lowercase(), doc_name(&doc)),
            old: old_text,
            new: new_text,
        });
        changed.push(doc);
    }
    if changed.is_empty() {
        return Ok(Checked::Nothing(
            "Nothing would change: the cluster already matches this YAML.".into(),
        ));
    }
    proposal.warnings.dedup();
    Ok(Checked::Change(Box::new((
        proposal,
        Action::Apply(changed),
    ))))
}

fn doc_name(doc: &Prepared) -> String {
    match &doc.namespace {
        Some(ns) => format!("{ns}/{}", doc.name),
        None => doc.name.clone(),
    }
}

fn doc_api(client: &kube::Client, doc: &Prepared) -> Api<DynamicObject> {
    match &doc.namespace {
        Some(ns) => Api::namespaced_with(client.clone(), ns, &doc.resource),
        None => Api::all_with(client.clone(), &doc.resource),
    }
}

/// The object a dry run produced, or why the server refused it.
fn dry_run_result(doc: &Prepared, result: DocResult) -> Result<Value, String> {
    match result.outcome {
        Outcome::Applied(value) => Ok(value),
        Outcome::Conflicts(conflicts) => {
            let list: Vec<String> = conflicts
                .iter()
                .map(|c| format!("{} owns {}", c.manager, c.field))
                .collect();
            Err(format!(
                "{} conflicts with fields other tools manage ({}). Kubyl never forces over them; change the manifest or ask the user to do it in Kubyl's YAML editor.",
                doc.label(),
                list.join(", ")
            ))
        }
        Outcome::Failed { message, causes } => {
            let mut text = format!("The server rejected {}: {message}", doc.label());
            for cause in causes.iter().take(10) {
                if cause.field.is_empty() {
                    text.push_str(&format!("\n- {}", cause.message));
                } else {
                    text.push_str(&format!("\n- {}: {}", cause.field, cause.message));
                }
            }
            Err(text)
        }
    }
}

// ----- patch_resource -----

fn patch_value(args: &Value) -> Result<Value, String> {
    match args.get("patch") {
        Some(Value::String(text)) => {
            serde_json::from_str(text).map_err(|e| format!("`patch` isn't valid JSON: {e}"))
        }
        Some(value @ (Value::Object(_) | Value::Array(_))) => Ok(value.clone()),
        _ => Err("`patch` is required (JSON).".into()),
    }
}

/// The patch with the version the user reviewed as a precondition, so it conflicts instead of
/// overwriting what changed during the approval.
fn with_precondition(patch_type: &str, mut patch: Value, resource_version: Option<&str>) -> Value {
    let Some(version) = resource_version else {
        return patch;
    };
    match (patch_type, &mut patch) {
        ("json", Value::Array(ops)) => {
            ops.insert(
                0,
                json!({"op": "test", "path": "/metadata/resourceVersion", "value": version}),
            );
        }
        (_, Value::Object(map)) => {
            let metadata = map.entry("metadata").or_insert_with(|| json!({}));
            if let Some(metadata) = metadata.as_object_mut() {
                metadata.insert("resourceVersion".into(), Value::String(version.into()));
            }
        }
        _ => {}
    }
    patch
}

fn build_patch(patch_type: &str, value: Value) -> Result<Patch<Value>, String> {
    match patch_type {
        "merge" => Ok(Patch::Merge(value)),
        "strategic" => Ok(Patch::Strategic(value)),
        "json" => serde_json::from_value(value)
            .map(Patch::Json)
            .map_err(|e| format!("`patch` isn't a JSON patch (RFC 6902): {e}")),
        other => Err(format!(
            "Unknown patch_type `{other}`; use merge, strategic or json."
        )),
    }
}

async fn check_patch(context: &ToolContext, args: &Value) -> Result<Checked, String> {
    let (info, namespace, name) = target(context, args)?;
    refuse_secret(info, "patch")?;
    let value = patch_value(args)?;
    let patch_type = str_arg(args, "patch_type").unwrap_or("merge").to_string();
    let patch = build_patch(&patch_type, value.clone())?;
    let api = api_for(client(context)?, info, namespace);
    let live = api.get(name).await.map_err(kube_error)?;
    if let Some(live) = typed_json(&live, info) {
        refuse_helm_release(info, &live)?;
    }
    let params = PatchParams {
        field_manager: Some(FIELD_MANAGER.into()),
        dry_run: true,
        ..Default::default()
    };
    let result = api
        .patch(name, &params, &patch)
        .await
        .map_err(|e| format!("The server rejected the patch: {}", kube_error(e)))?;
    let api_version = info.gvk.api_version();
    let resource_version = live.metadata.resource_version.clone();
    let old = live_json(Some(live), &api_version, &info.gvk.kind).unwrap_or_default();
    let new = live_json(Some(result), &api_version, &info.gvk.kind).unwrap_or_default();
    if old == new {
        return Ok(Checked::Nothing(
            "Nothing would change: the object already has these values.".into(),
        ));
    }
    let what = label(info, namespace, name);
    let mut proposal = proposal(context, format!("Patch {what}"))?;
    kind_warnings(&info.gvk.kind, "patch", &mut proposal.warnings);
    let (old_text, new_text) = (masked_yaml(old), masked_yaml(new));
    if old_text == new_text {
        proposal.warnings.push(
            "A credential-like value changes; it is masked here, so the diff looks the same."
                .into(),
        );
    }
    proposal.docs.push(ChangeDoc {
        label: what,
        old: Some(old_text),
        new: new_text,
    });
    Ok(Checked::Change(Box::new((
        proposal,
        Action::Patch {
            resource: store::api_resource(info),
            namespace: namespace.map(String::from),
            name: name.to_string(),
            patch_type,
            patch: value,
            resource_version,
        },
    ))))
}

// ----- delete_resource -----

async fn check_delete(context: &ToolContext, args: &Value) -> Result<Checked, String> {
    let (info, namespace, name) = target(context, args)?;
    if !info.supports("delete") {
        return Err(format!("`{}` can't be deleted.", info.gvr));
    }
    let api = api_for(client(context)?, info, namespace);
    let live = api.get(name).await.map_err(kube_error)?;
    if let Some(value) = typed_json(&live, info) {
        refuse_helm_release(info, &value)?;
    }
    let api_version = info.gvk.api_version();
    let uid = live.metadata.uid.clone();
    let old = display_yaml(
        store::to_json(&live).unwrap_or_default(),
        (&api_version, &info.gvk.kind),
    );
    let what = label(info, namespace, name);
    let mut proposal = proposal(context, format!("Delete {what}"))?;
    kind_warnings(&info.gvk.kind, "delete", &mut proposal.warnings);
    proposal.docs.push(ChangeDoc {
        label: what,
        old: Some(old),
        new: String::new(),
    });
    Ok(Checked::Change(Box::new((
        proposal,
        Action::Delete {
            resource: store::api_resource(info),
            namespace: namespace.map(String::from),
            name: name.to_string(),
            uid,
        },
    ))))
}

// ----- scale_resource -----

async fn check_scale(context: &ToolContext, args: &Value) -> Result<Checked, String> {
    let (info, namespace, name) = target(context, args)?;
    if !info.subresources.iter().any(|s| s == "scale") {
        return Err(format!(
            "`{}` has no scale subresource, so it can't be scaled.",
            info.gvr
        ));
    }
    let replicas = int_arg(args, "replicas")
        .filter(|n| (0..=10_000).contains(n))
        .ok_or("`replicas` must be a number from 0 to 10000.")? as u32;
    let api = api_for(client(context)?, info, namespace);
    // The count the scale subresource reports is the one `scale` changes (a custom resource may
    // keep its replicas elsewhere than `.spec.replicas`).
    let scale = api.get_scale(name).await.map_err(kube_error)?;
    let resource_version = scale.metadata.resource_version.clone();
    let current = scale
        .spec
        .and_then(|spec| spec.replicas)
        .unwrap_or(1)
        .max(0) as u32;
    let what = label(info, namespace, name);
    if current == replicas {
        return Ok(Checked::Nothing(format!(
            "Nothing would change: {what} already has {replicas} replicas."
        )));
    }
    let mut proposal = proposal(
        context,
        format!("Scale {what} from {current} to {replicas}"),
    )?;
    if replicas == 0 {
        proposal
            .warnings
            .push("Scaling to 0 stops every pod of this workload.".into());
    }
    proposal.docs.push(ChangeDoc {
        label: what,
        old: Some(format!("spec:\n  replicas: {current}\n")),
        new: format!("spec:\n  replicas: {replicas}\n"),
    });
    Ok(Checked::Change(Box::new((
        proposal,
        Action::Scale {
            resource: store::api_resource(info),
            namespace: namespace.map(String::from),
            name: name.to_string(),
            replicas,
            resource_version,
        },
    ))))
}

// ----- restart_resource -----

async fn check_restart(context: &ToolContext, args: &Value) -> Result<Checked, String> {
    let (info, namespace, name) = target(context, args)?;
    let restartable = info.gvk.group == "apps"
        && matches!(
            info.gvk.kind.as_str(),
            "Deployment" | "StatefulSet" | "DaemonSet"
        );
    if !restartable {
        return Err("Only Deployments, StatefulSets and DaemonSets can be restarted.".into());
    }
    let api = api_for(client(context)?, info, namespace);
    let live = api.get(name).await.map_err(kube_error)?;
    let resource_version = live.metadata.resource_version.clone();
    let current = live
        .data
        .pointer("/spec/template/metadata/annotations/kubectl.kubernetes.io~1restartedAt")
        .and_then(Value::as_str);
    let what = label(info, namespace, name);
    let mut proposal = proposal(context, format!("Restart {what}"))?;
    let annotation = |value: &str| {
        format!(
            "spec:\n  template:\n    metadata:\n      annotations:\n        kubectl.kubernetes.io/restartedAt: {value}\n"
        )
    };
    proposal.docs.push(ChangeDoc {
        label: what.clone(),
        old: Some(match current {
            Some(value) => annotation(value),
            None => "spec:\n  template:\n    metadata:\n      annotations: {}\n".into(),
        }),
        new: annotation("<time of approval>"),
    });
    proposal.warnings.push(
        "Its pods are replaced one by one (a rolling restart); the workload may be briefly degraded."
            .into(),
    );
    Ok(Checked::Change(Box::new((
        proposal,
        Action::Restart {
            resource: store::api_resource(info),
            namespace: namespace.map(String::from),
            name: name.to_string(),
            resource_version,
        },
    ))))
}

// ----- acting -----

async fn execute(client: kube::Client, action: Action) -> Result<String, String> {
    match action {
        Action::Apply(docs) => {
            let results = apply::apply_all(
                client,
                docs,
                Options {
                    dry_run: false,
                    force: false,
                },
            )
            .await;
            let mut lines = Vec::new();
            let mut failed = false;
            for result in results {
                match result.outcome {
                    Outcome::Applied(_) => lines.push(format!("applied {}", result.label)),
                    Outcome::Conflicts(_) => {
                        failed = true;
                        lines.push(format!(
                            "{}: another tool manages fields of it; not forced",
                            result.label
                        ));
                    }
                    Outcome::Failed { message, .. } => {
                        failed = true;
                        lines.push(format!("{}: failed: {message}", result.label));
                    }
                }
            }
            let text = lines.join("\n");
            if failed { Err(text) } else { Ok(text) }
        }
        Action::Patch {
            resource,
            namespace,
            name,
            patch_type,
            patch,
            resource_version,
        } => {
            let api = match &namespace {
                Some(ns) => Api::<DynamicObject>::namespaced_with(client, ns, &resource),
                None => Api::all_with(client, &resource),
            };
            let params = PatchParams {
                field_manager: Some(FIELD_MANAGER.into()),
                ..Default::default()
            };
            let patch = with_precondition(&patch_type, patch, resource_version.as_deref());
            api.patch(&name, &params, &build_patch(&patch_type, patch)?)
                .await
                .map_err(|e| format!("The patch failed: {}", kube_error(e)))?;
            Ok(format!("Patched {name}."))
        }
        Action::Delete {
            resource,
            namespace,
            name,
            uid,
        } => {
            let api = match &namespace {
                Some(ns) => Api::<DynamicObject>::namespaced_with(client, ns, &resource),
                None => Api::all_with(client, &resource),
            };
            let params = DeleteParams {
                propagation_policy: Some(kube::api::PropagationPolicy::Background),
                // Only the object the user reviewed: a replacement with the same name stays.
                preconditions: uid.map(|uid| Preconditions {
                    uid: Some(uid),
                    resource_version: None,
                }),
                ..Default::default()
            };
            match api.delete(&name, &params).await {
                Ok(_) => Ok(format!("Deleted {name}.")),
                Err(kube::Error::Api(status)) if status.code == 404 => {
                    Ok(format!("{name} was already gone."))
                }
                Err(kube::Error::Api(status)) if status.code == 409 => Err(format!(
                    "{name} was replaced after the user reviewed it, so it was not deleted."
                )),
                Err(err) => Err(kube_error(err)),
            }
        }
        Action::Scale {
            resource,
            namespace,
            name,
            replicas,
            resource_version,
        } => {
            let api = match &namespace {
                Some(ns) => Api::<DynamicObject>::namespaced_with(client, ns, &resource),
                None => Api::all_with(client, &resource),
            };
            let mut patch = json!({"spec": {"replicas": replicas}});
            if let Some(version) = resource_version {
                patch["metadata"] = json!({"resourceVersion": version});
            }
            api.patch_scale(&name, &write_params(), &Patch::Merge(patch))
                .await
                .map_err(|err| changed_since_review(&name, err))?;
            Ok(format!("Scaled {name} to {replicas} replicas."))
        }
        Action::Restart {
            resource,
            namespace,
            name,
            resource_version,
        } => {
            let api = match &namespace {
                Some(ns) => Api::<DynamicObject>::namespaced_with(client, ns, &resource),
                None => Api::all_with(client, &resource),
            };
            let now = jiff::Timestamp::now().to_string();
            let mut patch = json!({"spec": {"template": {"metadata": {"annotations": {
                "kubectl.kubernetes.io/restartedAt": now
            }}}}});
            if let Some(version) = resource_version {
                patch["metadata"] = json!({"resourceVersion": version});
            }
            api.patch(&name, &write_params(), &Patch::Merge(patch))
                .await
                .map_err(|err| changed_since_review(&name, err))?;
            Ok(format!(
                "Restarting {name}: its pods are replaced one by one."
            ))
        }
    }
}

fn write_params() -> PatchParams {
    PatchParams {
        field_manager: Some(FIELD_MANAGER.into()),
        ..Default::default()
    }
}

/// A 409 means the object moved on after the user reviewed it: say so instead of the raw error.
fn changed_since_review(name: &str, err: kube::Error) -> String {
    match err {
        kube::Error::Api(status) if status.code == 409 => format!(
            "{name} changed after the user reviewed it, so nothing was changed. Look at it again and propose the change again."
        ),
        other => kube_error(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::Fixed;

    #[test]
    fn every_change_tool_has_a_definition_and_is_not_read_only() {
        let defs = definitions();
        let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
        assert_eq!(names, CHANGE_TOOLS);
        for def in &defs {
            assert_eq!(def["annotations"]["readOnlyHint"], false);
        }
    }

    #[test]
    fn patches_carry_the_reviewed_version_as_a_precondition() {
        // A merge or strategic patch names the version; the server answers 409 if it moved.
        let merge = with_precondition("merge", json!({"spec": {"replicas": 4}}), Some("7"));
        assert_eq!(merge["metadata"]["resourceVersion"], "7");
        assert_eq!(merge["spec"]["replicas"], 4);
        let strategic = with_precondition(
            "strategic",
            json!({"metadata": {"labels": {"a": "b"}}}),
            Some("7"),
        );
        assert_eq!(strategic["metadata"]["resourceVersion"], "7");
        assert_eq!(strategic["metadata"]["labels"]["a"], "b");
        // A JSON patch starts with a `test` on it, so a stale target fails the whole patch.
        let ops = with_precondition("json", json!([{"op": "remove", "path": "/a"}]), Some("7"));
        assert_eq!(
            ops,
            json!([
                {"op": "test", "path": "/metadata/resourceVersion", "value": "7"},
                {"op": "remove", "path": "/a"},
            ])
        );
        // Without a version nothing is added.
        assert_eq!(
            with_precondition("merge", json!({"a": 1}), None),
            json!({"a": 1})
        );
    }

    #[test]
    fn patches_come_as_text_or_json() {
        assert_eq!(
            patch_value(&json!({"patch": "{\"spec\":{\"replicas\":2}}"})).unwrap(),
            json!({"spec": {"replicas": 2}})
        );
        assert_eq!(
            patch_value(&json!({"patch": [{"op": "remove", "path": "/a"}]})).unwrap(),
            json!([{"op": "remove", "path": "/a"}])
        );
        assert!(patch_value(&json!({"patch": "{"})).is_err());
        assert!(patch_value(&json!({})).is_err());
        assert!(build_patch("json", json!({"not": "an array"})).is_err());
        assert!(build_patch("merge", json!({})).is_ok());
        assert!(build_patch("bogus", json!({})).is_err());
    }

    #[test]
    fn displayed_objects_hide_server_fields_and_secrets_values() {
        let object = json!({
            "apiVersion": "v1", "kind": "ConfigMap",
            "metadata": {"name": "a", "resourceVersion": "5", "uid": "x", "managedFields": [{}]},
            "data": {"dbPassword": "hunter2"},
            "status": {"x": 1},
        });
        let yaml = display_yaml(object, ("v1", "ConfigMap"));
        assert!(!yaml.contains("resourceVersion"));
        assert!(!yaml.contains("managedFields"));
        assert!(!yaml.contains("status"));
        assert!(!yaml.contains("hunter2"), "{yaml}");
    }

    #[tokio::test]
    async fn without_a_connection_a_change_fails_before_asking() {
        let context = ToolContext {
            cluster_name: "kind-dev".into(),
            max_output: 4096,
            ..ToolContext::default()
        };
        let approver: Arc<dyn Approver> = Arc::new(Fixed(Decision::Approved));
        for name in CHANGE_TOOLS {
            let output = run(&context, &approver, name, &json!({})).await;
            assert!(output.is_error, "{name}: {}", output.text);
            assert!(
                !output.text.starts_with("Kubyl has no change tool"),
                "{name}"
            );
        }
        let unknown = run(&context, &approver, "format_disk", &json!({})).await;
        assert!(unknown.is_error);
    }
}
