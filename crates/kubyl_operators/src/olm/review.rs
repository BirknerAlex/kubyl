//! Reviewing a pending InstallPlan before approving it: which CRDs change (live CRD vs the
//! bundle's), which permissions the operator gains or loses, and what may not fit.
//!
//! The new manifests come from the plan's steps: inline JSON, or (what OLM writes for bundle
//! images) a reference to the ConfigMap OLM unpacked the bundle into, in the catalog's
//! namespace. Kubyl reads that ConfigMap like OLM does; it never pulls images or talks to
//! catalog pods. Nothing here is Secret data.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read as _;

use base64::Engine as _;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::Api;
use kubyl_yaml::diff::LineDiff;
use serde_json::Value;

use super::model::{BundleRef, Csv, InstallMode, InstallPlan, Permission};

/// The objects of an unpacked bundle ConfigMap. Each key holds one manifest file (YAML, maybe
/// several documents); with `olm.contentEncoding: gzip+base64` its bytes are base64 of gzip.
pub fn decode_bundle(config_map: &ConfigMap) -> Result<Vec<Value>, String> {
    let encoded = config_map
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("olm.contentEncoding"))
        .map(String::as_str)
        == Some("gzip+base64");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (key, value) in config_map.data.iter().flatten() {
        files.push((key.clone(), value.clone().into_bytes()));
    }
    for (key, value) in config_map.binary_data.iter().flatten() {
        files.push((key.clone(), value.0.clone()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut objects = Vec::new();
    for (key, bytes) in files {
        let bytes = if encoded {
            unpack(&bytes).map_err(|e| format!("{key}: {e}"))?
        } else {
            bytes
        };
        let text = String::from_utf8(bytes).map_err(|_| format!("{key}: not UTF-8"))?;
        let docs: Vec<Value> =
            serde_saphyr::from_multiple(&text).map_err(|e| format!("{key}: {e}"))?;
        objects.extend(docs.into_iter().filter(Value::is_object));
    }
    Ok(objects)
}

fn unpack(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let trimmed: Vec<u8> = bytes
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let gz = base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .map_err(|_| "not base64".to_string())?;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz.as_slice())
        .take(64 << 20)
        .read_to_end(&mut out)
        .map_err(|_| "not gzip".to_string())?;
    Ok(out)
}

/// The object of `kind`/`name` among a bundle's objects.
pub fn find<'a>(objects: &'a [Value], kind: &str, name: &str) -> Option<&'a Value> {
    objects.iter().find(|o| {
        o.get("kind").and_then(Value::as_str) == Some(kind)
            && o.pointer("/metadata/name").and_then(Value::as_str) == Some(name)
    })
}

/// What happens to one CRD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrdStatus {
    New,
    Changed,
    Unchanged,
    /// The bundle's manifest couldn't be read.
    Unknown,
}

/// One CRD of the plan.
#[derive(Clone, Debug)]
pub struct CrdChange {
    pub name: String,
    pub status: CrdStatus,
    /// `spec` of the live CRD vs the bundle's, as YAML.
    pub diff: LineDiff,
    /// A one-line summary (`+ 6 lines`, `v1beta1 no longer served`).
    pub summary: String,
    /// Things OLM or users trip over (versions still stored but dropped…).
    pub warnings: Vec<String>,
}

/// Drops what the API server or OLM set on a live CRD that the bundle doesn't have, so the diff
/// shows real changes only.
fn normalize_spec(crd: &Value) -> Value {
    let mut spec = crd.get("spec").cloned().unwrap_or(Value::Null);
    if let Some(spec) = spec.as_object_mut() {
        if spec.get("preserveUnknownFields") == Some(&Value::Bool(false)) {
            spec.remove("preserveUnknownFields");
        }
        if let Some(conversion) = spec.get_mut("conversion").and_then(Value::as_object_mut) {
            if let Some(config) = conversion
                .get_mut("webhook")
                .and_then(|w| w.get_mut("clientConfig"))
                .and_then(Value::as_object_mut)
            {
                // OLM injects the CA bundle and points the service at the operator's.
                config.remove("caBundle");
            }
            if conversion.get("strategy").and_then(Value::as_str) == Some("None")
                && conversion.len() == 1
            {
                spec.remove("conversion");
            }
        }
    }
    kubyl_yaml::render::sorted(spec)
}

/// Versions a CRD serves and stores: `(name, served, storage)`.
fn versions(crd: &Value) -> Vec<(String, bool, bool)> {
    crd.pointer("/spec/versions")
        .and_then(Value::as_array)
        .map(|versions| {
            versions
                .iter()
                .filter_map(|v| {
                    Some((
                        v.get("name")?.as_str()?.to_string(),
                        v.get("served").and_then(Value::as_bool).unwrap_or(false),
                        v.get("storage").and_then(Value::as_bool).unwrap_or(false),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Compares the live CRD (if any) with the bundle's.
pub fn crd_change(name: &str, live: Option<&Value>, new: &Value) -> CrdChange {
    let new_yaml = kubyl_yaml::render::to_yaml(&normalize_spec(new));
    let Some(live) = live else {
        let diff = kubyl_yaml::diff::diff("", &new_yaml, 3);
        return CrdChange {
            name: name.to_string(),
            status: CrdStatus::New,
            summary: "new CRD".into(),
            diff,
            warnings: Vec::new(),
        };
    };
    let old_yaml = kubyl_yaml::render::to_yaml(&normalize_spec(live));
    let diff = kubyl_yaml::diff::diff(&old_yaml, &new_yaml, 3);
    let mut warnings = Vec::new();
    let mut notes = Vec::new();
    let old_versions = versions(live);
    let new_versions = versions(new);
    let stored: Vec<String> = live
        .pointer("/status/storedVersions")
        .and_then(Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for (version, served, _) in &old_versions {
        let now = new_versions.iter().find(|(v, _, _)| v == version);
        let dropped = now.is_none();
        let unserved = *served && now.is_some_and(|(_, s, _)| !s);
        if dropped || unserved {
            notes.push(format!(
                "{version} {}",
                if dropped {
                    "removed"
                } else {
                    "no longer served"
                }
            ));
            if dropped && stored.contains(version) {
                warnings.push(format!(
                    "{version} is removed but still listed in storedVersions: OLM refuses the upgrade until the objects are migrated."
                ));
            } else if unserved {
                warnings.push(format!(
                    "{version} stops being served: clients and manifests using it break."
                ));
            }
        }
    }
    for (version, served, _) in &new_versions {
        if *served && !old_versions.iter().any(|(v, s, _)| v == version && *s) {
            notes.push(format!("serves {version}"));
        }
    }
    let old_storage = old_versions.iter().find(|v| v.2).map(|v| v.0.clone());
    let new_storage = new_versions.iter().find(|v| v.2).map(|v| v.0.clone());
    if old_storage != new_storage
        && let Some(storage) = &new_storage
    {
        notes.push(format!("stores {storage}"));
    }
    let (added, removed) = (diff.added, diff.removed);
    let status = if diff.is_empty() {
        CrdStatus::Unchanged
    } else {
        CrdStatus::Changed
    };
    let mut summary = match (added, removed) {
        (0, 0) => "unchanged".to_string(),
        (a, 0) => format!("+{a} lines"),
        (0, r) => format!("−{r} lines"),
        (a, r) => format!("+{a} −{r} lines"),
    };
    if !notes.is_empty() {
        summary = format!("{summary} · {}", notes.join(", "));
    }
    CrdChange {
        name: name.to_string(),
        status,
        diff,
        summary,
        warnings,
    }
}

/// Where a permission applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scope {
    Cluster,
    /// The namespaces the operator watches.
    Namespaced,
}

/// A permission the operator gains (`added`) or loses.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RuleChange {
    pub added: bool,
    pub scope: Scope,
    pub service_account: String,
    /// `resource (group)`, `resource/name`, or a non-resource URL.
    pub target: String,
    pub verbs: Vec<String>,
}

type RuleKey = (Scope, String, String);

fn flatten(permissions: &[Permission], scope: Scope) -> BTreeMap<RuleKey, BTreeSet<String>> {
    let mut out: BTreeMap<RuleKey, BTreeSet<String>> = BTreeMap::new();
    for permission in permissions {
        for rule in &permission.rules {
            let mut targets = Vec::new();
            for group in &rule.api_groups {
                for resource in &rule.resources {
                    let base = if group.is_empty() {
                        resource.clone()
                    } else {
                        format!("{resource} ({group})")
                    };
                    if rule.resource_names.is_empty() {
                        targets.push(base);
                    } else {
                        for name in &rule.resource_names {
                            targets.push(format!("{base} named {name}"));
                        }
                    }
                }
            }
            targets.extend(rule.non_resource_urls.iter().cloned());
            for target in targets {
                out.entry((scope, permission.service_account.clone(), target))
                    .or_default()
                    .extend(rule.verbs.iter().cloned());
            }
        }
    }
    out
}

/// Permissions added and removed between the installed CSV and the new one.
pub fn rbac_changes(old: Option<&Csv>, new: &Csv) -> Vec<RuleChange> {
    let collect = |csv: &Csv| {
        let mut all = flatten(&csv.cluster_permissions, Scope::Cluster);
        all.extend(flatten(&csv.permissions, Scope::Namespaced));
        all
    };
    let before = old.map(collect).unwrap_or_default();
    let after = collect(new);
    let mut out = Vec::new();
    for (key, verbs) in &after {
        let had = before.get(key);
        let gained: Vec<String> = verbs
            .iter()
            .filter(|v| had.is_none_or(|h| !h.contains(*v) && !h.contains("*")))
            .cloned()
            .collect();
        if !gained.is_empty() {
            out.push(RuleChange {
                added: true,
                scope: key.0,
                service_account: key.1.clone(),
                target: key.2.clone(),
                verbs: gained,
            });
        }
    }
    for (key, verbs) in &before {
        let has = after.get(key);
        let lost: Vec<String> = verbs
            .iter()
            .filter(|v| has.is_none_or(|h| !h.contains(*v) && !h.contains("*")))
            .cloned()
            .collect();
        if !lost.is_empty() {
            out.push(RuleChange {
                added: false,
                scope: key.0,
                service_account: key.1.clone(),
                target: key.2.clone(),
                verbs: lost,
            });
        }
    }
    out.sort_by(|a, b| {
        b.added
            .cmp(&a.added)
            .then(a.scope.cmp(&b.scope))
            .then(a.target.cmp(&b.target))
    });
    out
}

/// A version as numbers: `v1.30.4-eks` → `[1, 30, 4]`, `4.15` → `[4, 15]`.
pub fn version_numbers(version: &str) -> Vec<u64> {
    version
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .map_while(|part| part.parse().ok())
        .collect()
}

/// `a` compared with `b`, missing parts counting as 0.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (version_numbers(a), version_numbers(b));
    let len = a.len().max(b.len());
    for i in 0..len {
        let ord = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if ord.is_ne() {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// A compatibility note: fine, a warning, or a blocker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub level: Level,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warning,
    Blocker,
}

/// What the cluster runs, for the compatibility notes.
#[derive(Clone, Debug, Default)]
pub struct ClusterFacts {
    /// `v1.30.4`.
    pub kube_version: Option<String>,
    /// OpenShift's version (`4.16.3`), when it is OpenShift.
    pub openshift_version: Option<String>,
}

/// Compatibility of the new CSV with the cluster and the current install.
pub fn compatibility(new: &Csv, installed: Option<&Csv>, facts: &ClusterFacts) -> Vec<Note> {
    let mut notes = Vec::new();
    if let (Some(min), Some(kube)) = (&new.min_kube_version, &facts.kube_version) {
        let min_label = min.trim_end_matches("-0");
        if compare_versions(kube, min).is_lt() {
            notes.push(Note {
                level: Level::Blocker,
                text: format!("Kubernetes {kube} is older than minKubeVersion {min_label}."),
            });
        } else {
            notes.push(Note {
                level: Level::Ok,
                text: format!("Kubernetes {kube} meets minKubeVersion {min_label}."),
            });
        }
    }
    if let (Some(max), Some(ocp)) = (new.max_openshift_version(), &facts.openshift_version) {
        // `4.15` allows every 4.15.z.
        let current: Vec<u64> = version_numbers(ocp).into_iter().take(2).collect();
        let limit = version_numbers(&max);
        if current > limit {
            notes.push(Note {
                level: Level::Warning,
                text: format!("OpenShift {ocp} is newer than its maxOpenShiftVersion {max}."),
            });
        } else {
            notes.push(Note {
                level: Level::Ok,
                text: format!("maxOpenShiftVersion {max}: OpenShift upgrades past it are blocked."),
            });
        }
    }
    if let Some(installed) = installed {
        let mode = match installed.target_namespaces.as_deref() {
            None | Some("") => InstallMode::AllNamespaces,
            Some(targets) if targets.contains(',') => InstallMode::MultiNamespace,
            Some(target) if target == installed.namespace => InstallMode::OwnNamespace,
            Some(_) => InstallMode::SingleNamespace,
        };
        if !new.install_modes.is_empty() && !new.install_modes.contains(&mode) {
            notes.push(Note {
                level: Level::Blocker,
                text: format!(
                    "The new version doesn't support the install mode it runs in ({}).",
                    mode.label().to_lowercase()
                ),
            });
        } else if !new.install_modes.is_empty() {
            notes.push(Note {
                level: Level::Ok,
                text: format!(
                    "The install mode ({}) is still supported.",
                    mode.label().to_lowercase()
                ),
            });
        }
    }
    notes
}

/// Everything the review dialog shows.
#[derive(Clone, Debug, Default)]
pub struct Review {
    pub crds: Vec<CrdChange>,
    pub rbac: Vec<RuleChange>,
    pub notes: Vec<Note>,
    /// The other steps: `(kind, name, action)` (`create`, `update`, the step status).
    pub other_steps: Vec<(String, String, String)>,
    /// The new CSV's version.
    pub version: Option<String>,
    /// What couldn't be read (bundle ConfigMap forbidden…).
    pub problems: Vec<String>,
}

impl Review {
    pub fn blockers(&self) -> usize {
        self.notes
            .iter()
            .filter(|n| n.level == Level::Blocker)
            .count()
    }

    pub fn changed_crds(&self) -> usize {
        self.crds
            .iter()
            .filter(|c| c.status != CrdStatus::Unchanged)
            .count()
    }
}

/// The new manifest of a step: inline, or from its bundle ConfigMap (read once per bundle).
async fn step_object(
    client: &kube::Client,
    bundles: &mut HashMap<BundleRef, Result<Vec<Value>, String>>,
    kind: &str,
    name: &str,
    manifest: &str,
) -> Result<Value, String> {
    let Some(reference) = BundleRef::parse(manifest) else {
        return serde_json::from_str(manifest).map_err(|e| format!("{kind} {name}: {e}"));
    };
    if !bundles.contains_key(&reference) {
        let api: Api<ConfigMap> = Api::namespaced(client.clone(), &reference.namespace);
        let objects = match api.get(&reference.name).await {
            Ok(cm) => decode_bundle(&cm),
            Err(err) => Err(crate::errors::describe(
                &err,
                "get",
                "configmaps",
                Some(&reference.namespace),
            )),
        };
        bundles.insert(reference.clone(), objects);
    }
    match &bundles[&reference] {
        Ok(objects) => find(objects, kind, name)
            .cloned()
            .ok_or_else(|| format!("{kind} {name} isn't in the unpacked bundle")),
        Err(err) => Err(format!("The unpacked bundle can't be read. {err}")),
    }
}

/// Builds the review of `plan` (on Tokio). `installed`: the CSV it replaces.
pub async fn review(
    client: kube::Client,
    plan: InstallPlan,
    installed: Option<Csv>,
    mut facts: ClusterFacts,
    openshift: bool,
) -> Review {
    let mut out = Review::default();
    let mut bundles = HashMap::new();
    if openshift && facts.openshift_version.is_none() {
        facts.openshift_version = openshift_version(&client).await;
    }
    let crd_api: Api<kube::api::DynamicObject> = Api::all_with(
        client.clone(),
        &kube::api::ApiResource::erase::<
            k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
        >(&()),
    );
    let mut new_csv: Option<Csv> = None;
    for step in &plan.steps {
        match step.kind.as_str() {
            "CustomResourceDefinition" => {
                let new = step_object(
                    &client,
                    &mut bundles,
                    &step.kind,
                    &step.name,
                    &step.manifest,
                )
                .await;
                match new {
                    Ok(new) => {
                        let live = match crd_api.get_opt(&step.name).await {
                            Ok(live) => live.and_then(|o| serde_json::to_value(o).ok()),
                            Err(err) => {
                                out.problems.push(crate::errors::describe(
                                    &err,
                                    "get",
                                    "customresourcedefinitions",
                                    None,
                                ));
                                None
                            }
                        };
                        out.crds.push(crd_change(&step.name, live.as_ref(), &new));
                    }
                    Err(err) => {
                        if !out.problems.contains(&err) {
                            out.problems.push(err);
                        }
                        out.crds.push(CrdChange {
                            name: step.name.clone(),
                            status: CrdStatus::Unknown,
                            diff: LineDiff::default(),
                            summary: "couldn't read the new definition".into(),
                            warnings: Vec::new(),
                        });
                    }
                }
            }
            "ClusterServiceVersion" => {
                match step_object(
                    &client,
                    &mut bundles,
                    &step.kind,
                    &step.name,
                    &step.manifest,
                )
                .await
                {
                    Ok(object) => {
                        if new_csv.is_none() || plan.csv_names.first() == Some(&step.name) {
                            new_csv = Csv::parse(&object);
                        }
                    }
                    Err(err) => {
                        if !out.problems.contains(&err) {
                            out.problems.push(err);
                        }
                    }
                }
                out.other_steps.push((
                    step.kind.clone(),
                    step.name.clone(),
                    step_action(&step.status),
                ));
            }
            _ => out.other_steps.push((
                step.kind.clone(),
                step.name.clone(),
                step_action(&step.status),
            )),
        }
    }
    if let Some(new) = &new_csv {
        out.version = new.version.clone();
        out.rbac = rbac_changes(installed.as_ref(), new);
        out.notes = compatibility(new, installed.as_ref(), &facts);
    }
    for crd in &out.crds {
        for warning in &crd.warnings {
            out.notes.push(Note {
                level: if warning.contains("OLM refuses") {
                    Level::Blocker
                } else {
                    Level::Warning
                },
                text: format!("{}: {warning}", crd.name),
            });
        }
    }
    out
}

fn step_action(status: &str) -> String {
    match status {
        "Unknown" | "NotPresent" | "" => "create".into(),
        "Present" => "update".into(),
        other => other.to_lowercase(),
    }
}

/// OpenShift's version (`ClusterVersion/version`), if readable.
async fn openshift_version(client: &kube::Client) -> Option<String> {
    let request = http::Request::get("/apis/config.openshift.io/v1/clusterversions/version")
        .body(Vec::new())
        .ok()?;
    let value: Value = client.request(request).await.ok()?;
    value
        .pointer("/status/desired/version")
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::ByteString;
    use serde_json::json;

    fn crd(versions: Value, stored: &[&str], extra: Value) -> Value {
        let mut spec = json!({"group": "kafka.strimzi.io", "names": {"kind": "Kafka", "plural": "kafkas"},
                              "scope": "Namespaced", "versions": versions});
        if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
            spec.extend(extra.clone());
        }
        json!({"apiVersion": "apiextensions.k8s.io/v1", "kind": "CustomResourceDefinition",
               "metadata": {"name": "kafkas.kafka.strimzi.io"}, "spec": spec,
               "status": {"storedVersions": stored}})
    }

    #[test]
    fn crd_changes_ignore_server_defaults_and_flag_dropped_versions() {
        let v = |name: &str, served: bool, storage: bool, fields: Value| {
            json!({"name": name, "served": served, "storage": storage,
                   "schema": {"openAPIV3Schema": {"type": "object", "properties": fields}}})
        };
        let live = crd(
            json!([
                v("v1beta1", true, false, json!({})),
                v("v1beta2", true, true, json!({"spec": {"type": "object"}}))
            ]),
            &["v1beta1", "v1beta2"],
            json!({"preserveUnknownFields": false, "conversion": {"strategy": "None"}}),
        );
        // Same definition as the bundle writes it: no change.
        let same = crd(
            json!([
                v("v1beta1", true, false, json!({})),
                v("v1beta2", true, true, json!({"spec": {"type": "object"}}))
            ]),
            &[],
            json!({}),
        );
        let unchanged = crd_change("kafkas.kafka.strimzi.io", Some(&live), &same);
        assert_eq!(
            unchanged.status,
            CrdStatus::Unchanged,
            "{:?}",
            unchanged.diff
        );
        // A new field and v1beta1 dropped while still stored.
        let new = crd(
            json!([v(
                "v1beta2",
                true,
                true,
                json!({"spec": {"type": "object"}, "tieredStorage": {"type": "object"}})
            )]),
            &[],
            json!({}),
        );
        let change = crd_change("kafkas.kafka.strimzi.io", Some(&live), &new);
        assert_eq!(change.status, CrdStatus::Changed);
        assert!(
            change.summary.contains("v1beta1 removed"),
            "{}",
            change.summary
        );
        assert!(change.warnings[0].contains("OLM refuses"));
        assert!(!crd_change("x", None, &new).diff.is_empty());
    }

    #[test]
    fn bundles_decode_gzip_base64_files() {
        use std::io::Write as _;
        let yaml = "apiVersion: apiextensions.k8s.io/v1\nkind: CustomResourceDefinition\nmetadata:\n  name: kafkas.kafka.strimzi.io\n";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(yaml.as_bytes()).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(gz.finish().unwrap());
        let cm = ConfigMap {
            metadata: kube::api::ObjectMeta {
                annotations: Some(
                    [("olm.contentEncoding".to_string(), "gzip+base64".to_string())].into(),
                ),
                ..Default::default()
            },
            binary_data: Some(
                [("kafkas.yaml".to_string(), ByteString(encoded.into_bytes()))].into(),
            ),
            ..Default::default()
        };
        let objects = decode_bundle(&cm).unwrap();
        assert!(
            find(
                &objects,
                "CustomResourceDefinition",
                "kafkas.kafka.strimzi.io"
            )
            .is_some()
        );
        // Plain `data` (older OLM).
        let plain = ConfigMap {
            data: Some([("crd.yaml".to_string(), yaml.to_string())].into()),
            ..Default::default()
        };
        assert_eq!(decode_bundle(&plain).unwrap().len(), 1);
    }

    #[test]
    fn rbac_changes_and_compatibility() {
        let csv = |cluster_rules: Value, rules: Value, modes: Value, extra: Value| {
            let mut spec = json!({"version": "1.0.0", "installModes": modes, "install": {"spec": {
                "clusterPermissions": [{"serviceAccountName": "op", "rules": cluster_rules}],
                "permissions": [{"serviceAccountName": "op", "rules": rules}]}}});
            if let (Some(spec), Some(extra)) = (spec.as_object_mut(), extra.as_object()) {
                spec.extend(extra.clone());
            }
            Csv::parse(&json!({"metadata": {"name": "op.v1", "namespace": "kafka",
                "annotations": {"olm.targetNamespaces": "kafka",
                    "olm.properties": "[{\"type\":\"olm.maxOpenShiftVersion\",\"value\":\"4.15\"}]"}},
                "spec": spec}))
            .unwrap()
        };
        let own = json!([{"type": "OwnNamespace", "supported": true}]);
        let old = csv(
            json!([{"apiGroups": [""], "resources": ["namespaces"], "verbs": ["get"]}]),
            json!([{"apiGroups": [""], "resources": ["pods/exec"], "verbs": ["create", "delete"]}]),
            own.clone(),
            json!({}),
        );
        let new = csv(
            json!([{"apiGroups": [""], "resources": ["namespaces"], "verbs": ["get"]},
                   {"apiGroups": [""], "resources": ["nodes"], "verbs": ["get", "list"]}]),
            json!([{"apiGroups": [""], "resources": ["pods/exec"], "verbs": ["create"]},
                   {"apiGroups": ["events.k8s.io"], "resources": ["events"], "verbs": ["create"]}]),
            json!([{"type": "AllNamespaces", "supported": true}]),
            json!({"minKubeVersion": "1.25.0"}),
        );
        let changes = rbac_changes(Some(&old), &new);
        let text: Vec<String> = changes
            .iter()
            .map(|c| {
                format!(
                    "{}{:?} {} {}",
                    if c.added { "+" } else { "-" },
                    c.scope,
                    c.target,
                    c.verbs.join(",")
                )
            })
            .collect();
        assert_eq!(
            text,
            [
                "+Cluster nodes get,list",
                "+Namespaced events (events.k8s.io) create",
                "-Namespaced pods/exec delete"
            ]
        );
        let facts = ClusterFacts {
            kube_version: Some("v1.30.4".into()),
            openshift_version: Some("4.16.2".into()),
        };
        let notes = compatibility(&new, Some(&old), &facts);
        assert_eq!(notes[0].level, Level::Ok, "{notes:?}");
        assert_eq!(notes[1].level, Level::Warning, "{notes:?}");
        assert_eq!(notes[2].level, Level::Blocker, "{notes:?}");
        assert!(compare_versions("v1.30.4", "1.19.0-0").is_gt());
        assert!(compare_versions("1.25", "1.25.0").is_eq());
    }
}
