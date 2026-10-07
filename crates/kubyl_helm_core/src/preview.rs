//! What an install, upgrade, rollback or uninstall changes (decision 5), before anything runs.
//!
//! - Install: the objects of the dry run's manifest, grouped by kind.
//! - Upgrade and rollback: a per-object diff of the rendered manifest against the current one,
//!   plus the values diff (masked until revealed).
//! - Hooks that will run, and the CRDs of the chart's `crds/` folder (Helm installs them once
//!   and never upgrades or deletes them).
//! - Uninstall: what gets deleted, and what stays (`helm.sh/resource-policy: keep`, PVCs of
//!   StatefulSet templates, CRDs).
//!
//! Secrets in manifests are masked before anything is diffed or shown; whether a Secret's data
//! changed is still reported ("data changed, values hidden").

use std::collections::BTreeMap;

use kubyl_yaml_core::diff::{self, LineDiff};
use serde_json::Value;

use crate::decode::{Hook, Release};
use crate::present::{self, MASK};

/// Identifies an object across revisions (the API version may change between them).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectKey {
    /// The API group (`apps`; empty for core).
    pub group: String,
    pub kind: String,
    /// As written in the manifest, `None` when that's the release's own namespace (or none:
    /// namespaced kinds without one live in the release's), so an object keeps its key whether
    /// a revision spells the namespace out or not.
    pub namespace: Option<String>,
    pub name: String,
}

impl ObjectKey {
    /// `Deployment web`, `ConfigMap shop/settings`.
    pub fn label(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{} {ns}/{}", self.kind, self.name),
            None => format!("{} {}", self.kind, self.name),
        }
    }
}

/// One document of a manifest.
#[derive(Clone, PartialEq, Eq)]
pub struct Document {
    pub key: ObjectKey,
    pub api_version: String,
    /// The document as rendered (with Helm's `# Source:` line).
    pub text: String,
    /// `helm.sh/resource-policy: keep`.
    pub keep: bool,
}

/// Without the text: a rendered Secret's data is in it.
impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("key", &self.key)
            .field("api_version", &self.api_version)
            .field("keep", &self.keep)
            .finish_non_exhaustive()
    }
}

/// Splits a manifest of a release in `namespace` into its objects (documents without kind or
/// name are skipped).
pub fn documents(manifest: &str, namespace: &str) -> Vec<Document> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut flush = |text: &mut String| {
        if let Some(doc) = document(text, namespace) {
            out.push(doc);
        }
        text.clear();
    };
    for line in manifest.split_inclusive('\n') {
        if line.trim_end() == "---" || line.starts_with("--- ") {
            flush(&mut current);
        } else {
            current.push_str(line);
        }
    }
    flush(&mut current);
    out
}

fn document(text: &str, release_namespace: &str) -> Option<Document> {
    if text.trim().is_empty() {
        return None;
    }
    let parsed = kubyl_yaml_core::parse::parse(text);
    let root = parsed.roots().next()?;
    let get = |path: &[&str]| {
        root.find(&kubyl_yaml_core::parse::Path::keys(path))
            .and_then(|n| n.as_str())
            .map(str::to_string)
    };
    let api_version = get(&["apiVersion"])?;
    let kind = get(&["kind"])?;
    let name = get(&["metadata", "name"])?;
    let group = api_version
        .split_once('/')
        .map(|(g, _)| g.to_string())
        .unwrap_or_default();
    let keep =
        get(&["metadata", "annotations", "helm.sh/resource-policy"]).as_deref() == Some("keep");
    Some(Document {
        key: ObjectKey {
            group,
            kind,
            namespace: get(&["metadata", "namespace"])
                .filter(|ns| !ns.is_empty() && ns != release_namespace),
            name,
        },
        api_version,
        text: text.trim_start_matches('\n').to_string(),
        keep,
    })
}

/// The names of the CustomResourceDefinitions in YAML text (a chart's `crds/` file).
pub fn crd_names(text: &str) -> Vec<String> {
    documents(text, "")
        .into_iter()
        .filter(|d| d.key.kind == "CustomResourceDefinition")
        .map(|d| d.key.name)
        .collect()
}

/// How one object changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Changed,
    Unchanged,
}

/// One object of a per-object diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectChange {
    pub key: ObjectKey,
    pub change: Change,
    /// The diff of the masked texts (empty for added, removed and unchanged objects).
    pub diff: LineDiff,
    /// The masked text of the side that exists (new, or old for removed objects).
    pub text: String,
    /// A Secret whose data changed (its values are masked, so the diff can't show it).
    pub secret_data_changed: bool,
    /// The API version changed (`policy/v1beta1` → `policy/v1`).
    pub api_version: Option<(String, String)>,
}

/// Per-object changes from `old` to `new` (manifests of a release in `namespace`), changed and
/// added first, then removed, then unchanged; within those by kind rank and name.
pub fn object_changes(old: &str, new: &str, namespace: &str) -> Vec<ObjectChange> {
    let old_docs: BTreeMap<ObjectKey, Document> = documents(old, namespace)
        .into_iter()
        .map(|d| (d.key.clone(), d))
        .collect();
    let new_docs: BTreeMap<ObjectKey, Document> = documents(new, namespace)
        .into_iter()
        .map(|d| (d.key.clone(), d))
        .collect();
    let mut out = Vec::new();
    for (key, new_doc) in &new_docs {
        let new_masked = present::mask_secrets(&new_doc.text);
        match old_docs.get(key) {
            None => out.push(ObjectChange {
                key: key.clone(),
                change: Change::Added,
                diff: LineDiff::default(),
                text: new_masked,
                secret_data_changed: false,
                api_version: None,
            }),
            Some(old_doc) => {
                let old_masked = present::mask_secrets(&old_doc.text);
                let diff = diff::diff(&old_masked, &new_masked, 3);
                // The masked diff can't show a Secret's data: compare it raw, whatever else
                // changed.
                let secret_data_changed = key.kind == "Secret"
                    && secret_payload(&old_doc.text) != secret_payload(&new_doc.text);
                let api_version = (old_doc.api_version != new_doc.api_version)
                    .then(|| (old_doc.api_version.clone(), new_doc.api_version.clone()));
                let changed = !diff.is_empty() || secret_data_changed || api_version.is_some();
                out.push(ObjectChange {
                    key: key.clone(),
                    change: if changed {
                        Change::Changed
                    } else {
                        Change::Unchanged
                    },
                    diff,
                    text: new_masked,
                    secret_data_changed,
                    api_version,
                });
            }
        }
    }
    for (key, old_doc) in &old_docs {
        if !new_docs.contains_key(key) {
            out.push(ObjectChange {
                key: key.clone(),
                change: Change::Removed,
                diff: LineDiff::default(),
                text: present::mask_secrets(&old_doc.text),
                secret_data_changed: false,
                api_version: None,
            });
        }
    }
    let rank = |c: &Change| match c {
        Change::Changed => 0,
        Change::Added => 1,
        Change::Removed => 2,
        Change::Unchanged => 3,
    };
    out.sort_by(|a, b| {
        (
            rank(&a.change),
            present::kind_rank(&a.key.kind),
            &a.key.kind,
            &a.key.name,
        )
            .cmp(&(
                rank(&b.change),
                present::kind_rank(&b.key.kind),
                &b.key.kind,
                &b.key.name,
            ))
    });
    out
}

/// A Secret's `data` and `stringData`, for telling whether they changed (never shown).
fn secret_payload(text: &str) -> Option<(Value, Value)> {
    let value: Value = serde_saphyr::from_str(text).ok()?;
    Some((
        value.get("data").cloned().unwrap_or(Value::Null),
        value.get("stringData").cloned().unwrap_or(Value::Null),
    ))
}

/// Counts for the summary line (`2 changed · 1 added · 1 removed · 14 unchanged`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub changed: usize,
    pub added: usize,
    pub removed: usize,
    pub unchanged: usize,
}

impl Counts {
    pub fn of(changes: &[ObjectChange]) -> Self {
        let mut counts = Self::default();
        for change in changes {
            match change.change {
                Change::Changed => counts.changed += 1,
                Change::Added => counts.added += 1,
                Change::Removed => counts.removed += 1,
                Change::Unchanged => counts.unchanged += 1,
            }
        }
        counts
    }

    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        for (n, what) in [
            (self.changed, "changed"),
            (self.added, "added"),
            (self.removed, "removed"),
            (self.unchanged, "unchanged"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        if parts.is_empty() {
            "no objects".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// Values as YAML (keys sorted), for diffs.
pub fn values_yaml(values: &Value) -> String {
    match values {
        Value::Object(map) if map.is_empty() => String::new(),
        Value::Null => String::new(),
        other => kubyl_yaml_core::render::to_yaml(&kubyl_yaml_core::render::sorted(other.clone())),
    }
}

/// The values diff (old → new user-supplied values).
pub fn values_diff(old: &Value, new: &Value) -> LineDiff {
    diff::diff(&values_yaml(old), &values_yaml(new), 3)
}

/// A values line with its scalar masked (`password: hunter2` → `password: ••••••••`): values
/// diffs show which keys change without showing what they hold, until revealed. Booleans,
/// `null`, empty collections and keys stay readable.
pub fn mask_value_line(line: &str) -> String {
    let indent = line.len() - line.trim_start().len();
    let body = &line[indent..];
    let (prefix, value) = if let Some(rest) = body.strip_prefix("- ") {
        match split_key(rest) {
            Some((key, value)) => (format!("- {key}:"), value),
            None => ("-".to_string(), rest),
        }
    } else {
        match split_key(body) {
            Some((key, value)) => (format!("{key}:"), value),
            // A continuation of a block scalar or a bare item: all value.
            None if body.is_empty() => return line.to_string(),
            None => return format!("{}{MASK}", &line[..indent]),
        }
    };
    let value = value.trim();
    let keep = value.is_empty()
        || matches!(
            value,
            "true" | "false" | "null" | "~" | "{}" | "[]" | "|" | "|-" | "|+" | ">" | ">-" | ">+"
        );
    if keep {
        line.to_string()
    } else {
        format!("{}{prefix} {MASK}", &line[..indent])
    }
}

fn split_key(text: &str) -> Option<(&str, &str)> {
    if let Some(end) = text.find(": ") {
        return Some((&text[..end], &text[end + 2..]));
    }
    text.strip_suffix(':').map(|key| (key, ""))
}

/// A hook as the preview lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookInfo {
    pub kind: String,
    pub name: String,
    pub events: Vec<String>,
}

/// The hooks that run for `event` (`pre-install`, `post-upgrade`, …; `install` covers both).
pub fn hooks_for(hooks: &[Hook], operation: &str) -> Vec<HookInfo> {
    hooks
        .iter()
        .filter(|h| {
            h.events
                .iter()
                .any(|e| e.ends_with(&format!("-{operation}")) || e == operation)
        })
        .map(|h| HookInfo {
            kind: h.kind.clone(),
            name: h.name.clone(),
            events: h.events.clone(),
        })
        .collect()
}

/// What an install or upgrade dry run returned, ready for the preview.
#[derive(Clone)]
pub struct Preview {
    /// The release as rendered (its values and manifest stay in memory, never logged).
    pub release: Release,
    /// The rendered objects (install) or the per-object changes (upgrade, rollback).
    pub changes: Vec<ObjectChange>,
    pub hooks: Vec<HookInfo>,
    /// CRDs in the chart's `crds/` folder.
    pub crds: Vec<String>,
    /// The server-side dry run wasn't allowed: rendered client-side (less exact).
    pub client_side: bool,
    /// The values diff (upgrade, rollback).
    pub values: Option<LineDiff>,
}

/// Counts only: the values diff holds the values as typed (masked only when shown), and the
/// release its manifest.
impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Preview")
            .field("release", &self.release)
            .field("changes", &Counts::of(&self.changes))
            .field("hooks", &self.hooks.len())
            .field("crds", &self.crds)
            .field("client_side", &self.client_side)
            .field(
                "values",
                &self.values.as_ref().map(|v| (v.added, v.removed)),
            )
            .finish()
    }
}

/// An install's preview from the dry run's JSON.
pub fn install_preview(json: &str, client_side: bool) -> Result<Preview, String> {
    let release = crate::decode::from_json(json.as_bytes()).map_err(|e| e.to_string())?;
    let changes = object_changes("", &release.manifest, &release.summary.namespace);
    Ok(Preview {
        hooks: hooks_for(&release.hooks, "install"),
        crds: release.crds.clone(),
        changes,
        client_side,
        values: None,
        release,
    })
}

/// An upgrade's preview: the dry run against the current release.
pub fn upgrade_preview(
    json: &str,
    current: &Release,
    client_side: bool,
) -> Result<Preview, String> {
    let release = crate::decode::from_json(json.as_bytes()).map_err(|e| e.to_string())?;
    let changes = object_changes(
        &current.manifest,
        &release.manifest,
        &current.summary.namespace,
    );
    let values = values_diff(&current.values, &release.values);
    Ok(Preview {
        hooks: hooks_for(&release.hooks, "upgrade"),
        crds: release.crds.clone(),
        changes,
        client_side,
        values: Some(values),
        release,
    })
}

/// A rollback's preview from the stored revisions (Helm has no rollback dry run with output):
/// rolling back re-applies `target`'s manifest and values.
pub fn rollback_preview(current: &Release, target: &Release) -> Preview {
    Preview {
        changes: object_changes(
            &current.manifest,
            &target.manifest,
            &current.summary.namespace,
        ),
        hooks: hooks_for(&target.hooks, "rollback"),
        crds: Vec::new(),
        client_side: false,
        values: Some(values_diff(&current.values, &target.values)),
        release: target.clone(),
    }
}

/// Whether a dry run failed only because the server-side dry run isn't allowed (lookups need
/// `get`; validation needs the CRDs' kinds): retry client-side.
pub fn needs_client_fallback(error: &crate::cli::HelmError) -> bool {
    use crate::cli::ErrorKind;
    error.kind == ErrorKind::Forbidden
        || error.detail.contains("unknown flag: --dry-run")
        || error.detail.contains("invalid argument \"server\"")
}

/// An install's preview: the server-side dry run, client-side when that isn't allowed (on Tokio).
pub async fn install_dry_run(
    helm: &crate::cli::HelmInfo,
    target: Option<&kubyl_kube_core::cli::CliTarget>,
    spec: &crate::cmd::InstallSpec,
    values: &str,
) -> Result<Preview, String> {
    use crate::cmd::{self, Mode};
    let run = |mode| {
        crate::cli::run(
            helm,
            target,
            cmd::install(spec, values, mode, helm.version),
            None,
            None,
        )
    };
    match run(Mode::DryRunServer).await {
        Ok(output) => install_preview(&output.stdout, false),
        Err(err) if needs_client_fallback(&err) => {
            let output = run(Mode::DryRunClient).await.map_err(|e| e.message)?;
            install_preview(&output.stdout, true)
        }
        Err(err) => Err(err.message),
    }
}

/// An upgrade's preview against `current`: the server-side dry run, client-side when that isn't
/// allowed (on Tokio).
pub async fn upgrade_dry_run(
    helm: &crate::cli::HelmInfo,
    target: Option<&kubyl_kube_core::cli::CliTarget>,
    spec: &crate::cmd::UpgradeSpec,
    values: &str,
    current: &Release,
) -> Result<Preview, String> {
    use crate::cmd::{self, Mode};
    let run = |mode| {
        crate::cli::run(
            helm,
            target,
            cmd::upgrade(spec, values, mode, helm.version),
            None,
            None,
        )
    };
    match run(Mode::DryRunServer).await {
        Ok(output) => upgrade_preview(&output.stdout, current, false),
        Err(err) if needs_client_fallback(&err) => {
            let output = run(Mode::DryRunClient).await.map_err(|e| e.message)?;
            upgrade_preview(&output.stdout, current, true)
        }
        Err(err) => Err(err.message),
    }
}

/// The chart version a dry run rendered (what the release will say it runs): applying pins it,
/// so the install or upgrade runs the chart the preview showed.
pub fn rendered_version(preview: &Preview) -> Option<String> {
    let version = preview.release.summary.chart.version.trim();
    (!version.is_empty()).then(|| version.to_string())
}

/// What an uninstall deletes and what stays.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UninstallPlan {
    pub deleted: Vec<ObjectKey>,
    /// What stays, and why.
    pub kept: Vec<(String, String)>,
    pub hooks: Vec<HookInfo>,
}

/// The plan for uninstalling `release`.
pub fn uninstall_plan(release: &Release) -> UninstallPlan {
    let mut plan = UninstallPlan::default();
    for doc in documents(&release.manifest, &release.summary.namespace) {
        if doc.keep {
            plan.kept
                .push((doc.key.label(), "helm.sh/resource-policy: keep".to_string()));
            continue;
        }
        if doc.key.kind == "StatefulSet" {
            for template in volume_claim_templates(&doc.text) {
                plan.kept.push((
                    format!("PersistentVolumeClaims {template}-{}-*", doc.key.name),
                    "created by the StatefulSet, not by Helm: they stay with their data".into(),
                ));
            }
        }
        plan.deleted.push(doc.key);
    }
    plan.deleted.sort_by(|a, b| {
        (present::kind_rank(&a.kind), &a.kind, &a.name).cmp(&(
            present::kind_rank(&b.kind),
            &b.kind,
            &b.name,
        ))
    });
    for crd in &release.crds {
        plan.kept.push((
            format!("CustomResourceDefinition {crd}"),
            "installed from the chart's crds/ folder: Helm never deletes it".into(),
        ));
    }
    plan.hooks = hooks_for(&release.hooks, "delete");
    plan
}

/// The `volumeClaimTemplates` names of a StatefulSet document.
fn volume_claim_templates(text: &str) -> Vec<String> {
    let parsed = kubyl_yaml_core::parse::parse(text);
    let Some(root) = parsed.roots().next() else {
        return Vec::new();
    };
    root.find(&kubyl_yaml_core::parse::Path::keys(&[
        "spec",
        "volumeClaimTemplates",
    ]))
    .and_then(|n| match &n.value {
        kubyl_yaml_core::parse::NodeValue::Seq(items) => Some(items),
        _ => None,
    })
    .map(|items| {
        items
            .iter()
            .filter_map(|item| {
                item.find(&kubyl_yaml_core::parse::Path::keys(&["metadata", "name"]))
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            })
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OLD: &str = "---\n# Source: demo/templates/cm.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: web-settings\ndata:\n  color: blue\n---\n# Source: demo/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: web-auth\ndata:\n  password: aHVudGVyMg==\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  replicas: 1\n---\napiVersion: policy/v1beta1\nkind: PodDisruptionBudget\nmetadata:\n  name: web\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: old-svc\n";
    const NEW: &str = "---\n# Source: demo/templates/configmap.yaml\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: web-settings\ndata:\n  color: blue\n---\n# Source: demo/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: web-auth\ndata:\n  password: c2VjcmV0Mg==\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  replicas: 3\n---\napiVersion: policy/v1\nkind: PodDisruptionBudget\nmetadata:\n  name: web\n---\napiVersion: batch/v1\nkind: Job\nmetadata:\n  name: migrate\n";

    #[test]
    fn upgrades_diff_per_object_without_showing_secrets() {
        let changes = object_changes(OLD, NEW, "shop");
        let by = |kind: &str| changes.iter().find(|c| c.key.kind == kind).unwrap();
        let deployment = by("Deployment");
        assert_eq!(deployment.change, Change::Changed);
        assert_eq!(deployment.diff.added, 1);
        let secret = by("Secret");
        assert_eq!(secret.change, Change::Changed);
        assert!(secret.secret_data_changed);
        assert!(!format!("{changes:?}").contains("c2VjcmV0Mg=="));
        assert!(!format!("{changes:?}").contains("aHVudGVyMg=="));
        // A moved template isn't a change.
        assert_eq!(by("ConfigMap").change, Change::Unchanged);
        assert_eq!(
            by("PodDisruptionBudget").api_version,
            Some(("policy/v1beta1".into(), "policy/v1".into()))
        );
        assert_eq!(by("Job").change, Change::Added);
        assert_eq!(by("Service").change, Change::Removed);
        assert_eq!(
            Counts::of(&changes).label(),
            "3 changed · 1 added · 1 removed · 1 unchanged"
        );
        // Changed first, unchanged last.
        assert_eq!(changes[0].change, Change::Changed);
        assert_eq!(changes.last().unwrap().change, Change::Unchanged);
    }

    #[test]
    fn secret_data_changes_are_reported_next_to_other_changes() {
        let old = "apiVersion: v1\nkind: Secret\nmetadata:\n  name: auth\n  labels:\n    v: \"1\"\ndata:\n  password: aHVudGVyMg==\n";
        let new = "apiVersion: v1\nkind: Secret\nmetadata:\n  name: auth\n  labels:\n    v: \"2\"\ndata:\n  password: c2VjcmV0Mg==\n";
        let changes = object_changes(old, new, "shop");
        assert_eq!(changes[0].change, Change::Changed);
        assert!(!changes[0].diff.is_empty(), "the label changed");
        assert!(changes[0].secret_data_changed);
        // Only the label: the data stays.
        let same_data = new.replace("c2VjcmV0Mg==", "aHVudGVyMg==");
        assert!(!object_changes(old, &same_data, "shop")[0].secret_data_changed);
        // stringData counts too.
        let string_data =
            "apiVersion: v1\nkind: Secret\nmetadata:\n  name: auth\nstringData:\n  token: a\n";
        let changed = object_changes(
            string_data,
            &string_data.replace("token: a", "token: b"),
            "shop",
        );
        assert!(changed[0].secret_data_changed);
        assert!(!format!("{changes:?}{:?}", documents(new, "shop")).contains("c2VjcmV0Mg=="));
    }

    #[test]
    fn the_release_namespace_written_out_or_not_is_the_same_object() {
        let implicit =
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\ndata:\n  a: \"1\"\n";
        let explicit = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\n  namespace: shop\ndata:\n  a: \"1\"\n";
        let changes = object_changes(implicit, explicit, "shop");
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].change, Change::Changed);
        assert_eq!(changes[0].key.label(), "ConfigMap settings");
        // Another namespace is another object.
        let other = explicit.replace("namespace: shop", "namespace: ops");
        let changes = object_changes(implicit, &other, "shop");
        assert_eq!(Counts::of(&changes).label(), "1 added · 1 removed");
        assert!(
            changes
                .iter()
                .any(|c| c.key.label() == "ConfigMap ops/settings")
        );
    }

    #[test]
    fn values_diffs_mask_scalars() {
        let diff = values_diff(
            &json!({"replicaCount": 1, "auth": {"password": "hunter2"}}),
            &json!({"replicaCount": 2, "auth": {"password": "hunter2", "enabled": true}}),
        );
        assert!(diff.added >= 2);
        assert_eq!(
            mask_value_line("  password: hunter2"),
            format!("  password: {MASK}")
        );
        assert_eq!(mask_value_line("  enabled: true"), "  enabled: true");
        assert_eq!(mask_value_line("auth:"), "auth:");
        assert_eq!(mask_value_line("- name: pager"), format!("- name: {MASK}"));
        assert_eq!(mask_value_line("- a"), format!("- {MASK}"));
        assert_eq!(mask_value_line("note: |"), "note: |");
        assert_eq!(mask_value_line("  line one"), format!("  {MASK}"));
    }

    #[test]
    fn uninstall_says_what_stays() {
        let manifest = "---\napiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: data\n  annotations:\n    helm.sh/resource-policy: keep\n---\napiVersion: apps/v1\nkind: StatefulSet\nmetadata:\n  name: db\nspec:\n  volumeClaimTemplates:\n  - metadata:\n      name: data\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: db\n";
        let release = Release {
            summary: Default::default(),
            values: json!({}),
            chart_values: json!({}),
            manifest: manifest.into(),
            notes: None,
            hooks: vec![Hook {
                name: "cleanup".into(),
                kind: "Job".into(),
                path: String::new(),
                events: vec!["pre-delete".into()],
                manifest: String::new(),
            }],
            crds: vec!["widgets.demo.kubyl.dev".into()],
        };
        let plan = uninstall_plan(&release);
        assert_eq!(plan.deleted.len(), 2);
        assert_eq!(plan.deleted[0].kind, "StatefulSet");
        let kept: Vec<&str> = plan.kept.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(
            kept,
            [
                "PersistentVolumeClaim data",
                "PersistentVolumeClaims data-db-*",
                "CustomResourceDefinition widgets.demo.kubyl.dev"
            ]
        );
        assert_eq!(plan.hooks[0].name, "cleanup");
    }

    #[test]
    fn dry_run_json_becomes_a_preview() {
        use base64::Engine as _;
        let crd = base64::engine::general_purpose::STANDARD.encode(
            "apiVersion: apiextensions.k8s.io/v1\nkind: CustomResourceDefinition\nmetadata:\n  name: widgets.demo.kubyl.dev\n",
        );
        let json = json!({
            "name": "web", "namespace": "shop", "version": 1,
            "info": {"status": "pending-install", "notes": "hi"},
            "chart": {"metadata": {"name": "demo", "version": "0.2.0"}, "values": {"replicaCount": 1},
                      "files": [{"name": "crds/widgets.yaml", "data": crd}, {"name": "README.md", "data": ""}]},
            "config": {"replicaCount": 2},
            "manifest": NEW,
            "hooks": [{"name": "web-migrate", "kind": "Job", "path": "x", "events": ["pre-install", "pre-upgrade"], "manifest": ""}]
        })
        .to_string();
        let preview = install_preview(&json, false).unwrap();
        assert_eq!(preview.crds, ["widgets.demo.kubyl.dev"]);
        assert_eq!(preview.hooks[0].name, "web-migrate");
        assert!(preview.changes.iter().all(|c| c.change == Change::Added));
        assert!(
            !preview
                .changes
                .iter()
                .any(|c| c.text.contains("c2VjcmV0Mg=="))
        );
        let current = crate::decode::from_json(json.as_bytes()).unwrap();
        let upgrade = upgrade_preview(&json, &current, true).unwrap();
        assert!(upgrade.client_side);
        assert!(upgrade.values.unwrap().is_empty());
    }
}
