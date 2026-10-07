//! What the details views show per kind, read from the spec and status. Values that come from
//! Secrets are never read: post-build substitutions from Secrets and HelmRelease `valuesFrom`
//! show their names only, Receivers their webhook path (never the token), URLs without
//! credentials.

use serde_json::Value;

use crate::kinds::FluxKind;
use crate::links::strip_credentials;
use crate::model::{FluxObject, ObjectRef, latest_image, str_of};

/// A labelled value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact {
    pub label: &'static str,
    pub value: String,
    /// Shown in the monospace font.
    pub mono: bool,
}

fn fact(label: &'static str, value: impl Into<String>) -> Fact {
    Fact {
        label,
        value: value.into(),
        mono: false,
    }
}

fn mono(label: &'static str, value: impl Into<String>) -> Fact {
    Fact {
        label,
        value: value.into(),
        mono: true,
    }
}

fn bool_of(value: &Value, pointer: &str) -> Option<bool> {
    value.pointer(pointer).and_then(Value::as_bool)
}

fn text_of(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

/// A reference to a ConfigMap or Secret whose content Kubyl doesn't show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataRef {
    /// `ConfigMap` or `Secret`.
    pub kind: String,
    pub name: String,
    /// `valuesKey` / `targetPath` of a HelmRelease's `valuesFrom`.
    pub key: Option<String>,
    pub optional: bool,
}

impl DataRef {
    fn parse(value: &Value) -> Option<DataRef> {
        Some(DataRef {
            kind: str_of(value, "kind").unwrap_or_else(|| "ConfigMap".into()),
            name: str_of(value, "name")?,
            key: str_of(value, "valuesKey").or_else(|| str_of(value, "targetPath")),
            optional: value.get("optional").and_then(Value::as_bool) == Some(true),
        })
    }

    pub fn is_secret(&self) -> bool {
        self.kind == "Secret"
    }
}

/// A Kustomization's spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KustomizationSpec {
    pub path: String,
    pub prune: bool,
    pub target_namespace: Option<String>,
    pub force: bool,
    pub wait: bool,
    pub timeout: Option<String>,
    pub retry_interval: Option<String>,
    pub service_account: Option<String>,
    /// Inline `postBuild.substitute`: names and values (they're in the spec in plain text).
    pub substitutions: Vec<(String, String)>,
    /// `postBuild.substituteFrom`: names only.
    pub substitute_from: Vec<DataRef>,
    pub components: Vec<String>,
}

impl KustomizationSpec {
    pub fn parse(object: &FluxObject) -> KustomizationSpec {
        let spec = object.raw.get("spec").unwrap_or(&Value::Null);
        let mut substitutions: Vec<(String, String)> = spec
            .pointer("/postBuild/substitute")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        substitutions.sort();
        KustomizationSpec {
            path: str_of(spec, "path").unwrap_or_else(|| "./".into()),
            prune: bool_of(spec, "/prune").unwrap_or(false),
            target_namespace: str_of(spec, "targetNamespace"),
            force: bool_of(spec, "/force").unwrap_or(false),
            wait: bool_of(spec, "/wait").unwrap_or(false),
            timeout: str_of(spec, "timeout"),
            retry_interval: str_of(spec, "retryInterval"),
            service_account: str_of(spec, "serviceAccountName"),
            substitutions,
            substitute_from: spec
                .pointer("/postBuild/substituteFrom")
                .and_then(Value::as_array)
                .map(|refs| refs.iter().filter_map(DataRef::parse).collect())
                .unwrap_or_default(),
            components: spec
                .get("components")
                .and_then(Value::as_array)
                .map(|c| {
                    c.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// One entry of a Kustomization's `status.history` (Flux 2.7+).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KustomizationHistory {
    pub revision: String,
    pub first: Option<String>,
    pub last: Option<String>,
    pub status: String,
    pub total: i64,
    pub duration: Option<String>,
}

pub fn kustomization_history(object: &FluxObject) -> Vec<KustomizationHistory> {
    object
        .raw
        .pointer("/status/history")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter(|e| e.get("chartVersion").is_none())
                .map(|e| KustomizationHistory {
                    revision: text_of(e, "/metadata/revision").unwrap_or_default(),
                    first: str_of(e, "firstReconciled"),
                    last: str_of(e, "lastReconciled"),
                    status: str_of(e, "lastReconciledStatus").unwrap_or_default(),
                    total: e
                        .get("totalReconciliations")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    duration: str_of(e, "lastReconciledDuration"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A HelmRelease's chart and settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmReleaseSpec {
    /// The chart's name (`spec.chart.spec.chart`), or the `chartRef` source's name.
    pub chart: String,
    /// The version constraint (`6.x`), when set.
    pub version: Option<String>,
    pub source: Option<ObjectRef>,
    /// The chart comes from `spec.chartRef` (an OCIRepository or HelmChart).
    pub chart_ref: bool,
    pub release_name: String,
    pub target_namespace: Option<String>,
    pub storage_namespace: Option<String>,
    pub values_from: Vec<DataRef>,
    /// Top-level keys of the inline `spec.values` (their values aren't shown here).
    pub inline_values: Vec<String>,
    pub install_retries: Option<i64>,
    pub upgrade_retries: Option<i64>,
    pub remediate_last_failure: Option<bool>,
    /// `rollback` or `uninstall` (upgrade remediation strategy).
    pub upgrade_strategy: Option<String>,
    pub drift_detection: Option<String>,
}

impl HelmReleaseSpec {
    pub fn parse(object: &FluxObject) -> HelmReleaseSpec {
        let spec = object.raw.get("spec").unwrap_or(&Value::Null);
        let chart_ref = spec.get("chartRef").is_some();
        let target_namespace = str_of(spec, "targetNamespace");
        // helm-controller's `GetReleaseName`: `<targetNamespace>-<name>` when a target namespace
        // is set; then `ShortenName` for Helm's 53-character limit.
        let release_name =
            shorten_release_name(&str_of(spec, "releaseName").unwrap_or_else(|| {
                match &target_namespace {
                    Some(ns) => format!("{ns}-{}", object.name),
                    None => object.name.clone(),
                }
            }));
        let mut inline_values: Vec<String> = spec
            .get("values")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        inline_values.sort();
        HelmReleaseSpec {
            chart: text_of(spec, "/chart/spec/chart")
                .or_else(|| text_of(spec, "/chartRef/name"))
                .unwrap_or_default(),
            version: text_of(spec, "/chart/spec/version"),
            source: object.source.clone(),
            chart_ref,
            release_name,
            storage_namespace: str_of(spec, "storageNamespace")
                .or_else(|| text_of(&object.raw, "/status/storageNamespace")),
            target_namespace,
            values_from: spec
                .get("valuesFrom")
                .and_then(Value::as_array)
                .map(|refs| refs.iter().filter_map(DataRef::parse).collect())
                .unwrap_or_default(),
            inline_values,
            install_retries: spec
                .pointer("/install/remediation/retries")
                .and_then(Value::as_i64),
            upgrade_retries: spec
                .pointer("/upgrade/remediation/retries")
                .and_then(Value::as_i64),
            remediate_last_failure: bool_of(spec, "/upgrade/remediation/remediateLastFailure"),
            upgrade_strategy: text_of(spec, "/upgrade/remediation/strategy"),
            drift_detection: text_of(spec, "/driftDetection/mode"),
        }
    }

    /// `3 retries`, `unlimited`, `none`.
    pub fn retries_label(retries: Option<i64>) -> String {
        match retries {
            None | Some(0) => "none".into(),
            Some(n) if n < 0 => "unlimited".into(),
            Some(1) => "1 retry".into(),
            Some(n) => format!("{n} retries"),
        }
    }
}

/// helm-controller's `release.ShortenName`: names over 53 characters become their first 40
/// characters, `-` and the first 12 hex digits of their SHA-256.
pub fn shorten_release_name(name: &str) -> String {
    use sha2::{Digest as _, Sha256};
    const MAX: usize = 53;
    const HASH: usize = 12;
    if name.len() <= MAX {
        return name.to_string();
    }
    let sum: String = Sha256::digest(name.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // Release names are ASCII; cut at a char boundary anyway.
    let mut cut = MAX - (HASH + 1);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}-{}", &name[..cut], &sum[..HASH])
}

/// One Helm release revision from `status.history` (v2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmHistory {
    pub version: i64,
    pub name: String,
    pub namespace: String,
    pub chart_name: String,
    pub chart_version: String,
    pub app_version: Option<String>,
    pub status: String,
    pub first_deployed: Option<String>,
    pub last_deployed: Option<String>,
    pub action: Option<String>,
}

impl HelmHistory {
    /// The Secret Helm keeps this revision in (`sh.helm.release.v1.<name>.v<version>`).
    pub fn storage_secret(&self) -> String {
        format!("sh.helm.release.v1.{}.v{}", self.name, self.version)
    }
}

pub fn helm_history(object: &FluxObject) -> Vec<HelmHistory> {
    object
        .raw
        .pointer("/status/history")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| {
                    Some(HelmHistory {
                        version: e.get("version")?.as_i64()?,
                        name: str_of(e, "name")?,
                        namespace: str_of(e, "namespace").unwrap_or_default(),
                        chart_name: str_of(e, "chartName").unwrap_or_default(),
                        chart_version: str_of(e, "chartVersion").unwrap_or_default(),
                        app_version: str_of(e, "appVersion"),
                        status: str_of(e, "status").unwrap_or_default(),
                        first_deployed: str_of(e, "firstDeployed"),
                        last_deployed: str_of(e, "lastDeployed"),
                        action: str_of(e, "action"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Failure counters (`installFailures`, `upgradeFailures`, `failures`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Failures {
    pub install: i64,
    pub upgrade: i64,
    pub total: i64,
}

pub fn failures(object: &FluxObject) -> Failures {
    let count = |key: &str| {
        object
            .raw
            .pointer(&format!("/status/{key}"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    Failures {
        install: count("installFailures"),
        upgrade: count("upgradeFailures"),
        total: count("failures"),
    }
}

/// The Helm release behind a HelmRelease: its storage namespace and the Secret of its latest
/// revision (from `status.history`, or `lastReleaseRevision` of `v2beta1`). Helm keeps the
/// release in `spec.storageNamespace` (or what `status.storageNamespace` recorded), else in the
/// HelmRelease's own namespace: in every version, never the target namespace (the history's
/// `namespace` is the release's, where its objects go).
pub fn helm_storage(object: &FluxObject) -> Option<(String, String)> {
    let spec = HelmReleaseSpec::parse(object);
    let namespace = spec
        .storage_namespace
        .clone()
        .unwrap_or_else(|| object.namespace.clone());
    if let Some(latest) = helm_history(object).into_iter().next() {
        return Some((namespace, latest.storage_secret()));
    }
    let revision = object
        .raw
        .pointer("/status/lastReleaseRevision")
        .and_then(Value::as_i64)?;
    Some((
        namespace,
        format!("sh.helm.release.v1.{}.v{revision}", spec.release_name),
    ))
}

/// The facts of a source, image or notification object (Kustomizations and HelmReleases have
/// their own sections).
pub fn facts(object: &FluxObject) -> Vec<Fact> {
    let raw = &object.raw;
    let spec = raw.get("spec").unwrap_or(&Value::Null);
    let status = raw.get("status").unwrap_or(&Value::Null);
    let mut facts = Vec::new();
    let url = |facts: &mut Vec<Fact>, key: &'static str| {
        if let Some(url) = str_of(spec, key) {
            facts.push(mono("URL", strip_credentials(&url)));
        }
    };
    let secret = |facts: &mut Vec<Fact>| {
        if let Some(name) = text_of(spec, "/secretRef/name") {
            facts.push(mono("Secret", format!("{name} (not shown)")));
        }
    };
    match object.kind {
        FluxKind::GitRepository => {
            url(&mut facts, "url");
            if let Some(reference) = git_ref(spec) {
                facts.push(mono("Ref", reference));
            }
            secret(&mut facts);
        }
        FluxKind::OCIRepository => {
            url(&mut facts, "url");
            if let Some(reference) = oci_ref(spec) {
                facts.push(mono("Ref", reference));
            }
            if let Some(provider) = str_of(spec, "provider") {
                facts.push(fact("Provider", provider));
            }
            secret(&mut facts);
        }
        FluxKind::HelmRepository => {
            url(&mut facts, "url");
            facts.push(fact(
                "Type",
                str_of(spec, "type").unwrap_or_else(|| "default (HTTP index)".into()),
            ));
            secret(&mut facts);
        }
        FluxKind::HelmChart => {
            if let Some(chart) = str_of(spec, "chart") {
                facts.push(mono("Chart", chart));
            }
            if let Some(version) = str_of(spec, "version") {
                facts.push(mono("Version", version));
            }
            if let Some(source) = &object.source {
                facts.push(mono("Source", source.label(&object.namespace)));
            }
        }
        FluxKind::Bucket => {
            if let Some(endpoint) = str_of(spec, "endpoint") {
                facts.push(mono("Endpoint", strip_credentials(&endpoint)));
            }
            if let Some(bucket) = str_of(spec, "bucketName") {
                facts.push(mono("Bucket", bucket));
            }
            if let Some(provider) = str_of(spec, "provider") {
                facts.push(fact("Provider", provider));
            }
            secret(&mut facts);
        }
        FluxKind::ExternalArtifact => {
            if let Some(source) = text_of(spec, "/sourceRef/name") {
                facts.push(mono(
                    "Producer",
                    format!(
                        "{}/{source}",
                        text_of(spec, "/sourceRef/kind").unwrap_or_default()
                    ),
                ));
            }
        }
        FluxKind::ImageRepository => {
            if let Some(image) = str_of(spec, "image") {
                facts.push(mono("Image", image));
            }
            if let Some(count) = text_of(status, "/lastScanResult/tagCount") {
                facts.push(fact("Tags", count));
            }
            if let Some(scan) = text_of(status, "/lastScanResult/scanTime") {
                facts.push(mono("Last scan", scan));
            }
            secret(&mut facts);
        }
        FluxKind::ImagePolicy => {
            if let Some(latest) = latest_image(status) {
                facts.push(mono("Latest image", latest));
            }
            if let Some(policy) = image_policy(spec) {
                facts.push(mono("Policy", policy));
            }
            if let Some(repository) = &object.source {
                facts.push(mono("Repository", repository.label(&object.namespace)));
            }
        }
        FluxKind::ImageUpdateAutomation => {
            if let Some(source) = &object.source {
                facts.push(mono("Source", source.label(&object.namespace)));
            }
            if let Some(branch) = text_of(spec, "/git/checkout/ref/branch") {
                facts.push(mono("Checkout", branch));
            }
            if let Some(branch) = text_of(spec, "/git/push/branch") {
                facts.push(mono("Push to", branch));
            }
            if let Some(path) = text_of(spec, "/update/path") {
                facts.push(mono("Path", path));
            }
            if let Some(commit) = str_of(status, "lastPushCommit") {
                facts.push(mono("Last push", commit));
            }
            if let Some(time) = str_of(status, "lastPushTime") {
                facts.push(mono("Pushed at", time));
            }
            if let Some(time) = str_of(status, "lastAutomationRunTime") {
                facts.push(mono("Last run", time));
            }
        }
        FluxKind::Alert => {
            if let Some(provider) = &object.source {
                facts.push(mono("Provider", provider.label(&object.namespace)));
            }
            facts.push(fact(
                "Severity",
                str_of(spec, "eventSeverity").unwrap_or_else(|| "info".into()),
            ));
            let sources = event_sources(object);
            if !sources.is_empty() {
                facts.push(mono("Sources", sources.join(", ")));
            }
        }
        FluxKind::Provider => {
            if let Some(kind) = str_of(spec, "type") {
                facts.push(fact("Type", kind));
            }
            if let Some(address) = str_of(spec, "address") {
                facts.push(mono("Address", strip_credentials(&address)));
            }
            if let Some(channel) = str_of(spec, "channel") {
                facts.push(mono("Channel", channel));
            }
            secret(&mut facts);
        }
        FluxKind::Receiver => {
            if let Some(kind) = str_of(spec, "type") {
                facts.push(fact("Type", kind));
            }
            if let Some(path) = str_of(status, "webhookPath") {
                facts.push(mono("Webhook path", path));
            }
            if let Some(events) = spec.get("events").and_then(Value::as_array) {
                let events: Vec<&str> = events.iter().filter_map(Value::as_str).collect();
                if !events.is_empty() {
                    facts.push(mono("Events", events.join(", ")));
                }
            }
            let resources = receiver_resources(object);
            if !resources.is_empty() {
                facts.push(mono("Resources", resources.join(", ")));
            }
            if let Some(name) = text_of(spec, "/secretRef/name") {
                facts.push(mono("Token", format!("in Secret {name} (not shown)")));
            }
        }
        FluxKind::Kustomization | FluxKind::HelmRelease => {}
    }
    if let Some(interval) = &object.interval {
        facts.push(mono("Interval", interval.clone()));
    }
    if let Some(timeout) = str_of(spec, "timeout")
        && object.kind.is_source()
    {
        facts.push(mono("Timeout", timeout));
    }
    facts
}

/// `branch master`, `tag v1.2.0`, `semver >=1.0`, `commit 3e0ff8a`.
pub fn git_ref(spec: &Value) -> Option<String> {
    let reference = spec.get("ref")?;
    for (key, label) in [
        ("commit", "commit"),
        ("name", "ref"),
        ("semver", "semver"),
        ("tag", "tag"),
        ("branch", "branch"),
    ] {
        if let Some(value) = str_of(reference, key) {
            return Some(format!("{label} {value}"));
        }
    }
    None
}

/// `tag latest`, `semver 6.x`, `digest sha256:…`.
pub fn oci_ref(spec: &Value) -> Option<String> {
    let reference = spec.get("ref")?;
    for key in ["digest", "semver", "tag"] {
        if let Some(value) = str_of(reference, key) {
            return Some(format!("{key} {value}"));
        }
    }
    None
}

/// `semver 6.x`, `alphabetical asc`, `numerical desc`.
pub fn image_policy(spec: &Value) -> Option<String> {
    let policy = spec.get("policy")?;
    if let Some(range) = text_of(policy, "/semver/range") {
        return Some(format!("semver {range}"));
    }
    if let Some(order) = text_of(policy, "/alphabetical/order") {
        return Some(format!("alphabetical {order}"));
    }
    if let Some(order) = text_of(policy, "/numerical/order") {
        return Some(format!("numerical {order}"));
    }
    policy.as_object()?.keys().next().cloned()
}

/// An Alert's event sources (`Kustomization/*`, `HelmRelease/flux-system/podinfo`).
pub fn event_sources(object: &FluxObject) -> Vec<String> {
    object
        .raw
        .pointer("/spec/eventSources")
        .and_then(Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .filter_map(|s| ObjectRef::parse(s, &object.namespace, None))
                .map(|r| r.label(&object.namespace))
                .collect()
        })
        .unwrap_or_default()
}

/// The objects a Receiver reconciles.
pub fn receiver_resources(object: &FluxObject) -> Vec<String> {
    object
        .raw
        .pointer("/spec/resources")
        .and_then(Value::as_array)
        .map(|r| {
            r.iter()
                .filter_map(|s| ObjectRef::parse(s, &object.namespace, None))
                .map(|r| r.label(&object.namespace))
                .collect()
        })
        .unwrap_or_default()
}

/// The repository URL of a source (credentials stripped), for commit links and "uses".
pub fn source_url(object: &FluxObject) -> Option<String> {
    object
        .raw
        .pointer("/spec/url")
        .and_then(Value::as_str)
        .map(strip_credentials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use std::sync::Arc;

    fn parse(value: Value) -> FluxObject {
        FluxObject::parse(&Arc::new(value)).unwrap()
    }

    #[test]
    fn kustomization_spec_names_secrets_only() {
        let ks = parse(fixtures::kustomization_ready());
        let spec = KustomizationSpec::parse(&ks);
        assert_eq!(spec.path, "./kustomize");
        assert!(spec.prune);
        assert_eq!(spec.target_namespace.as_deref(), Some("flux-podinfo"));
        assert_eq!(spec.substitutions, [("cluster_env".into(), "dev".into())]);
        assert_eq!(spec.substitute_from.len(), 1);
        assert!(spec.substitute_from[0].is_secret() && spec.substitute_from[0].optional);
        let history = kustomization_history(&ks);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "ReconciliationSucceeded");
    }

    #[test]
    fn helm_release_details() {
        let hr = parse(fixtures::helm_release_v2());
        let spec = HelmReleaseSpec::parse(&hr);
        assert_eq!(spec.chart, "podinfo");
        assert_eq!(spec.version.as_deref(), Some("6.x"));
        assert_eq!(spec.release_name, "podinfo-helm");
        assert_eq!(spec.values_from.len(), 2);
        assert_eq!(spec.values_from[1].key.as_deref(), Some("values.yaml"));
        assert_eq!(spec.inline_values, ["resources"]);
        assert_eq!(
            HelmReleaseSpec::retries_label(spec.install_retries),
            "3 retries"
        );
        assert_eq!(HelmReleaseSpec::retries_label(Some(-1)), "unlimited");
        assert_eq!(spec.remediate_last_failure, Some(true));
        let history = helm_history(&hr);
        assert_eq!(
            history[0].storage_secret(),
            "sh.helm.release.v1.podinfo-helm.v1"
        );
        assert_eq!(
            helm_storage(&hr),
            Some((
                "flux-demo".into(),
                "sh.helm.release.v1.podinfo-helm.v1".into()
            ))
        );
        // v2beta1: no history; the release revision and the release name. Storage stays in
        // the HelmRelease's namespace though the release targets `web`.
        let old = parse(fixtures::helm_release_v2beta1());
        assert_eq!(
            helm_storage(&old),
            Some(("apps".into(), "sh.helm.release.v1.web.v3".into()))
        );
        let mut stored = fixtures::helm_release_v2beta1();
        stored["spec"]["storageNamespace"] = "helm-storage".into();
        assert_eq!(helm_storage(&parse(stored)).unwrap().0, "helm-storage");
        // v2 with a target namespace and no storage namespace anywhere: its own namespace,
        // not the history's (the release's) namespace.
        let mut targeted = fixtures::helm_release_v2();
        targeted["spec"]["targetNamespace"] = "web".into();
        targeted["status"]["history"][0]["namespace"] = "web".into();
        targeted["status"]
            .as_object_mut()
            .unwrap()
            .remove("storageNamespace");
        assert_eq!(helm_storage(&parse(targeted)).unwrap().0, "flux-demo");
        assert_eq!(
            failures(&old),
            Failures {
                install: 0,
                upgrade: 1,
                total: 2
            }
        );
        let stalled = parse(fixtures::helm_release_stalled());
        assert_eq!(failures(&stalled).install, 4);
        // Without a release name and with a target namespace: Helm's default name.
        let mut value = fixtures::helm_release_v2();
        value["spec"]["targetNamespace"] = "web".into();
        assert_eq!(
            HelmReleaseSpec::parse(&parse(value)).release_name,
            "web-podinfo-helm"
        );
        // Over Helm's 53 characters: shortened like helm-controller does.
        let long = "a-very-long-target-namespace-for-the-platform-team-podinfo-helm";
        let short = shorten_release_name(long);
        assert_eq!(
            short,
            "a-very-long-target-namespace-for-the-pla-1012ac5a7417"
        );
        assert_eq!(shorten_release_name("podinfo"), "podinfo");
    }

    #[test]
    fn source_and_notification_facts_hide_secrets() {
        let get = |value: Value, label: &str| {
            facts(&parse(value))
                .into_iter()
                .find(|f| f.label == label)
                .map(|f| f.value)
        };
        assert_eq!(
            get(fixtures::git_repository_v1beta2(), "URL").as_deref(),
            Some("https://gitlab.example.com/platform/infra.git")
        );
        assert_eq!(
            get(fixtures::git_repository_v1beta2(), "Ref").as_deref(),
            Some("tag v1.2.0")
        );
        assert_eq!(
            get(fixtures::git_repository_v1beta2(), "Secret").as_deref(),
            Some("infra-auth (not shown)")
        );
        assert_eq!(
            get(fixtures::oci_repository(), "Ref").as_deref(),
            Some("tag latest")
        );
        assert_eq!(
            get(fixtures::image_policy(), "Policy").as_deref(),
            Some("semver 6.x")
        );
        assert_eq!(
            get(fixtures::image_update_automation(), "Last push").as_deref(),
            Some("b7c1d2e3f4a5b6c7d8e9f00112233445566778899")
        );
        assert_eq!(
            get(fixtures::alert(), "Sources").as_deref(),
            Some("Kustomization/*, HelmRelease/*")
        );
        let address = get(fixtures::provider(), "Address").unwrap();
        assert!(!address.contains("pass") && !address.contains("abc"));
        let receiver = facts(&parse(fixtures::receiver()));
        assert!(receiver.iter().any(|f| f.label == "Webhook path"));
        assert!(
            receiver
                .iter()
                .all(|f| !f.value.contains("kubyl-dev-not-a-real-token"))
        );
        assert_eq!(
            get(fixtures::helm_chart(), "Source").as_deref(),
            Some("HelmRepository/podinfo")
        );
        assert_eq!(
            get(fixtures::image_repository(), "Tags").as_deref(),
            Some("97")
        );
    }
}
