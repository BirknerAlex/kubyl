//! Provider-independent pre-flight checks (README "Cluster update providers" and "Deprecated
//! and removed APIs"). Everything here runs on Tokio from [`Inputs`] gathered on the UI
//! thread; only the operators check ([`operators`]) reads phase 12's state on the UI thread.
//!
//! - PodDisruptionBudgets that allow no disruption block node drains.
//! - Deprecated APIs still requested: OpenShift's `APIRequestCount`, else Prometheus
//!   (`apiserver_requested_deprecated_apis`), else the API server's `/metrics`.
//! - Removed APIs in Helm releases' manifests and in objects' last-applied configuration.
//! - Node headroom to drain one node per pool; version skew and minor-by-minor updates.

use std::collections::{BTreeMap, HashMap};

use futures::StreamExt as _;
use kube::api::{ApiResource, DynamicObject, ListParams};
use kubyl_core::Gvr;
use kubyl_metrics::prometheus::PromClient;
use kubyl_operators::api::Installed;
use kubyl_operators::helm::decode::Driver;
use kubyl_resources::format::parse_quantity;
use serde_json::Value;

use crate::check::{Check, CheckStatus, Detail, Fix};
use crate::kube_api::{self, array_at, int_at, items, str_at};
use crate::model::{ObjectLink, ProviderKind};
use crate::removed::{self, Removed};
use crate::version::{Version, minor_of};

/// Objects scanned per kind at most (last-applied configurations).
const SCAN_LIMIT: usize = 20_000;
/// Helm releases decoded at the same time.
const HELM_PARALLEL: usize = 6;
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// A Helm release's latest revision (from `kubyl_operators`' Helm snapshot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRef {
    pub namespace: String,
    pub name: String,
    pub driver: Driver,
    /// The storage object of the latest revision.
    pub object: String,
    pub revision: u32,
}

/// Helm releases to scan, or why they can't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelmInput {
    Releases(Vec<ReleaseRef>),
    Problem(String),
}

/// A served kind to scan for last-applied apiVersions.
#[derive(Clone, Debug)]
pub struct ScanKind {
    pub resource: ApiResource,
    pub namespaced: bool,
}

/// What the checks need, gathered on the UI thread.
#[derive(Clone)]
pub struct Inputs {
    pub client: kube::Client,
    pub provider: ProviderKind,
    /// The API server's version (`v1.30.6`).
    pub current_kube: String,
    /// The target as the provider names it (`4.17.12`, `1.31`).
    pub target: String,
    /// The target's Kubernetes minor.
    pub target_kube: Option<(u64, u64)>,
    pub prometheus: Option<PromClient>,
    /// OpenShift's `apirequestcounts.apiserver.openshift.io` is served.
    pub api_request_counts: bool,
    pub scan: Vec<ScanKind>,
    pub helm: HelmInput,
}

/// The target's Kubernetes minor for a provider's version string.
pub fn target_kube_minor(provider: ProviderKind, target: &str) -> Option<(u64, u64)> {
    let version = Version::parse(target)?;
    match provider {
        ProviderKind::OpenShift => crate::version::openshift_kube_minor(&version),
        _ => Some(version.minor_key()),
    }
}

/// The kinds of the removal table the cluster serves in another version, to scan.
pub fn scan_kinds(discovery: &kubyl_kube::discovery::Discovery) -> Vec<ScanKind> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for removal in removed::REMOVED.iter().filter(|r| r.stored) {
        // Events are many and never applied with kubectl.
        if removal.resource == "events" || seen.iter().any(|s| s == removal.resource) {
            continue;
        }
        seen.push(removal.resource.to_string());
        let served = discovery
            .preferred()
            .filter(|r| {
                r.gvr.resource == removal.resource && r.gvk.kind == removal.kind && r.is_listable()
            })
            // The same plural and kind can live in an unrelated group (OpenShift's
            // `ingresses.config.openshift.io` is kind `Ingress`): only the groups the table
            // names for the kind, or its replacement's.
            .find(|r| kind_group(removal, &r.gvr.group));
        if let Some(info) = served {
            out.push(ScanKind {
                resource: kubyl_resources::store::api_resource(info),
                namespaced: info.namespaced,
            });
        }
    }
    out
}

/// Whether `group` serves the kind of `removal`: a group the table lists for it, or the group of
/// its replacement.
fn kind_group(removal: &Removed, group: &str) -> bool {
    removed::REMOVED
        .iter()
        .any(|r| r.resource == removal.resource && r.kind == removal.kind && r.group == group)
        || removal
            .replacement
            .split_once('/')
            .is_some_and(|(g, _)| g == group)
}

/// Runs every Tokio-side check.
pub async fn run(inputs: Inputs) -> Vec<Check> {
    let current = minor_of(&inputs.current_kube);
    let nodes = kube_api::get(&inputs.client, "/api/v1/nodes").await;
    let (pdb, deprecated, helm, stored, capacity) = futures::join!(
        pdbs(&inputs.client),
        deprecated(&inputs),
        helm(&inputs.client, &inputs.helm, current, inputs.target_kube),
        stored(&inputs.client, &inputs.scan, current, inputs.target_kube),
        capacity(&inputs.client, nodes.as_ref(), inputs.provider),
    );
    let skew = match &nodes {
        Ok(list) => skew(
            items(list),
            &inputs.current_kube,
            inputs.target_kube,
            inputs.provider,
        ),
        Err(err) => forbidden_or(
            "version-skew",
            "Version skew",
            err,
            "list nodes (cluster-wide)",
        ),
    };
    vec![pdb, deprecated, helm, stored, capacity, skew]
}

/// A check that couldn't run.
fn forbidden_or(id: &'static str, title: &str, err: &kube::Error, missing: &str) -> Check {
    let summary = if kube_api::is_forbidden(err) {
        format!("Not checked: you can't {missing}.")
    } else {
        format!("Not checked: {err}")
    };
    Check::new(id, title, CheckStatus::Unknown, summary)
}

fn removed_status(removal: &Removed, current: Option<(u64, u64)>) -> CheckStatus {
    match current {
        // Already gone today: a warning (it isn't this update that breaks it).
        Some(current) if removal.removed_in <= current => CheckStatus::Warn,
        _ => CheckStatus::Fail,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

// ----- PodDisruptionBudgets -----

async fn pdbs(client: &kube::Client) -> Check {
    match kube_api::get(client, "/apis/policy/v1/poddisruptionbudgets").await {
        Ok(list) => pdb_check(items(&list)),
        Err(err) => forbidden_or(
            "pdb",
            "PodDisruptionBudgets allow node drain",
            &err,
            "list poddisruptionbudgets.policy cluster-wide",
        ),
    }
}

/// PDBs that allow no disruption while they protect pods block every drain of their nodes.
pub fn pdb_check(pdbs: &[Value]) -> Check {
    let blocking: Vec<&Value> = pdbs
        .iter()
        .filter(|p| {
            int_at(p, "/status/disruptionsAllowed") == Some(0)
                && int_at(p, "/status/expectedPods").unwrap_or_default() > 0
        })
        .collect();
    if blocking.is_empty() {
        return Check::new(
            "pdb",
            "PodDisruptionBudgets allow node drain",
            CheckStatus::Pass,
            match pdbs.len() {
                0 => "No PodDisruptionBudgets.".to_string(),
                n => format!(
                    "{} allow evictions.",
                    plural(n, "PodDisruptionBudget", "PodDisruptionBudgets")
                ),
            },
        );
    }
    let details: Vec<Detail> = blocking
        .iter()
        .map(|p| {
            let ns = str_at(p, "/metadata/namespace").unwrap_or_default();
            let name = str_at(p, "/metadata/name").unwrap_or_default();
            let rule = match (
                p.pointer("/spec/minAvailable"),
                p.pointer("/spec/maxUnavailable"),
            ) {
                (Some(min), _) => format!(
                    "minAvailable {}",
                    min.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| min.to_string())
                ),
                (_, Some(max)) => format!(
                    "maxUnavailable {}",
                    max.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| max.to_string())
                ),
                _ => String::new(),
            };
            Detail::new(
                CheckStatus::Fail,
                format!("{ns}/{name} allows 0 disruptions"),
            )
            .with_sub(format!(
                "{} healthy of {} expected pods · {rule}",
                int_at(p, "/status/currentHealthy").unwrap_or_default(),
                int_at(p, "/status/expectedPods").unwrap_or_default(),
            ))
            .fix(Fix::Open {
                label: "Open PDB".into(),
                link: ObjectLink::new(
                    Gvr::new("policy", "v1", "poddisruptionbudgets"),
                    Some(ns),
                    name,
                ),
            })
        })
        .collect();
    let first = details[0].fix.clone();
    let summary = if blocking.len() == 1 {
        format!(
            "{}; draining its pods' nodes waits until it allows an eviction.",
            details[0].text
        )
    } else {
        format!(
            "{} allow 0 disruptions; draining their pods' nodes waits until they allow evictions.",
            plural(
                blocking.len(),
                "PodDisruptionBudget",
                "PodDisruptionBudgets"
            )
        )
    };
    let mut check = Check::new(
        "pdb",
        if blocking.len() == 1 {
            "PodDisruptionBudget blocks node drain"
        } else {
            "PodDisruptionBudgets block node drain"
        },
        CheckStatus::Fail,
        summary,
    )
    .details(details);
    check.fix = first;
    check
}

// ----- Deprecated APIs still requested -----

/// One deprecated API that clients still request.
#[derive(Clone, Debug, PartialEq)]
pub struct DeprecatedUse {
    pub group: String,
    pub version: String,
    pub resource: String,
    pub removed_in: Option<(u64, u64)>,
    /// Requests in the last 24 h, when the source counts them.
    pub requests: Option<u64>,
    /// Who asked (`user · user agent · N`).
    pub clients: Vec<String>,
}

impl DeprecatedUse {
    fn label(&self) -> String {
        let gv = if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        };
        format!("{gv} {}", self.resource)
    }
}

async fn deprecated(inputs: &Inputs) -> Check {
    let title = "Deprecated APIs still requested";
    let (uses, source) = if inputs.api_request_counts {
        match kube_api::get(
            &inputs.client,
            "/apis/apiserver.openshift.io/v1/apirequestcounts",
        )
        .await
        {
            Ok(list) => (
                parse_api_request_counts(items(&list)),
                "APIRequestCount, last 24 h",
            ),
            Err(err) => {
                return forbidden_or(
                    "deprecated-apis",
                    title,
                    &err,
                    "list apirequestcounts.apiserver.openshift.io",
                );
            }
        }
    } else if let Some(prometheus) = &inputs.prometheus {
        match prometheus_uses(prometheus).await {
            Ok(uses) => (uses, "Prometheus, last 24 h"),
            Err(err) => {
                return Check::new(
                    "deprecated-apis",
                    title,
                    CheckStatus::Unknown,
                    format!("Not checked: Prometheus answered {err}"),
                );
            }
        }
    } else {
        match kube_api::get_text(&inputs.client, "/metrics").await {
            Ok(text) => (
                parse_metrics_text(&text),
                "the API server's /metrics, since it started",
            ),
            Err(err) if kube_api::is_forbidden(&err) => {
                return Check::new(
                    "deprecated-apis",
                    title,
                    CheckStatus::Unknown,
                    "Not checked: no Prometheus was found and you can't get /metrics from the API server (nonResourceURLs /metrics).",
                );
            }
            Err(err) => {
                return Check::new(
                    "deprecated-apis",
                    title,
                    CheckStatus::Unknown,
                    format!("Not checked: {err}"),
                );
            }
        }
    };
    deprecated_check(uses, inputs.target_kube, source)
}

/// The check from the deprecated APIs clients requested.
pub fn deprecated_check(
    mut uses: Vec<DeprecatedUse>,
    target: Option<(u64, u64)>,
    source: &str,
) -> Check {
    let title = "Deprecated APIs still requested";
    uses.retain(|u| u.requests != Some(0));
    if uses.is_empty() {
        return Check::new(
            "deprecated-apis",
            title,
            CheckStatus::Pass,
            format!("No client requested a deprecated API (from {source})."),
        );
    }
    uses.sort_by(|a, b| {
        a.removed_in
            .cmp(&b.removed_in)
            .then(a.label().cmp(&b.label()))
    });
    let breaks = |u: &DeprecatedUse| match (u.removed_in, target) {
        (Some(removed), Some(target)) => removed <= target,
        _ => false,
    };
    let details: Vec<Detail> = uses
        .iter()
        .map(|u| {
            let status = if breaks(u) {
                CheckStatus::Fail
            } else {
                CheckStatus::Warn
            };
            let removed = u
                .removed_in
                .map(|(a, b)| format!("removed in {a}.{b}"))
                .unwrap_or_else(|| "deprecated".into());
            let requests = u
                .requests
                .map(|n| format!(" · {}", plural(n as usize, "request", "requests")))
                .unwrap_or_default();
            let mut detail = Detail::new(status, format!("{} · {removed}{requests}", u.label()));
            if !u.clients.is_empty() {
                detail = detail.with_sub(u.clients.join(", "));
            }
            detail
        })
        .collect();
    let failing = uses.iter().filter(|u| breaks(u)).count();
    let status = if failing > 0 {
        CheckStatus::Fail
    } else {
        CheckStatus::Warn
    };
    let summary = if failing > 0 {
        format!(
            "{} the target removes {} still requested ({source}); those clients break after the update.",
            plural(failing, "API", "APIs"),
            if failing == 1 { "is" } else { "are" }
        )
    } else {
        format!(
            "{} still requested ({source}); a later version removes {}.",
            plural(uses.len(), "deprecated API is", "deprecated APIs are"),
            if uses.len() == 1 { "it" } else { "them" }
        )
    };
    Check::new("deprecated-apis", title, status, summary).details(details)
}

/// `1.32` → `(1, 32)`.
fn release_minor(text: &str) -> Option<(u64, u64)> {
    minor_of(text)
}

/// OpenShift's APIRequestCounts with a `removedInRelease`: requests of the last 24 h and who
/// made them.
pub fn parse_api_request_counts(items: &[Value]) -> Vec<DeprecatedUse> {
    items
        .iter()
        .filter_map(|item| {
            let removed = str_at(item, "/status/removedInRelease")?;
            let name = str_at(item, "/metadata/name")?;
            // `<resource>.<version>.<group>`; the core group has no group part.
            let mut parts = name.splitn(3, '.');
            let resource = parts.next()?.to_string();
            let version = parts.next()?.to_string();
            let group = parts.next().unwrap_or_default().to_string();
            let requests = int_at(item, "/status/requestCount")
                .unwrap_or_default()
                .max(0) as u64;
            let mut by_client: HashMap<(String, String), u64> = HashMap::new();
            for hour in array_at(item, "/status/last24h") {
                for node in array_at(hour, "/byNode") {
                    for user in array_at(node, "/byUser") {
                        let key = (
                            str_at(user, "/username").unwrap_or_default().to_string(),
                            str_at(user, "/userAgent").unwrap_or_default().to_string(),
                        );
                        *by_client.entry(key).or_default() +=
                            int_at(user, "/requestCount").unwrap_or_default().max(0) as u64;
                    }
                }
            }
            let mut clients: Vec<((String, String), u64)> = by_client.into_iter().collect();
            clients.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let clients = clients
                .into_iter()
                .take(3)
                .map(|((user, agent), n)| {
                    let agent = agent.split(' ').next().unwrap_or_default().to_string();
                    if agent.is_empty() {
                        format!("{user} · {n}")
                    } else {
                        format!("{user} · {agent} · {n}")
                    }
                })
                .collect();
            Some(DeprecatedUse {
                group,
                version,
                resource,
                removed_in: release_minor(removed),
                requests: Some(requests),
                clients,
            })
        })
        .collect()
}

async fn prometheus_uses(prometheus: &PromClient) -> Result<Vec<DeprecatedUse>, String> {
    let deprecated = prometheus
        .query("group by (group, version, resource, removed_release) (apiserver_requested_deprecated_apis)")
        .await
        .map_err(|e| e.to_string())?;
    if deprecated.is_empty() {
        return Ok(Vec::new());
    }
    let versions: Vec<String> = {
        let mut v: Vec<String> = deprecated
            .iter()
            .filter_map(|s| s.labels.get("version").cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let counts = prometheus
        .query(&format!(
            "sum by (group, version, resource) (increase(apiserver_request_total{{version=~\"{}\"}}[24h]))",
            versions.join("|")
        ))
        .await
        .map_err(|e| e.to_string())?;
    let count_of = |g: &str, v: &str, r: &str| {
        counts
            .iter()
            .find(|s| {
                s.labels
                    .get("group")
                    .map(String::as_str)
                    .unwrap_or_default()
                    == g
                    && s.labels.get("version").map(String::as_str) == Some(v)
                    && s.labels.get("resource").map(String::as_str) == Some(r)
            })
            .map(|s| s.value.round().max(0.0) as u64)
    };
    let mut out: Vec<DeprecatedUse> = Vec::new();
    for sample in deprecated {
        let label = |k: &str| sample.labels.get(k).cloned().unwrap_or_default();
        let (group, version, resource) = (label("group"), label("version"), label("resource"));
        if out
            .iter()
            .any(|u| u.group == group && u.version == version && u.resource == resource)
        {
            continue;
        }
        out.push(DeprecatedUse {
            requests: Some(count_of(&group, &version, &resource).unwrap_or(0)),
            removed_in: release_minor(&label("removed_release")),
            group,
            version,
            resource,
            clients: Vec::new(),
        });
    }
    Ok(out)
}

/// `apiserver_requested_deprecated_apis{group="…",removed_release="1.32",resource="…",…} 1`
/// lines of the API server's `/metrics` (requested since it started, no counts).
pub fn parse_metrics_text(text: &str) -> Vec<DeprecatedUse> {
    let mut out: Vec<DeprecatedUse> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("apiserver_requested_deprecated_apis{") else {
            continue;
        };
        let Some((labels, value)) = rest.split_once('}') else {
            continue;
        };
        if value.trim().parse::<f64>().unwrap_or_default() <= 0.0 {
            continue;
        }
        let mut map: BTreeMap<&str, String> = BTreeMap::new();
        for pair in labels.split("\",") {
            if let Some((k, v)) = pair.split_once("=\"") {
                map.insert(k.trim_matches(','), v.trim_end_matches('"').to_string());
            }
        }
        let get = |k: &str| map.get(k).cloned().unwrap_or_default();
        let (group, version, resource) = (get("group"), get("version"), get("resource"));
        if out
            .iter()
            .any(|u| u.group == group && u.version == version && u.resource == resource)
        {
            continue;
        }
        out.push(DeprecatedUse {
            removed_in: release_minor(&get("removed_release")),
            group,
            version,
            resource,
            requests: None,
            clients: Vec::new(),
        });
    }
    out
}

// ----- Removed APIs in Helm releases -----

/// One manifest object of a release that uses a removed API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmFinding {
    pub release: ReleaseRef,
    pub api_version: String,
    pub kind: String,
    pub name: String,
    pub removal: Removed,
}

async fn helm(
    client: &kube::Client,
    input: &HelmInput,
    current: Option<(u64, u64)>,
    target: Option<(u64, u64)>,
) -> Check {
    let title = "Helm releases use current APIs";
    let releases = match input {
        HelmInput::Problem(problem) => {
            return Check::new(
                "removed-apis-helm",
                title,
                CheckStatus::Unknown,
                format!("Not checked: {problem}"),
            );
        }
        HelmInput::Releases(releases) => releases.clone(),
    };
    let count = releases.len();
    let results: Vec<Result<Vec<HelmFinding>, (ReleaseRef, String)>> =
        futures::stream::iter(releases.into_iter().map(|release| {
            let client = client.clone();
            async move {
                let loaded = kubyl_operators::helm::service::load(
                    client,
                    release.driver,
                    release.namespace.clone(),
                    release.object.clone(),
                )
                .await;
                match loaded {
                    // Only what uses a removed API is kept; the decoded release is dropped here.
                    Ok(decoded) => {
                        let mut manifests = vec![decoded.manifest.as_str()];
                        manifests.extend(decoded.hooks.iter().map(|h| h.manifest.as_str()));
                        Ok(helm_findings(&release, &manifests, target))
                    }
                    Err(err) => Err((release, err)),
                }
            }
        }))
        .buffer_unordered(HELM_PARALLEL)
        .collect()
        .await;
    let mut findings: Vec<HelmFinding> = Vec::new();
    let mut failed: Vec<(ReleaseRef, String)> = Vec::new();
    for result in results {
        match result {
            Ok(mut found) => findings.append(&mut found),
            Err(err) => failed.push(err),
        }
    }
    helm_check(count, findings, failed, current)
}

/// The objects of a release's manifests that use an API removed up to `target`.
pub fn helm_findings(
    release: &ReleaseRef,
    manifests: &[&str],
    target: Option<(u64, u64)>,
) -> Vec<HelmFinding> {
    let mut out = Vec::new();
    for manifest in manifests {
        for object in kubyl_operators::helm::present::manifest_objects(manifest) {
            let Some(removal) = removed::lookup(&object.api_version, &object.kind) else {
                continue;
            };
            if target.is_some_and(|t| removal.removed_in > t) {
                continue;
            }
            out.push(HelmFinding {
                release: release.clone(),
                api_version: object.api_version,
                kind: object.kind,
                name: object.name,
                removal: *removal,
            });
        }
    }
    out
}

/// The Helm check from its findings.
pub fn helm_check(
    releases: usize,
    findings: Vec<HelmFinding>,
    failed: Vec<(ReleaseRef, String)>,
    current: Option<(u64, u64)>,
) -> Check {
    let title = "Helm releases use current APIs";
    let mut by_release: BTreeMap<(String, String), Vec<&HelmFinding>> = BTreeMap::new();
    for finding in &findings {
        by_release
            .entry((
                finding.release.namespace.clone(),
                finding.release.name.clone(),
            ))
            .or_default()
            .push(finding);
    }
    let mut details: Vec<Detail> = by_release
        .values()
        .map(|found| {
            let release = &found[0].release;
            let status = found
                .iter()
                .map(|f| removed_status(&f.removal, current))
                .min()
                .unwrap_or(CheckStatus::Warn);
            let objects: Vec<String> = found
                .iter()
                .map(|f| {
                    format!(
                        "{} {} {} (removed in {}, use {})",
                        f.api_version,
                        f.kind,
                        f.name,
                        f.removal.removed_label(),
                        f.removal.replacement
                    )
                })
                .collect();
            Detail::new(
                status,
                format!(
                    "{}/{} revision {}",
                    release.namespace, release.name, release.revision
                ),
            )
            .with_sub(objects.join("; "))
            .fix(Fix::OpenRelease {
                namespace: release.namespace.clone(),
                name: release.name.clone(),
                object: release.object.clone(),
                secret: release.driver == Driver::Secret,
            })
        })
        .collect();
    for (release, err) in &failed {
        details.push(
            Detail::new(
                CheckStatus::Unknown,
                format!("{}/{} not read", release.namespace, release.name),
            )
            .with_sub(err.clone()),
        );
    }
    if by_release.is_empty() {
        let status = if failed.is_empty() {
            CheckStatus::Pass
        } else {
            CheckStatus::Unknown
        };
        let summary = if failed.is_empty() {
            match releases {
                0 => "No Helm releases.".to_string(),
                n => format!(
                    "{} checked, none uses a removed API.",
                    plural(n, "release", "releases")
                ),
            }
        } else {
            format!(
                "{} of {} couldn't be read.",
                plural(failed.len(), "release", "releases"),
                releases
            )
        };
        return Check::new("removed-apis-helm", title, status, summary).details(details);
    }
    let status = details
        .iter()
        .map(|d| d.status)
        .filter(|s| *s != CheckStatus::Unknown)
        .min()
        .unwrap_or(CheckStatus::Warn);
    let first = details[0].fix.clone();
    let mut check = Check::new(
        "removed-apis-helm",
        "Helm releases use removed APIs",
        status,
        format!(
            "{} of {} {} manifests with removed APIs; helm upgrade fails on them until they're mapped (helm mapkubeapis) and the charts are fixed.",
            by_release.len(),
            plural(releases, "release", "releases"),
            if by_release.len() == 1 { "has" } else { "have" }
        ),
    )
    .details(details);
    check.fix = first;
    check
}

// ----- Removed APIs in last-applied configurations -----

/// An object a client last applied with a removed API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFinding {
    pub link: ObjectLink,
    pub kind: String,
    pub api_version: String,
    pub removal: Removed,
}

async fn stored(
    client: &kube::Client,
    scan: &[ScanKind],
    current: Option<(u64, u64)>,
    target: Option<(u64, u64)>,
) -> Check {
    let mut findings = Vec::new();
    let mut denied = Vec::new();
    // Kinds whose scan stopped early (an error, or the object limit).
    let mut incomplete = Vec::new();
    let mut scanned = 0usize;
    for kind in scan {
        let api: kube::Api<DynamicObject> = kube::Api::all_with(client.clone(), &kind.resource);
        let mut params = ListParams::default().limit(500);
        let mut seen = 0usize;
        loop {
            match api.list_metadata(&params).await {
                Ok(list) => {
                    for object in &list.items {
                        seen += 1;
                        let meta = &object.metadata;
                        let Some(applied) =
                            meta.annotations.as_ref().and_then(|a| a.get(LAST_APPLIED))
                        else {
                            continue;
                        };
                        if let Some(finding) = stored_finding(
                            applied,
                            meta.namespace.as_deref(),
                            meta.name.as_deref(),
                            &kind.resource,
                            target,
                        ) {
                            findings.push(finding);
                        }
                    }
                    match list.metadata.continue_ {
                        Some(token) if !token.is_empty() => {
                            if seen >= SCAN_LIMIT {
                                incomplete.push(format!(
                                    "{} (the first {SCAN_LIMIT} only)",
                                    kind.resource.plural
                                ));
                                break;
                            }
                            params = params.continue_token(&token);
                        }
                        _ => break,
                    }
                }
                Err(err) => {
                    if kube_api::is_forbidden(&err) {
                        denied.push(kind.resource.plural.clone());
                    } else {
                        incomplete.push(format!("{}: {err}", kind.resource.plural));
                    }
                    break;
                }
            }
        }
        scanned += seen;
    }
    stored_check(findings, denied, incomplete, scanned, current)
}

/// A finding from one object's last-applied configuration.
pub fn stored_finding(
    last_applied: &str,
    namespace: Option<&str>,
    name: Option<&str>,
    resource: &ApiResource,
    target: Option<(u64, u64)>,
) -> Option<StoredFinding> {
    // Only the apiVersion and kind are read; the rest (which may hold Secret-like data of
    // other kinds) is dropped with the parsed value.
    let applied: Value = serde_json::from_str(last_applied).ok()?;
    let api_version = str_at(&applied, "/apiVersion")?;
    let kind = str_at(&applied, "/kind")?;
    let removal = removed::lookup(api_version, kind)?;
    if target.is_some_and(|t| removal.removed_in > t) {
        return None;
    }
    Some(StoredFinding {
        link: ObjectLink::new(
            Gvr::new(
                resource.group.clone(),
                resource.version.clone(),
                resource.plural.clone(),
            ),
            namespace,
            name?.to_string(),
        ),
        kind: kind.to_string(),
        api_version: api_version.to_string(),
        removal: *removal,
    })
}

/// The last-applied check from its findings.
pub fn stored_check(
    findings: Vec<StoredFinding>,
    denied: Vec<String>,
    incomplete: Vec<String>,
    scanned: usize,
    current: Option<(u64, u64)>,
) -> Check {
    let title = "Objects last applied with current APIs";
    let mut details: Vec<Detail> = findings
        .iter()
        .take(50)
        .map(|f| {
            let name = match &f.link.namespace {
                Some(ns) => format!("{ns}/{}", f.link.name),
                None => f.link.name.clone(),
            };
            Detail::new(
                removed_status(&f.removal, current),
                format!("{} {name}", f.kind),
            )
            .with_sub(format!(
                "last applied as {} (removed in {}); apply it as {} next time",
                f.api_version,
                f.removal.removed_label(),
                f.removal.replacement
            ))
            .fix(Fix::Open {
                label: "Open".into(),
                link: f.link.clone(),
            })
        })
        .collect();
    if findings.len() > 50 {
        details.push(Detail::new(
            CheckStatus::Warn,
            format!("… {} more", findings.len() - 50),
        ));
    }
    if !denied.is_empty() {
        details.push(
            Detail::new(CheckStatus::Unknown, "Not scanned")
                .with_sub(format!("you can't list {} cluster-wide", denied.join(", "))),
        );
    }
    for kind in &incomplete {
        details.push(Detail::new(CheckStatus::Unknown, "Scan incomplete").with_sub(kind.clone()));
    }
    if findings.is_empty() {
        let (status, summary) = if !denied.is_empty() {
            (
                CheckStatus::Unknown,
                format!("Partly checked: you can't list {}.", denied.join(", ")),
            )
        } else if !incomplete.is_empty() {
            (
                CheckStatus::Unknown,
                format!(
                    "Partly checked: {} scanned, but some kinds couldn't be read to the end.",
                    plural(scanned, "object", "objects")
                ),
            )
        } else {
            (
                CheckStatus::Pass,
                format!(
                    "{} scanned; none was last applied with a removed API.",
                    plural(scanned, "object", "objects")
                ),
            )
        };
        return Check::new("removed-apis-objects", title, status, summary).details(details);
    }
    let status = findings
        .iter()
        .map(|f| removed_status(&f.removal, current))
        .min()
        .unwrap_or(CheckStatus::Warn);
    Check::new(
        "removed-apis-objects",
        "Objects last applied with removed APIs",
        status,
        format!(
            "{} last applied with removed APIs; the manifests that created them fail on the new version.",
            plural(findings.len(), "object was", "objects were")
        ),
    )
    .details(details)
}

// ----- Node headroom -----

/// The pool a node belongs to, by the labels the clouds set.
fn pool_of(node: &Value) -> String {
    let labels = node.pointer("/metadata/labels").and_then(Value::as_object);
    let label = |k: &str| labels.and_then(|l| l.get(k)).and_then(Value::as_str);
    for key in [
        "eks.amazonaws.com/nodegroup",
        "cloud.google.com/gke-nodepool",
        "kubernetes.azure.com/agentpool",
        "karpenter.sh/nodepool",
    ] {
        if let Some(pool) = label(key) {
            return pool.to_string();
        }
    }
    let roles: Vec<&str> = labels
        .map(|l| {
            l.keys()
                .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/"))
                .collect()
        })
        .unwrap_or_default();
    if roles
        .iter()
        .any(|r| *r == "control-plane" || *r == "master")
    {
        "control-plane".into()
    } else {
        roles
            .first()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "workers".into())
    }
}

/// CPU (cores) and memory (bytes) a pod requests.
fn pod_requests(pod: &Value) -> (f64, f64) {
    let sum = |pointer: &str| {
        array_at(pod, pointer).iter().fold((0.0, 0.0), |acc, c| {
            let cpu = str_at(c, "/resources/requests/cpu")
                .and_then(parse_quantity)
                .unwrap_or_default();
            let memory = str_at(c, "/resources/requests/memory")
                .and_then(parse_quantity)
                .unwrap_or_default();
            (acc.0 + cpu, acc.1 + memory)
        })
    };
    let (cpu, memory) = sum("/spec/containers");
    // Init containers run before the others: the pod needs the larger of both.
    let init = array_at(pod, "/spec/initContainers")
        .iter()
        .fold((0.0f64, 0.0f64), |acc, c| {
            let cpu = str_at(c, "/resources/requests/cpu")
                .and_then(parse_quantity)
                .unwrap_or_default();
            let memory = str_at(c, "/resources/requests/memory")
                .and_then(parse_quantity)
                .unwrap_or_default();
            (acc.0.max(cpu), acc.1.max(memory))
        });
    (cpu.max(init.0), memory.max(init.1))
}

async fn capacity(
    client: &kube::Client,
    nodes: Result<&Value, &kube::Error>,
    provider: ProviderKind,
) -> Check {
    let title = "Node capacity to drain";
    let nodes = match nodes {
        Ok(list) => items(list).to_vec(),
        Err(err) => return forbidden_or("capacity", title, err, "list nodes (cluster-wide)"),
    };
    match kube_api::get(
        client,
        "/api/v1/pods?fieldSelector=status.phase%21%3DSucceeded%2Cstatus.phase%21%3DFailed",
    )
    .await
    {
        Ok(list) => capacity_check(&nodes, items(&list), provider),
        Err(err) => forbidden_or("capacity", title, &err, "list pods cluster-wide"),
    }
}

/// Whether each pool can take the pods of its busiest node while that node drains.
pub fn capacity_check(nodes: &[Value], pods: &[Value], provider: ProviderKind) -> Check {
    let title = "Node capacity to drain";
    let mut requested: HashMap<&str, (f64, f64)> = HashMap::new();
    for pod in pods {
        if let Some(node) = str_at(pod, "/spec/nodeName") {
            let (cpu, memory) = pod_requests(pod);
            let entry = requested.entry(node).or_default();
            entry.0 += cpu;
            entry.1 += memory;
        }
    }
    // Pool → (name, allocatable, requested) of its schedulable nodes.
    type NodeUse = (String, (f64, f64), (f64, f64));
    let mut pools: BTreeMap<String, Vec<NodeUse>> = BTreeMap::new();
    for node in nodes {
        if node.pointer("/spec/unschedulable").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let name = str_at(node, "/metadata/name")
            .unwrap_or_default()
            .to_string();
        let allocatable = (
            str_at(node, "/status/allocatable/cpu")
                .and_then(parse_quantity)
                .unwrap_or_default(),
            str_at(node, "/status/allocatable/memory")
                .and_then(parse_quantity)
                .unwrap_or_default(),
        );
        let used = requested.get(name.as_str()).copied().unwrap_or_default();
        pools
            .entry(pool_of(node))
            .or_default()
            .push((name, allocatable, used));
    }
    let surge = provider.is_cloud();
    let mut details = Vec::new();
    let mut worst = CheckStatus::Pass;
    for (pool, nodes) in &pools {
        if nodes.len() < 2 {
            // One node: its pods have nowhere to go unless the provider adds a surge node.
            let status = if surge || pool == "control-plane" {
                CheckStatus::Pass
            } else {
                CheckStatus::Warn
            };
            worst = worst.min(status);
            if status == CheckStatus::Warn {
                details.push(
                    Detail::new(status, format!("{pool}: 1 node"))
                        .with_sub("its pods have nowhere to go while it drains"),
                );
            }
            continue;
        }
        let free: (f64, f64) = nodes.iter().fold((0.0, 0.0), |acc, (_, a, u)| {
            (acc.0 + (a.0 - u.0).max(0.0), acc.1 + (a.1 - u.1).max(0.0))
        });
        let (busiest, _, need) = nodes
            .iter()
            .max_by(|a, b| {
                a.2.0
                    .partial_cmp(&b.2.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("two nodes");
        let busiest_free = nodes
            .iter()
            .find(|(n, _, _)| n == busiest)
            .map(|(_, a, u)| ((a.0 - u.0).max(0.0), (a.1 - u.1).max(0.0)))
            .unwrap_or_default();
        let others = (free.0 - busiest_free.0, free.1 - busiest_free.1);
        let fits = need.0 <= others.0 && need.1 <= others.1;
        let status = if fits || surge {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        };
        worst = worst.min(status);
        let gib = |b: f64| b / 1024.0 / 1024.0 / 1024.0;
        details.push(
            Detail::new(status, format!("{pool}: {} nodes", nodes.len())).with_sub(format!(
                "draining {busiest} needs {:.1} CPU and {:.1} GiB; the others have {:.1} CPU and {:.1} GiB free",
                need.0,
                gib(need.1),
                others.0,
                gib(others.1)
            )),
        );
    }
    let summary = match worst {
        CheckStatus::Pass if surge => {
            "Each pool can drain a node (the provider adds a surge node).".to_string()
        }
        CheckStatus::Pass => format!(
            "{} can drain a node at a time.",
            plural(pools.len(), "pool", "pools")
        ),
        _ => {
            "Some pools can't take the pods of a draining node: they stay Pending until it returns."
                .to_string()
        }
    };
    Check::new("capacity", title, worst, summary).details(details)
}

// ----- Version skew -----

/// Kubelets vs the control plane now and after the update, and minor-by-minor updates.
pub fn skew(
    nodes: &[Value],
    current_kube: &str,
    target: Option<(u64, u64)>,
    provider: ProviderKind,
) -> Check {
    let title = "Version skew";
    let Some(control) = minor_of(current_kube) else {
        return Check::new(
            "version-skew",
            title,
            CheckStatus::Unknown,
            "The API server's version is unknown.",
        );
    };
    let mut details = Vec::new();
    let mut worst = CheckStatus::Pass;
    if let Some(target) = target
        && provider != ProviderKind::OpenShift
        && target.0 == control.0
        && target.1 > control.1.saturating_add(1)
    {
        worst = CheckStatus::Fail;
        details.push(Detail::new(
            CheckStatus::Fail,
            format!(
                "The control plane updates one minor at a time: {}.{} → {}.{} first",
                control.0,
                control.1,
                control.0,
                control.1.saturating_add(1)
            ),
        ));
    }
    let mut versions: BTreeMap<String, usize> = BTreeMap::new();
    for node in nodes {
        let kubelet = str_at(node, "/status/nodeInfo/kubeletVersion").unwrap_or_default();
        let name = str_at(node, "/metadata/name").unwrap_or_default();
        *versions.entry(kubelet.to_string()).or_default() += 1;
        let Some(minor) = minor_of(kubelet) else {
            continue;
        };
        if minor > control {
            worst = worst.min(CheckStatus::Fail);
            details.push(Detail::new(
                CheckStatus::Fail,
                format!("{name}: kubelet {kubelet} is newer than the control plane"),
            ));
        } else if let Some(target) = target
            && target.0 == minor.0
            && target.1 > minor.1.saturating_add(3)
        {
            worst = worst.min(CheckStatus::Fail);
            details.push(
                Detail::new(
                    CheckStatus::Fail,
                    format!(
                        "{name}: kubelet {kubelet} would be {} minors behind {}.{}",
                        target.1 - minor.1,
                        target.0,
                        target.1
                    ),
                )
                .with_sub("kubelets may be at most 3 minors older than the control plane: update this node first"),
            );
        }
    }
    let summary = if worst == CheckStatus::Pass {
        let list: Vec<String> = versions
            .iter()
            .map(|(v, n)| format!("{v} on {}", plural(*n, "node", "nodes")))
            .collect();
        match target {
            Some((a, b)) => format!(
                "Kubelets {} stay within 3 minors of {a}.{b}.",
                list.join(", ")
            ),
            None => format!("Kubelets {}.", list.join(", ")),
        }
    } else {
        "The versions don't fit the Kubernetes skew policy.".to_string()
    };
    Check::new("version-skew", title, worst, summary).details(details)
}

// ----- Operators (phase 12) -----

/// Installed operators against the target: `olm.maxOpenShiftVersion` blocks OpenShift minor
/// updates; a declared max Kubernetes version is advisory.
pub fn operators(
    installed: &Installed,
    target_kube: Option<(u64, u64)>,
    target_openshift: Option<(u64, u64)>,
) -> Check {
    let title = "Installed operators compatible";
    let operators = match installed {
        Installed::Ready(operators) => operators,
        Installed::Loading => {
            return Check::new(
                "operators",
                title,
                CheckStatus::Unknown,
                "Reading the installed operators…",
            );
        }
        Installed::NoOlm => {
            return Check::new(
                "operators",
                title,
                CheckStatus::Info,
                "No OLM: no operators to check.",
            );
        }
        Installed::NotConnected => {
            return Check::new(
                "operators",
                title,
                CheckStatus::Unknown,
                "The cluster isn't connected.",
            );
        }
        Installed::Problem(problem) => {
            return Check::new(
                "operators",
                title,
                CheckStatus::Unknown,
                format!("Not checked: {problem}"),
            );
        }
    };
    let mut details = Vec::new();
    for op in operators {
        if let (Some(max), Some(target)) = (&op.max_openshift_version, target_openshift)
            && let Some(max_minor) = minor_of(max)
            && target > max_minor
        {
            details.push(
                Detail::new(
                    CheckStatus::Fail,
                    format!(
                        "{} {} allows OpenShift up to {max}",
                        op.name,
                        op.version.as_deref().unwrap_or_default()
                    ),
                )
                .with_sub(
                    "OpenShift blocks the minor update until the operator is updated or removed",
                )
                .fix(Fix::Operators),
            );
        }
        if let (Some(max), Some(target)) = (&op.max_kube_version, target_kube)
            && let Some(max_minor) = minor_of(max)
            && target > max_minor
        {
            details.push(
                Detail::new(
                    CheckStatus::Warn,
                    format!(
                        "{} {} declares Kubernetes up to {max}",
                        op.name,
                        op.version.as_deref().unwrap_or_default()
                    ),
                )
                .with_sub("advisory: OLM doesn't enforce it")
                .fix(Fix::Operators),
            );
        }
    }
    if details.is_empty() {
        let target = match (target_openshift, target_kube) {
            (Some((a, b)), Some((c, d))) => format!("OpenShift {a}.{b} / Kubernetes {c}.{d}"),
            (None, Some((c, d))) => format!("Kubernetes {c}.{d}"),
            _ => "the target".into(),
        };
        return Check::new(
            "operators",
            title,
            CheckStatus::Pass,
            match operators.len() {
                0 => "No operators installed.".to_string(),
                n => format!("{} of {n} allow {target}.", n),
            },
        );
    }
    let status = details
        .iter()
        .map(|d| d.status)
        .min()
        .unwrap_or(CheckStatus::Warn);
    Check::new(
        "operators",
        "Installed operators may not support the target",
        status,
        format!(
            "{} of {} {} a lower version.",
            details.len(),
            plural(operators.len(), "operator", "operators"),
            if details.len() == 1 {
                "declares"
            } else {
                "declare"
            }
        ),
    )
    .details(details)
    .fix(Fix::Operators)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pdbs_that_allow_no_disruption_fail() {
        let check = pdb_check(&[
            json!({"metadata": {"namespace": "payments", "name": "ledger-writer-pdb"}, "spec": {"minAvailable": 3},
                   "status": {"disruptionsAllowed": 0, "expectedPods": 3, "currentHealthy": 3}}),
            json!({"metadata": {"namespace": "a", "name": "ok"}, "status": {"disruptionsAllowed": 1, "expectedPods": 3}}),
            // Protects nothing: no pods to evict.
            json!({"metadata": {"namespace": "a", "name": "empty"}, "status": {"disruptionsAllowed": 0, "expectedPods": 0}}),
        ]);
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.title, "PodDisruptionBudget blocks node drain");
        assert_eq!(check.details.len(), 1);
        assert_eq!(
            check.details[0].text,
            "payments/ledger-writer-pdb allows 0 disruptions"
        );
        assert!(
            check.details[0]
                .sub
                .as_deref()
                .unwrap()
                .contains("minAvailable 3")
        );
        assert!(matches!(check.fix, Some(Fix::Open { .. })));
        assert_eq!(pdb_check(&[]).status, CheckStatus::Pass);
    }

    #[test]
    fn api_request_counts() {
        let uses = parse_api_request_counts(&[
            json!({"metadata": {"name": "flowschemas.v1beta3.flowcontrol.apiserver.k8s.io"},
                   "status": {"removedInRelease": "1.32", "requestCount": 2,
                              "last24h": [{"byNode": [{"byUser": [{"username": "system:serviceaccount:tools:legacy", "userAgent": "legacy-tool/1.0 (linux)", "requestCount": 2}]}]}]}}),
            json!({"metadata": {"name": "pods.v1."}, "status": {"requestCount": 999}}),
        ]);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].resource, "flowschemas");
        assert_eq!(uses[0].group, "flowcontrol.apiserver.k8s.io");
        assert_eq!(uses[0].removed_in, Some((1, 32)));
        assert_eq!(
            uses[0].clients,
            ["system:serviceaccount:tools:legacy · legacy-tool/1.0 · 2"]
        );
        // Removed by the target: fails; removed later: a warning.
        assert_eq!(
            deprecated_check(uses.clone(), Some((1, 32)), "x").status,
            CheckStatus::Fail
        );
        assert_eq!(
            deprecated_check(uses, Some((1, 31)), "x").status,
            CheckStatus::Warn
        );
        assert_eq!(
            deprecated_check(Vec::new(), Some((1, 31)), "x").status,
            CheckStatus::Pass
        );
    }

    #[test]
    fn metrics_text() {
        let text = "# HELP apiserver_requested_deprecated_apis …\n\
            apiserver_requested_deprecated_apis{group=\"flowcontrol.apiserver.k8s.io\",removed_release=\"1.32\",resource=\"flowschemas\",subresource=\"\",version=\"v1beta3\"} 1\n\
            apiserver_request_total{code=\"200\"} 5\n";
        let uses = parse_metrics_text(text);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].version, "v1beta3");
        assert_eq!(uses[0].removed_in, Some((1, 32)));
        assert_eq!(uses[0].requests, None);
    }

    fn release() -> ReleaseRef {
        ReleaseRef {
            namespace: "kubyl-updates".into(),
            name: "legacy-app".into(),
            driver: Driver::Secret,
            object: "sh.helm.release.v1.legacy-app.v1".into(),
            revision: 1,
        }
    }

    #[test]
    fn helm_manifests_with_removed_apis() {
        let manifest = "---\n# Source: legacy-app/templates/ingress.yaml\napiVersion: extensions/v1beta1\nkind: Ingress\nmetadata:\n  name: web\n---\napiVersion: flowcontrol.apiserver.k8s.io/v1beta3\nkind: FlowSchema\nmetadata:\n  name: legacy\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n";
        let findings = helm_findings(&release(), &[manifest], Some((1, 32)));
        assert_eq!(findings.len(), 2);
        // On 1.31, the Ingress is already gone (warn) and the FlowSchema breaks with 1.32 (fail).
        let check = helm_check(3, findings.clone(), Vec::new(), Some((1, 31)));
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.details.len(), 1);
        assert!(
            check.details[0]
                .sub
                .as_deref()
                .unwrap()
                .contains("extensions/v1beta1 Ingress web")
        );
        assert!(matches!(
            check.details[0].fix,
            Some(Fix::OpenRelease { secret: true, .. })
        ));
        // Up to 1.31 only the Ingress counts.
        assert_eq!(
            helm_findings(&release(), &[manifest], Some((1, 31))).len(),
            1
        );
        assert_eq!(
            helm_check(3, Vec::new(), Vec::new(), Some((1, 31))).status,
            CheckStatus::Pass
        );
        let failed = helm_check(1, Vec::new(), vec![(release(), "Forbidden".into())], None);
        assert_eq!(failed.status, CheckStatus::Unknown);
    }

    #[test]
    fn last_applied_scan() {
        let resource = ApiResource {
            group: "networking.k8s.io".into(),
            version: "v1".into(),
            api_version: "networking.k8s.io/v1".into(),
            kind: "Ingress".into(),
            plural: "ingresses".into(),
        };
        let applied =
            r#"{"apiVersion":"extensions/v1beta1","kind":"Ingress","metadata":{"name":"web"}}"#;
        let finding =
            stored_finding(applied, Some("shop"), Some("web"), &resource, Some((1, 31))).unwrap();
        assert_eq!(finding.link.gvr.resource, "ingresses");
        assert_eq!(finding.removal.removed_label(), "1.22");
        let current = r#"{"apiVersion":"networking.k8s.io/v1","kind":"Ingress"}"#;
        assert!(
            stored_finding(current, Some("shop"), Some("web"), &resource, Some((1, 31))).is_none()
        );
        let check = stored_check(vec![finding], Vec::new(), Vec::new(), 10, Some((1, 30)));
        // Removed long before the current version: a warning, not this update's failure.
        assert_eq!(check.status, CheckStatus::Warn);
        assert_eq!(
            stored_check(Vec::new(), vec!["ingresses".into()], Vec::new(), 0, None).status,
            CheckStatus::Unknown
        );
        // A scan that stopped early (an error, the object limit) never passes.
        let partial = stored_check(
            Vec::new(),
            Vec::new(),
            vec!["ingresses (the first 20000 only)".into()],
            20_000,
            None,
        );
        assert_eq!(partial.status, CheckStatus::Unknown);
        assert_eq!(partial.details[0].text, "Scan incomplete");
        assert_eq!(
            stored_check(Vec::new(), Vec::new(), Vec::new(), 5, None).status,
            CheckStatus::Pass
        );
    }

    /// OpenShift serves `ingresses.config.openshift.io` (kind `Ingress`) next to the real
    /// Ingresses: the scan must pick `networking.k8s.io` whatever the server's order.
    #[test]
    fn scan_kinds_pick_the_kind_s_own_group() {
        use kubyl_core::{Gvk, Gvr};
        use kubyl_kube::discovery::{ApiResourceInfo, Discovery};
        let info = |group: &str, namespaced: bool| ApiResourceInfo {
            gvk: Gvk::new(group, "v1", "Ingress"),
            gvr: Gvr::new(group, "v1", "ingresses"),
            singular: "ingress".into(),
            namespaced,
            verbs: vec!["list".into(), "watch".into()],
            short_names: vec![],
            categories: vec![],
            subresources: vec![],
            preferred: true,
        };
        let discovery = Discovery {
            groups: vec![],
            resources: vec![
                info("config.openshift.io", false),
                info("networking.k8s.io", true),
            ],
            aggregated: true,
        };
        let kinds = scan_kinds(&discovery);
        let ingress = kinds
            .iter()
            .find(|k| k.resource.plural == "ingresses")
            .unwrap();
        assert_eq!(ingress.resource.group, "networking.k8s.io");
        assert!(ingress.namespaced);
    }

    fn node(name: &str, pool: &str, cpu: &str, memory: &str, kubelet: &str) -> Value {
        json!({"metadata": {"name": name, "labels": {"eks.amazonaws.com/nodegroup": pool}},
               "status": {"allocatable": {"cpu": cpu, "memory": memory}, "nodeInfo": {"kubeletVersion": kubelet}}})
    }

    fn pod(node: &str, cpu: &str, memory: &str) -> Value {
        json!({"spec": {"nodeName": node, "containers": [{"resources": {"requests": {"cpu": cpu, "memory": memory}}}]}})
    }

    #[test]
    fn headroom_per_pool() {
        let nodes = [
            node("a", "general", "4", "16Gi", "v1.30.6"),
            node("b", "general", "4", "16Gi", "v1.30.6"),
        ];
        let fits = capacity_check(
            &nodes,
            &[pod("a", "1", "2Gi"), pod("b", "1", "2Gi")],
            ProviderKind::SelfManaged,
        );
        assert_eq!(fits.status, CheckStatus::Pass);
        let full = capacity_check(
            &nodes,
            &[pod("a", "3500m", "2Gi"), pod("b", "3500m", "2Gi")],
            ProviderKind::SelfManaged,
        );
        assert_eq!(full.status, CheckStatus::Warn);
        // Managed clouds add a surge node.
        let cloud = capacity_check(
            &nodes,
            &[pod("a", "3500m", "2Gi"), pod("b", "3500m", "2Gi")],
            ProviderKind::Eks,
        );
        assert_eq!(cloud.status, CheckStatus::Pass);
    }

    #[test]
    fn skew_rules() {
        let nodes = [
            node("a", "p", "1", "1Gi", "v1.30.6"),
            node("b", "p", "1", "1Gi", "v1.28.3"),
        ];
        assert_eq!(
            skew(&nodes, "v1.30.6", Some((1, 31)), ProviderKind::Eks).status,
            CheckStatus::Pass
        );
        // 1.28 kubelets would be 4 minors behind 1.32; and 1.30 → 1.32 skips a minor.
        let check = skew(&nodes, "v1.30.6", Some((1, 32)), ProviderKind::Eks);
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.details.len(), 2);
        // OpenShift targets are its own versions; the minor rule is the CVO's.
        let check = skew(&nodes, "v1.30.6", Some((1, 31)), ProviderKind::OpenShift);
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn operator_versions() {
        use kubyl_operators::api::InstalledOperator;
        let op = InstalledOperator {
            name: "cert-utils-operator".into(),
            package: None,
            namespace: "ops".into(),
            csv: "cert-utils-operator.v1.3.0".into(),
            version: Some("1.3.0".into()),
            channel: None,
            min_kube_version: None,
            max_kube_version: Some("1.30".into()),
            max_openshift_version: Some("4.17".into()),
            status: kubyl_operators::olm::join::OperatorStatus::Succeeded,
        };
        let installed = Installed::Ready(vec![op]);
        let check = operators(&installed, Some((1, 31)), Some((4, 18)));
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.details.len(), 2);
        assert_eq!(
            operators(&installed, Some((1, 30)), Some((4, 17))).status,
            CheckStatus::Pass
        );
        assert_eq!(
            operators(&Installed::NoOlm, None, None).status,
            CheckStatus::Info
        );
        assert_eq!(
            target_kube_minor(ProviderKind::OpenShift, "4.18.2"),
            Some((1, 31))
        );
        assert_eq!(
            target_kube_minor(ProviderKind::K3s, "v1.33.6+k3s1"),
            Some((1, 33))
        );
    }
}
