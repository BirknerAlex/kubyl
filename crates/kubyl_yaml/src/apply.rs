//! Dry run and apply of the editor's documents.
//!
//! Server-side apply with field manager `kubyl` (`dryRun=All` for the dry run, strict field
//! validation). A 409 lists the conflicting managers so the user can force. Servers without
//! server-side apply (415) fall back to replace (existing objects) or create.

use std::ops::Range;

use kube::Client;
use kube::api::{Api, DynamicObject, Patch, PatchParams, PostParams};
use kube::discovery::ApiResource;
use kubyl_core::Gvk;
use kubyl_kube::discovery::Discovery;
use kubyl_resources::ops::FIELD_MANAGER;
use serde_json::Value;

use crate::parse::{Node, Parsed, Path};
use crate::render::{self, SecretValues};
use crate::validate::{Problem, Severity, Source};

/// One document, ready to send.
#[derive(Clone, Debug)]
pub struct Prepared {
    /// Position in the stream (0-based).
    pub index: usize,
    pub gvk: Gvk,
    pub resource: ApiResource,
    pub namespace: Option<String>,
    pub name: String,
    pub object: Value,
    /// Where the document is in the buffer (to place server errors).
    pub span: Range<usize>,
}

impl Prepared {
    /// `certificate/api-tls`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.gvk.kind.to_lowercase(), self.name)
    }
}

/// Turns the parsed buffer into objects. `default_namespace` fills in namespaced objects
/// without one (the namespace in the editor's header).
pub fn prepare(
    parsed: &Parsed,
    discovery: &Discovery,
    default_namespace: Option<&str>,
    secrets: Option<&SecretValues>,
) -> Result<Vec<Prepared>, Vec<Problem>> {
    let mut problems = Vec::new();
    let mut out = Vec::new();
    if let Some(error) = &parsed.error {
        problems.push(Problem {
            range: error.range.clone(),
            severity: Severity::Error,
            message: error.message.clone(),
            path: None,
            source: Source::Syntax,
        });
    }
    for (index, doc) in parsed.docs.iter().enumerate() {
        let Some(root) = &doc.root else {
            continue;
        };
        match prepare_one(index, root, discovery, default_namespace, secrets) {
            Ok(p) => out.push(Prepared {
                span: doc.span.clone(),
                ..p
            }),
            Err(p) => problems.push(p),
        }
    }
    if out.is_empty() && problems.is_empty() {
        problems.push(Problem::error(0..0, "Nothing to apply."));
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

fn prepare_one(
    index: usize,
    root: &Node,
    discovery: &Discovery,
    default_namespace: Option<&str>,
    secrets: Option<&SecretValues>,
) -> Result<Prepared, Problem> {
    let at = |key: &str| {
        root.entry(key)
            .map_or(root.span.start..root.span.start, |e| e.key.span.clone())
    };
    let api_version = root
        .get("apiVersion")
        .and_then(Node::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Problem::error(at("apiVersion"), "Required value: apiVersion"))?;
    let kind = root
        .get("kind")
        .and_then(Node::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Problem::error(at("kind"), "Required value: kind"))?;
    let (group, version) = match api_version.split_once('/') {
        Some((g, v)) => (g, v),
        None => ("", api_version),
    };
    let gvk = Gvk::new(group, version, kind);
    let info = discovery.by_gvk(&gvk).ok_or_else(|| {
        Problem::error(
            at("kind"),
            format!("The cluster doesn't serve {kind} in {api_version}."),
        )
    })?;
    let mut object = root.to_json();
    let name = object
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| Problem::error(at("metadata"), "Required value: metadata.name"))?;
    let namespace = if info.namespaced {
        let ns = object
            .pointer("/metadata/namespace")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| default_namespace.map(String::from))
            .unwrap_or_else(|| "default".into());
        if let Some(meta) = object.get_mut("metadata").and_then(Value::as_object_mut) {
            meta.insert("namespace".into(), Value::String(ns.clone()));
        }
        Some(ns)
    } else {
        None
    };
    render::strip_server_fields(&mut object);
    render::restore_secret(&mut object, secrets);
    Ok(Prepared {
        index,
        gvk,
        resource: kubyl_resources::store::api_resource(info),
        namespace,
        name,
        object,
        span: root.span.clone(),
    })
}

/// A field another manager owns (`helm` owns `.spec.renewBefore`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub manager: String,
    pub field: String,
}

/// A field-level error from the server (validation, admission webhooks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cause {
    /// `spec.privateKey.rotationPolicy` (empty when the server didn't name one).
    pub field: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The server's resulting object.
    Applied(Value),
    Conflicts(Vec<Conflict>),
    Failed {
        message: String,
        causes: Vec<Cause>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocResult {
    pub index: usize,
    pub label: String,
    pub span: Range<usize>,
    pub outcome: Outcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub dry_run: bool,
    /// Take over fields other managers own.
    pub force: bool,
}

/// Applies each document in order. Stops at nothing: every document gets a result.
pub async fn apply_all(client: Client, docs: Vec<Prepared>, options: Options) -> Vec<DocResult> {
    let mut results = Vec::new();
    for doc in docs {
        let outcome = apply_one(&client, &doc, options).await;
        results.push(DocResult {
            index: doc.index,
            label: doc.label(),
            span: doc.span.clone(),
            outcome,
        });
    }
    results
}

fn api(client: &Client, doc: &Prepared) -> Api<DynamicObject> {
    match &doc.namespace {
        Some(ns) => Api::namespaced_with(client.clone(), ns, &doc.resource),
        None => Api::all_with(client.clone(), &doc.resource),
    }
}

async fn apply_one(client: &Client, doc: &Prepared, options: Options) -> Outcome {
    let api = api(client, doc);
    let mut params = PatchParams::apply(FIELD_MANAGER).validation_strict();
    if options.force {
        params = params.force();
    }
    if options.dry_run {
        params = params.dry_run();
    }
    match api
        .patch(&doc.name, &params, &Patch::Apply(&doc.object))
        .await
    {
        Ok(object) => Outcome::Applied(serde_json::to_value(object).unwrap_or_default()),
        Err(kube::Error::Api(status)) if status.code == 415 => {
            replace_or_create(&api, doc, options).await
        }
        Err(err) => failure(err),
    }
}

/// For servers without server-side apply: `PUT` with the live resourceVersion, or `POST`.
async fn replace_or_create(api: &Api<DynamicObject>, doc: &Prepared, options: Options) -> Outcome {
    let params = PostParams {
        dry_run: options.dry_run,
        field_manager: Some(FIELD_MANAGER.into()),
    };
    let mut object: DynamicObject = match serde_json::from_value(doc.object.clone()) {
        Ok(o) => o,
        Err(err) => {
            return Outcome::Failed {
                message: err.to_string(),
                causes: Vec::new(),
            };
        }
    };
    let result = match api.get_opt(&doc.name).await {
        Ok(Some(live)) => {
            object.metadata.resource_version = live.metadata.resource_version;
            api.replace(&doc.name, &params, &object).await
        }
        Ok(None) => api.create(&params, &object).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(object) => Outcome::Applied(serde_json::to_value(object).unwrap_or_default()),
        Err(err) => failure(err),
    }
}

fn failure(err: kube::Error) -> Outcome {
    let kube::Error::Api(status) = err else {
        return Outcome::Failed {
            message: err.to_string(),
            causes: Vec::new(),
        };
    };
    let causes: Vec<_> = status
        .details
        .as_ref()
        .map(|d| d.causes.clone())
        .unwrap_or_default();
    if status.code == 409 && (status.reason == "Conflict" || !causes.is_empty()) {
        let conflicts = conflicts(&status.message, &causes);
        if !conflicts.is_empty() {
            return Outcome::Conflicts(conflicts);
        }
    }
    Outcome::Failed {
        message: status.message.clone(),
        causes: causes
            .into_iter()
            .map(|c| Cause {
                field: c.field.trim_start_matches('.').to_string(),
                message: c.message,
            })
            .collect(),
    }
}

/// SSA conflicts, from the status causes or, on older servers, from the message
/// (`Apply failed with 1 conflict: conflict with "helm" using apps/v1: .spec.replicas`).
fn conflicts(message: &str, causes: &[kube::core::response::StatusCause]) -> Vec<Conflict> {
    let manager_of = |text: &str| {
        text.split_once("conflict with \"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(m, _)| m.to_string())
    };
    let mut out: Vec<Conflict> = causes
        .iter()
        .filter(|c| c.reason == "FieldManagerConflict" || c.message.contains("conflict with"))
        .filter_map(|c| {
            Some(Conflict {
                manager: manager_of(&c.message)?,
                field: c.field.clone(),
            })
        })
        .collect();
    if out.is_empty() {
        for part in message.split("conflict with \"").skip(1) {
            let Some((manager, rest)) = part.split_once('"') else {
                continue;
            };
            let field = rest
                .rsplit_once(": ")
                .map(|(_, f)| f.trim().trim_end_matches(',').to_string())
                .unwrap_or_default();
            out.push(Conflict {
                manager: manager.to_string(),
                field,
            });
        }
    }
    out
}

/// Server errors as editor problems: a cause's field is located in its document; otherwise the
/// problem sits on the document's first line.
pub fn server_problems(parsed: &Parsed, results: &[DocResult]) -> Vec<Problem> {
    let mut out = Vec::new();
    for result in results {
        let Outcome::Failed { message, causes } = &result.outcome else {
            continue;
        };
        let root = parsed.docs.get(result.index).and_then(|d| d.root.as_ref());
        let first_line = result.span.start..result.span.start;
        let place = |field: &str| -> Range<usize> {
            let Some(root) = root else {
                return first_line.clone();
            };
            let path = Path::parse(field);
            // The deepest part of the path that exists in the buffer.
            for len in (1..=path.0.len()).rev() {
                let prefix = Path(path.0[..len].to_vec());
                if let Some(entry) = root.find_entry(&prefix) {
                    return if len == path.0.len() && entry.value.as_scalar().is_some() {
                        entry.value.span.clone()
                    } else {
                        entry.key.span.clone()
                    };
                }
                if let Some(node) = root.find(&prefix) {
                    return node.span.clone();
                }
            }
            root.as_map()
                .and_then(|e| e.first())
                .map_or(first_line.clone(), |e| e.key.span.clone())
        };
        let with_field: Vec<&Cause> = causes.iter().filter(|c| !c.field.is_empty()).collect();
        if with_field.is_empty() {
            out.push(Problem {
                range: place(""),
                severity: Severity::Error,
                message: message.clone(),
                path: None,
                source: Source::Server,
            });
        }
        for cause in with_field {
            out.push(Problem {
                range: place(&cause.field),
                severity: Severity::Error,
                message: cause.message.clone(),
                path: Some(cause.field.clone()),
                source: Source::Server,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use kube::core::response::StatusCause;

    #[test]
    fn parses_conflicts() {
        let causes = vec![StatusCause {
            reason: "FieldManagerConflict".into(),
            message: "conflict with \"helm\" using cert-manager.io/v1".into(),
            field: ".spec.renewBefore".into(),
        }];
        assert_eq!(
            conflicts("", &causes),
            [Conflict {
                manager: "helm".into(),
                field: ".spec.renewBefore".into()
            }]
        );
        let message = "Apply failed with 2 conflicts: conflict with \"kubectl-client-side-apply\" using apps/v1: .spec.replicas, conflict with \"helm\" using apps/v1: .spec.template";
        let found = conflicts(message, &[]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].manager, "kubectl-client-side-apply");
        assert_eq!(found[0].field, ".spec.replicas");
        assert_eq!(found[1].manager, "helm");
    }

    #[test]
    fn places_server_errors_on_their_field() {
        let text = "apiVersion: cert-manager.io/v1\nkind: Certificate\nspec:\n  privateKey:\n    rotationPolicy: Allways\n";
        let parsed = parse(text);
        let results = vec![DocResult {
            index: 0,
            label: "certificate/x".into(),
            span: 0..text.len(),
            outcome: Outcome::Failed {
                message: "invalid".into(),
                causes: vec![Cause {
                    field: "spec.privateKey.rotationPolicy".into(),
                    message: "Unsupported value: \"Allways\"".into(),
                }],
            },
        }];
        let problems = server_problems(&parsed, &results);
        assert_eq!(problems.len(), 1);
        assert_eq!(&text[problems[0].range.clone()], "Allways");
        assert_eq!(problems[0].source, Source::Server);
    }
}
