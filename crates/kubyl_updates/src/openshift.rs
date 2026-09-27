//! OpenShift: `ClusterVersion` (`config.openshift.io/v1`) through the Kubernetes API, like
//! `oc adm upgrade`. No extra credentials.
//!
//! - Read: the channel and the channels offered, `availableUpdates`, `conditionalUpdates` with
//!   their risks, history, conditions; ClusterOperators; MachineConfigPools and the node an
//!   update works on.
//! - Start: a merge patch of `spec.desiredUpdate: {version, image, force: false}` (what
//!   `oc adm upgrade --to` sends; `--allow-not-recommended` picks from the conditional updates).
//! - Channel: a merge patch of `spec.channel` (`oc adm upgrade channel`).

use kubyl_core::Gvr;
use regex::Regex;
use serde_json::{Value, json};

use crate::check::{Check, CheckStatus, Detail, Fix};
use crate::kube_api::{self, array_at, conditions, int_at, items, str_at, timestamp_at};
use crate::model::{
    Component, Condition, Current, HistoryEntry, ObjectLink, Plan, Pool, PoolKind, PoolState,
    Progress, ProviderKind, Risk, Scope, Status, Target, TargetKind, Writes,
};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};
use crate::version::{Version, compare, minor_of};

const CLUSTER_VERSION: &str = "/apis/config.openshift.io/v1/clusterversions/version";
const CLUSTER_OPERATORS: &str = "/apis/config.openshift.io/v1/clusteroperators";
const MACHINE_CONFIG_POOLS: &str = "/apis/machineconfiguration.openshift.io/v1/machineconfigpools";
const ADMIN_GATES: &str = "/api/v1/namespaces/openshift-config-managed/configmaps/admin-gates";
const ADMIN_ACKS: &str = "/api/v1/namespaces/openshift-config/configmaps/admin-acks";
/// The node annotation the Machine Config Operator sets while it works on a node.
const MCO_STATE: &str = "machineconfiguration.openshift.io/state";

/// The OpenShift provider of one cluster.
pub struct OpenShift {
    client: kube::Client,
}

impl OpenShift {
    pub fn new(client: kube::Client) -> Self {
        Self { client }
    }
}

impl UpdateProvider for OpenShift {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenShift
    }

    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>> {
        let client = self.client.clone();
        Box::pin(async move { read(client).await })
    }

    fn preflight_extras(&self, status: &Status, target: &str) -> ProviderFuture<Vec<Check>> {
        let client = self.client.clone();
        let status = status.clone();
        let target = target.to_string();
        Box::pin(async move { extras(client, &status, &target).await })
    }

    fn plan(
        &self,
        status: &Status,
        scope: &Scope,
        target: &str,
        cluster: &str,
    ) -> Result<Plan, String> {
        plan(status, scope, target, cluster)
    }

    fn start(&self, plan: &Plan) -> ProviderFuture<Result<String, ProviderError>> {
        let client = self.client.clone();
        let request = plan.request.clone();
        Box::pin(async move {
            let patch = request
                .get("patch")
                .cloned()
                .ok_or_else(|| ProviderError::Other("Nothing to change.".into()))?;
            kube_api::merge_patch(&client, CLUSTER_VERSION, &patch)
                .await
                .map_err(|e| {
                    ProviderError::from_kube(&e, "patch", "clusterversions.config.openshift.io")
                })?;
            Ok(request
                .get("done")
                .and_then(Value::as_str)
                .unwrap_or("Done.")
                .to_string())
        })
    }
}

async fn read(client: kube::Client) -> Result<Status, ProviderError> {
    let cv = kube_api::get(&client, CLUSTER_VERSION)
        .await
        .map_err(|e| ProviderError::from_kube(&e, "get", "clusterversions.config.openshift.io"))?;
    let kubernetes = client
        .apiserver_version()
        .await
        .ok()
        .map(|info| info.git_version);
    let mut status = parse_cluster_version(&cv);
    status.current.kubernetes = kubernetes;
    let target = status
        .progress
        .as_ref()
        .map(|p| p.target.clone())
        .unwrap_or_else(|| status.current.version.clone());
    match kube_api::get(&client, CLUSTER_OPERATORS).await {
        Ok(list) => status.components = parse_operators(items(&list), &target),
        Err(err) if kube_api::is_forbidden(&err) => status.notes.push(crate::model::Note {
            warning: true,
            title: "Can't read ClusterOperators".into(),
            text: ProviderError::from_kube(&err, "list", "clusteroperators.config.openshift.io")
                .to_string(),
            command: None,
            url: None,
        }),
        Err(_) => {}
    }
    // Hosted control planes have no MachineConfigPools: nothing to show then.
    if let Ok(list) = kube_api::get(&client, MACHINE_CONFIG_POOLS).await {
        let pools = items(&list);
        let nodes = if pools.iter().any(pool_is_updating) {
            kube_api::get(&client, "/api/v1/nodes")
                .await
                .map(|l| items(&l).to_vec())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        status.pools = parse_pools(pools, &nodes);
    }
    Ok(status)
}

fn pool_is_updating(pool: &Value) -> bool {
    conditions(pool).iter().any(|c| c.is("Updating", true))
        || int_at(pool, "/status/updatedMachineCount") != int_at(pool, "/status/machineCount")
}

/// The whole update state of a `ClusterVersion` (without operators and pools).
pub fn parse_cluster_version(cv: &Value) -> Status {
    let history: Vec<HistoryEntry> = array_at(cv, "/status/history")
        .iter()
        .map(|h| HistoryEntry {
            version: str_at(h, "/version").unwrap_or_default().to_string(),
            state: str_at(h, "/state").unwrap_or_default().to_string(),
            started: timestamp_at(h, "/startedTime"),
            completed: timestamp_at(h, "/completionTime"),
            verified: h.get("verified").and_then(Value::as_bool) == Some(true),
            accepted_risks: str_at(h, "/acceptedRisks").map(str::to_string),
        })
        .collect();
    let conds = conditions(cv);
    let desired = str_at(cv, "/status/desired/version").unwrap_or_default();
    // The cluster runs the newest completed version; while an update runs, that's not the
    // desired one yet.
    let current_version = history
        .iter()
        .find(|h| h.completed())
        .map(|h| h.version.clone())
        .unwrap_or_else(|| desired.to_string());
    let progressing = conds.iter().any(|c| c.is("Progressing", true));
    let partial = history.first().is_some_and(|h| h.state == "Partial");
    let progress = (progressing && (partial || desired != current_version)).then(|| {
        let message = conds
            .iter()
            .find(|c| c.kind == "Progressing")
            .and_then(|c| c.message.clone())
            .unwrap_or_default();
        Progress {
            target: desired.to_string(),
            percent: percent_of(&message),
            message,
            started: history.first().and_then(|h| h.started),
            failing: conds
                .iter()
                .find(|c| c.is("Failing", true))
                .and_then(|c| c.message.clone().or_else(|| c.reason.clone())),
        }
    });
    let current = Current {
        version: current_version.clone(),
        kubernetes: None,
        platform: None,
        channel: str_at(cv, "/spec/channel").map(str::to_string),
        channels: array_at(cv, "/status/desired/channels")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        cluster_id: str_at(cv, "/spec/clusterID").map(str::to_string),
        support: None,
        conditions: conds,
    };
    let targets = targets(cv, &current, progress.as_ref());
    Status {
        provider: "OpenShift (ClusterVersion)".into(),
        current,
        targets,
        history,
        progress,
        writes: Writes {
            control_plane: true,
            channel: true,
            ..Writes::default()
        },
        ..Status::default()
    }
}

/// `…(61% complete)…` → 61.
fn percent_of(message: &str) -> Option<f32> {
    let re = Regex::new(r"(\d+(?:\.\d+)?)% complete").expect("valid");
    re.captures(message)?.get(1)?.as_str().parse().ok()
}

fn release_target(release: &Value, kind: TargetKind, current: &Version) -> Option<Target> {
    let version = str_at(release, "/version")?;
    let mut target = Target::new(version, kind);
    target.image = str_at(release, "/image").map(str::to_string);
    target.url = str_at(release, "/url").map(str::to_string);
    target.channels = array_at(release, "/channels")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    target.minor = Version::parse(version).is_some_and(|v| v.minor_key() > current.minor_key());
    Some(target)
}

/// Available and conditional updates, newest first; minor ones blocked while
/// `Upgradeable=False`, every one blocked while an update runs.
fn targets(cv: &Value, current: &Current, progress: Option<&Progress>) -> Vec<Target> {
    let Some(version) = Version::parse(&current.version) else {
        return Vec::new();
    };
    let mut out: Vec<Target> = array_at(cv, "/status/availableUpdates")
        .iter()
        .filter_map(|r| release_target(r, TargetKind::Available, &version))
        .collect();
    for conditional in array_at(cv, "/status/conditionalUpdates") {
        let Some(release) = conditional.get("release") else {
            continue;
        };
        let recommended = array_at(conditional, "/conditions")
            .iter()
            .find(|c| str_at(c, "/type") == Some("Recommended"))
            .and_then(|c| str_at(c, "/status"));
        let kind = if recommended == Some("True") {
            TargetKind::Available
        } else {
            TargetKind::Conditional
        };
        let Some(mut target) = release_target(release, kind, &version) else {
            continue;
        };
        if out.iter().any(|t| t.version == target.version) {
            continue;
        }
        target.risks = array_at(conditional, "/risks")
            .iter()
            .map(|r| Risk {
                name: str_at(r, "/name").unwrap_or_default().to_string(),
                message: str_at(r, "/message").unwrap_or_default().to_string(),
                url: str_at(r, "/url").map(str::to_string),
            })
            .collect();
        out.push(target);
    }
    out.sort_by(|a, b| compare(&b.version, &a.version));
    let upgradeable = current
        .condition("Upgradeable")
        .filter(|c| c.status == Some(false));
    for target in &mut out {
        if let Some(progress) = progress {
            target.blocked.push(format!(
                "An update to {} is running. Wait for it to finish.",
                progress.target
            ));
        }
        if target.minor
            && let Some(condition) = upgradeable
        {
            target.blocked.push(format!(
                "Upgradeable=False{}: {}",
                condition
                    .reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default(),
                condition
                    .message
                    .as_deref()
                    .unwrap_or("minor updates are blocked")
            ));
        }
        if !target.blocked.is_empty() {
            target.kind = TargetKind::Blocked;
        }
    }
    // The newest available update is the suggestion.
    if progress.is_none()
        && let Some(first) = out.iter_mut().find(|t| t.kind == TargetKind::Available)
    {
        first.kind = TargetKind::Recommended;
    }
    out
}

/// ClusterOperators: their version against `target`, conditions and the message that matters.
pub fn parse_operators(list: &[Value], target: &str) -> Vec<Component> {
    let mut out: Vec<Component> = list
        .iter()
        .map(|co| {
            let conds = conditions(co);
            let get = |kind: &str| conds.iter().find(|c| c.kind == kind);
            let version = array_at(co, "/status/versions")
                .iter()
                .find(|v| str_at(v, "/name") == Some("operator"))
                .and_then(|v| str_at(v, "/version"))
                .map(str::to_string);
            let degraded = get("Degraded").and_then(|c| c.status);
            let progressing = get("Progressing").and_then(|c| c.status);
            let message = [
                (degraded == Some(true)).then(|| get("Degraded")).flatten(),
                (progressing == Some(true))
                    .then(|| get("Progressing"))
                    .flatten(),
                (get("Available").and_then(|c| c.status) == Some(false))
                    .then(|| get("Available"))
                    .flatten(),
            ]
            .into_iter()
            .flatten()
            .find_map(|c: &Condition| c.message.clone());
            Component {
                name: str_at(co, "/metadata/name").unwrap_or_default().to_string(),
                updated: version.as_deref() == Some(target),
                version,
                available: get("Available").and_then(|c| c.status),
                progressing,
                degraded,
                message,
            }
        })
        .collect();
    // Working ones first, then by name.
    out.sort_by(|a, b| {
        let rank = |c: &Component| {
            (
                c.degraded != Some(true),
                c.progressing != Some(true),
                c.updated,
            )
        };
        rank(a).cmp(&rank(b)).then(a.name.cmp(&b.name))
    });
    out
}

/// MachineConfigPools, with the node an update is working on.
pub fn parse_pools(pools: &[Value], nodes: &[Value]) -> Vec<Pool> {
    let mut out: Vec<Pool> = pools
        .iter()
        .map(|mcp| {
            let name = str_at(mcp, "/metadata/name")
                .unwrap_or_default()
                .to_string();
            let conds = conditions(mcp);
            let count = |p: &str| int_at(mcp, p).and_then(|n| usize::try_from(n).ok());
            let mut pool = Pool::new(
                format!("mcp:{name}"),
                name.clone(),
                PoolKind::MachineConfigPool,
            );
            pool.nodes = count("/status/machineCount");
            pool.updated = count("/status/updatedMachineCount");
            pool.ready = count("/status/readyMachineCount");
            pool.degraded = count("/status/degradedMachineCount").unwrap_or_default();
            pool.version = str_at(mcp, "/status/configuration/name").map(str::to_string);
            pool.surge = Some(match mcp.pointer("/spec/maxUnavailable") {
                Some(Value::Number(n)) => format!("maxUnavailable {n}"),
                Some(Value::String(s)) => format!("maxUnavailable {s}"),
                _ => "maxUnavailable 1".into(),
            });
            let paused = mcp.pointer("/spec/paused").and_then(Value::as_bool) == Some(true);
            pool.state = if conds.iter().any(|c| c.is("Degraded", true)) || pool.degraded > 0 {
                PoolState::Degraded
            } else if conds.iter().any(|c| c.is("Updating", true)) {
                PoolState::Updating
            } else if paused {
                PoolState::Paused
            } else {
                PoolState::Idle
            };
            pool.message = conds
                .iter()
                .find(|c| c.is("Degraded", true) || c.is("NodeDegraded", true))
                .and_then(|c| c.message.clone())
                .or_else(|| paused.then(|| "paused: its nodes don't update".into()));
            let selector: Vec<(String, String)> = mcp
                .pointer("/spec/nodeSelector/matchLabels")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if !selector.is_empty() {
                pool.draining = nodes
                    .iter()
                    .filter(|node| {
                        selector.iter().all(|(k, v)| {
                            node.pointer("/metadata/labels")
                                .and_then(|l| l.get(k))
                                .and_then(Value::as_str)
                                == Some(v.as_str())
                        })
                    })
                    .find(|node| {
                        node.pointer("/metadata/annotations")
                            .and_then(|a| a.get(MCO_STATE))
                            .and_then(Value::as_str)
                            == Some("Working")
                    })
                    .map(|node| {
                        let name = str_at(node, "/metadata/name").unwrap_or_default();
                        let cordoned = node.pointer("/spec/unschedulable").and_then(Value::as_bool)
                            == Some(true);
                        if cordoned {
                            format!("draining {name}")
                        } else {
                            format!("updating {name}")
                        }
                    });
            }
            pool.object = Some(ObjectLink::new(
                Gvr::new(
                    "machineconfiguration.openshift.io",
                    "v1",
                    "machineconfigpools",
                ),
                None,
                name,
            ));
            pool
        })
        .collect();
    // master first, then by name.
    out.sort_by(|a, b| {
        (a.name != "master")
            .cmp(&(b.name != "master"))
            .then(a.name.cmp(&b.name))
    });
    out
}

/// A short release image for summaries: `quay.io/…@sha256:1a2b3c4d…`.
fn short_image(image: &str) -> String {
    match image.split_once("@sha256:") {
        Some((repo, digest)) => format!("{repo}@sha256:{}…", &digest[..digest.len().min(12)]),
        None => image.to_string(),
    }
}

/// What a write does (the summary before confirming).
pub fn plan(status: &Status, scope: &Scope, target: &str, cluster: &str) -> Result<Plan, String> {
    match scope {
        Scope::ControlPlane => {
            let found = status
                .target(target)
                .ok_or_else(|| format!("{target} isn't offered to this cluster."))?;
            if !found.startable() {
                return Err(found.blocked.join(" "));
            }
            let conditional = found.kind == TargetKind::Conditional;
            let mut update = json!({"version": found.version, "force": false});
            if let Some(image) = &found.image {
                update["image"] = json!(image);
            }
            let oc = format!(
                "oc adm upgrade --to {}{}",
                found.version,
                if conditional {
                    " --allow-not-recommended"
                } else {
                    ""
                }
            );
            let mut changes = vec![format!(
                "ClusterVersion version: spec.desiredUpdate.version = {} (like {oc})",
                found.version
            )];
            if let Some(image) = &found.image {
                changes.push(format!("spec.desiredUpdate.image = {}", short_image(image)));
            }
            let minor = found.minor;
            let kind_label = format!(
                "{}, {}",
                if minor { "minor update" } else { "z-stream" },
                found.kind.label()
            );
            let mut notes = vec![
                "The cluster updates its control plane and operators first, then the nodes pool by pool: nodes are drained and rebooted one at a time.".to_string(),
            ];
            if minor {
                notes.push(
                    "A minor update changes the Kubernetes version: APIs removed in it stop working."
                        .into(),
                );
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update {cluster} to {}?", found.version),
                from: status.current.version.clone(),
                to: found.version.clone(),
                kind_label,
                changes,
                risks: if conditional {
                    found.risks.clone()
                } else {
                    Vec::new()
                },
                notes,
                irreversible: true,
                request: json!({
                    "patch": {"spec": {"desiredUpdate": update}},
                    "done": format!("Started the update of {cluster} to {}.", found.version),
                }),
            })
        }
        Scope::Channel(channel) => {
            let from = status.current.channel.clone().unwrap_or_default();
            if &from == channel {
                return Err(format!("{cluster} already follows {channel}."));
            }
            let mut notes = vec![
                "The cluster asks the update service for the new channel's updates. Nothing is updated until you start an update.".to_string(),
            ];
            if !status.current.channels.is_empty() && !status.current.channels.contains(channel) {
                notes.push(format!(
                    "{channel} isn't one of the channels this release lists; the cluster may find no updates in it."
                ));
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Change the channel of {cluster} to {channel}?"),
                from: if from.is_empty() {
                    "none".into()
                } else {
                    from.clone()
                },
                to: channel.clone(),
                kind_label: "channel change".into(),
                changes: vec![format!(
                    "ClusterVersion version: spec.channel = {channel} (like oc adm upgrade channel {channel})"
                )],
                risks: Vec::new(),
                notes,
                irreversible: false,
                request: json!({
                    "patch": {"spec": {"channel": channel}},
                    "done": format!("{cluster} now follows {channel}."),
                }),
            })
        }
        _ => Err("OpenShift updates the whole cluster; its pools follow.".into()),
    }
}

/// Provider checks: operators healthy, pools not paused or degraded, `Upgradeable` and the
/// admin acks a minor update needs.
async fn extras(client: kube::Client, status: &Status, target: &str) -> Vec<Check> {
    let mut checks = Vec::new();
    checks.push(operators_check(&status.components));
    if let Some(check) = pools_check(&status.pools) {
        checks.push(check);
    }
    let minor = status.target(target).is_some_and(|t| t.minor);
    if minor {
        let gates = kube_api::get(&client, ADMIN_GATES).await;
        let acks = kube_api::get(&client, ADMIN_ACKS).await;
        checks.push(upgradeable_check(
            status,
            target,
            gates.as_ref().ok(),
            acks.as_ref().ok(),
            gates
                .as_ref()
                .err()
                .or(acks.as_ref().err())
                .filter(|e| kube_api::is_forbidden(e))
                .is_some(),
        ));
    }
    checks
}

fn operators_check(components: &[Component]) -> Check {
    let title = "Cluster operators healthy";
    if components.is_empty() {
        return Check::new(
            "cluster-operators",
            title,
            CheckStatus::Unknown,
            "Couldn't read ClusterOperators (list clusteroperators.config.openshift.io).",
        );
    }
    let bad: Vec<&Component> = components
        .iter()
        .filter(|c| c.degraded == Some(true) || c.available == Some(false))
        .collect();
    if bad.is_empty() {
        return Check::new(
            "cluster-operators",
            title,
            CheckStatus::Pass,
            format!(
                "{} of {} Available, none Degraded.",
                components.len(),
                components.len()
            ),
        );
    }
    Check::new(
        "cluster-operators",
        title,
        CheckStatus::Fail,
        format!(
            "{} of {} cluster operators are degraded or unavailable; an update may stall on them.",
            bad.len(),
            components.len()
        ),
    )
    .details(
        bad.iter()
            .map(|c| {
                let state = if c.available == Some(false) {
                    "unavailable"
                } else {
                    "degraded"
                };
                let mut detail = Detail::new(CheckStatus::Fail, format!("{} · {state}", c.name));
                if let Some(message) = &c.message {
                    detail = detail.with_sub(message.clone());
                }
                detail.fix(Fix::Open {
                    label: "Open".into(),
                    link: ObjectLink::new(
                        Gvr::new("config.openshift.io", "v1", "clusteroperators"),
                        None,
                        c.name.clone(),
                    ),
                })
            })
            .collect(),
    )
}

fn pools_check(pools: &[Pool]) -> Option<Check> {
    if pools.is_empty() {
        return None;
    }
    let bad: Vec<&Pool> = pools
        .iter()
        .filter(|p| matches!(p.state, PoolState::Degraded | PoolState::Paused))
        .collect();
    let title = "Machine config pools ready";
    if bad.is_empty() {
        return Some(Check::new(
            "machine-config-pools",
            title,
            CheckStatus::Pass,
            format!("{} pools, none paused or degraded.", pools.len()),
        ));
    }
    let degraded = bad.iter().any(|p| p.state == PoolState::Degraded);
    Some(
        Check::new(
            "machine-config-pools",
            title,
            if degraded {
                CheckStatus::Fail
            } else {
                CheckStatus::Warn
            },
            if degraded {
                "A degraded pool stops its nodes from updating."
            } else {
                "Paused pools keep their nodes on the old version until you unpause them."
            },
        )
        .details(
            bad.iter()
                .map(|p| {
                    let mut detail = Detail::new(
                        if p.state == PoolState::Degraded {
                            CheckStatus::Fail
                        } else {
                            CheckStatus::Warn
                        },
                        format!(
                            "{} · {}",
                            p.name,
                            if p.state == PoolState::Degraded {
                                "degraded"
                            } else {
                                "paused"
                            }
                        ),
                    );
                    if let Some(message) = &p.message {
                        detail = detail.with_sub(message.clone());
                    }
                    match &p.object {
                        Some(link) => detail.fix(Fix::Open {
                            label: "Open".into(),
                            link: link.clone(),
                        }),
                        None => detail,
                    }
                })
                .collect(),
        ),
    )
}

/// `Upgradeable` and the admin acks of the current minor (gates keyed `ack-<minor>-…`).
fn upgradeable_check(
    status: &Status,
    target: &str,
    gates: Option<&Value>,
    acks: Option<&Value>,
    forbidden: bool,
) -> Check {
    let title = "Minor update allowed";
    let current_minor = minor_of(&status.current.version)
        .map(|(a, b)| format!("{a}.{b}"))
        .unwrap_or_default();
    let prefix = format!("ack-{current_minor}-");
    let acked = |key: &str| {
        acks.and_then(|a| a.pointer("/data"))
            .and_then(|d| d.get(key))
            .and_then(Value::as_str)
            == Some("true")
    };
    let missing: Vec<(String, String)> = gates
        .and_then(|g| g.pointer("/data"))
        .and_then(Value::as_object)
        .map(|data| {
            data.iter()
                .filter(|(k, _)| k.starts_with(&prefix) && !acked(k))
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let details: Vec<Detail> = missing
        .iter()
        .map(|(key, text)| {
            Detail::new(CheckStatus::Fail, format!("Admin ack {key}"))
                .with_sub(text.clone())
                .fix(Fix::Copy {
                    label: "Copy oc command".into(),
                    text: format!(
                        "oc -n openshift-config patch cm admin-acks --type=merge --patch '{{\"data\":{{\"{key}\":\"true\"}}}}'"
                    ),
                })
        })
        .collect();
    match status.current.condition("Upgradeable") {
        Some(c) if c.status == Some(false) => Check::new(
            "upgradeable",
            title,
            CheckStatus::Fail,
            format!(
                "Upgradeable=False{}: {}",
                c.reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default(),
                c.message
                    .as_deref()
                    .unwrap_or("the cluster blocks minor updates")
            ),
        )
        .details(details),
        _ if !missing.is_empty() => Check::new(
            "upgradeable",
            title,
            CheckStatus::Fail,
            format!("{target} needs admin acknowledgements first."),
        )
        .details(details),
        _ if forbidden => Check::new(
            "upgradeable",
            title,
            CheckStatus::Unknown,
            "Can't read the admin gates: missing get configmaps in openshift-config-managed and openshift-config.",
        ),
        _ => Check::new(
            "upgradeable",
            title,
            CheckStatus::Pass,
            "Upgradeable, no admin acknowledgement missing.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster_version() -> Value {
        json!({
            "spec": {"channel": "stable-4.17", "clusterID": "0e6a7c1e-0000-4000-8000-000000000001"},
            "status": {
                "desired": {"version": "4.17.8", "channels": ["candidate-4.17", "fast-4.17", "stable-4.17", "stable-4.18"]},
                "history": [
                    {"state": "Completed", "version": "4.17.8", "startedTime": "2026-08-30T10:00:00Z", "completionTime": "2026-08-30T11:12:00Z", "verified": true},
                    {"state": "Completed", "version": "4.17.6", "startedTime": "2026-07-01T10:00:00Z", "completionTime": "2026-07-01T11:00:00Z", "verified": true}
                ],
                "availableUpdates": [
                    {"version": "4.17.9", "image": "quay.io/openshift-release-dev/ocp-release@sha256:9999999999999999999999999999999999999999999999999999999999999999", "channels": ["stable-4.17"]},
                    {"version": "4.17.12", "image": "quay.io/openshift-release-dev/ocp-release@sha256:1212121212121212121212121212121212121212121212121212121212121212", "url": "https://access.redhat.com/errata/RHBA-2026:0012"}
                ],
                "conditionalUpdates": [
                    {"release": {"version": "4.17.13", "image": "quay.io/x@sha256:13"},
                     "risks": [{"name": "ExampleStorageDriverRegression", "message": "Volumes may fail to attach.", "url": "https://issues.example.com/OCPBUGS-12345", "matchingRules": [{"type": "Always"}]}],
                     "conditions": [{"type": "Recommended", "status": "False", "reason": "ExampleStorageDriverRegression"}]},
                    {"release": {"version": "4.18.2", "image": "quay.io/x@sha256:182"},
                     "risks": [{"name": "ExampleNetworkPolicyChange", "message": "Policies change.", "matchingRules": [{"type": "PromQL"}]}],
                     "conditions": [{"type": "Recommended", "status": "False"}]}
                ],
                "conditions": [
                    {"type": "Available", "status": "True"},
                    {"type": "Progressing", "status": "False", "message": "Cluster version is 4.17.8"},
                    {"type": "Upgradeable", "status": "False", "reason": "AdminAckRequired", "message": "Kubernetes 1.31 removes APIs."}
                ]
            }
        })
    }

    #[test]
    fn reads_channel_targets_and_blocks_minor_updates() {
        let status = parse_cluster_version(&cluster_version());
        assert_eq!(status.current.version, "4.17.8");
        assert_eq!(status.current.channel.as_deref(), Some("stable-4.17"));
        assert_eq!(status.current.channels.len(), 4);
        let versions: Vec<(&str, TargetKind)> = status
            .targets
            .iter()
            .map(|t| (t.version.as_str(), t.kind))
            .collect();
        assert_eq!(
            versions,
            [
                ("4.18.2", TargetKind::Blocked),
                ("4.17.13", TargetKind::Conditional),
                ("4.17.12", TargetKind::Recommended),
                ("4.17.9", TargetKind::Available),
            ]
        );
        let minor = status.target("4.18.2").unwrap();
        assert!(minor.minor);
        assert!(
            minor.blocked[0].contains("AdminAckRequired"),
            "{:?}",
            minor.blocked
        );
        assert_eq!(
            status.target("4.17.13").unwrap().risks[0].name,
            "ExampleStorageDriverRegression"
        );
        assert!(status.progress.is_none());
        assert_eq!(status.suggested().unwrap().version, "4.17.12");
    }

    #[test]
    fn progress_while_updating() {
        let mut cv = cluster_version();
        cv["status"]["desired"]["version"] = json!("4.17.12");
        cv["status"]["history"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"state": "Partial", "version": "4.17.12", "startedTime": "2026-09-26T08:00:00Z"}));
        cv["status"]["conditions"][1] = json!({"type": "Progressing", "status": "True",
            "message": "Working towards 4.17.12: 512 of 845 done (61% complete), waiting on machine-config"});
        let status = parse_cluster_version(&cv);
        // The cluster still runs the last completed version.
        assert_eq!(status.current.version, "4.17.8");
        let progress = status.progress.as_ref().unwrap();
        assert_eq!(progress.target, "4.17.12");
        assert_eq!(progress.percent, Some(61.0));
        assert!(status.targets.iter().all(|t| t.kind == TargetKind::Blocked));
    }

    #[test]
    fn operators_and_pools() {
        let operators = parse_operators(
            &[
                json!({"metadata": {"name": "dns"}, "status": {"versions": [{"name": "operator", "version": "4.17.12"}],
                    "conditions": [{"type": "Available", "status": "True"}, {"type": "Progressing", "status": "False"}, {"type": "Degraded", "status": "False"}]}}),
                json!({"metadata": {"name": "machine-config"}, "status": {"versions": [{"name": "operator", "version": "4.17.8"}],
                    "conditions": [{"type": "Available", "status": "True"}, {"type": "Progressing", "status": "True", "message": "Working towards 4.17.12"}]}}),
            ],
            "4.17.12",
        );
        assert_eq!(operators[0].name, "machine-config");
        assert!(!operators[0].updated);
        assert_eq!(
            operators[0].message.as_deref(),
            Some("Working towards 4.17.12")
        );
        assert!(operators[1].updated);

        let pools = parse_pools(
            &[
                json!({"metadata": {"name": "worker"}, "spec": {"nodeSelector": {"matchLabels": {"node-role.kubernetes.io/worker": ""}}},
                    "status": {"machineCount": 3, "updatedMachineCount": 1, "readyMachineCount": 2, "degradedMachineCount": 0,
                        "conditions": [{"type": "Updating", "status": "True"}]}}),
                json!({"metadata": {"name": "master"}, "status": {"machineCount": 3, "updatedMachineCount": 3, "readyMachineCount": 3,
                    "conditions": [{"type": "Updated", "status": "True"}]}}),
            ],
            &[
                json!({"metadata": {"name": "worker-1.example.internal", "labels": {"node-role.kubernetes.io/worker": ""},
                "annotations": {MCO_STATE: "Working"}}, "spec": {"unschedulable": true}}),
            ],
        );
        assert_eq!(pools[0].name, "master");
        assert_eq!(pools[1].state, PoolState::Updating);
        assert_eq!((pools[1].updated, pools[1].nodes), (Some(1), Some(3)));
        assert_eq!(
            pools[1].draining.as_deref(),
            Some("draining worker-1.example.internal")
        );
    }

    #[test]
    fn plans_patch_like_oc() {
        let status = parse_cluster_version(&cluster_version());
        let update = plan(&status, &Scope::ControlPlane, "4.17.12", "ocp").unwrap();
        assert_eq!(
            update.request["patch"]["spec"]["desiredUpdate"]["version"],
            "4.17.12"
        );
        assert_eq!(
            update.request["patch"]["spec"]["desiredUpdate"]["force"],
            false
        );
        assert!(
            update.request["patch"]["spec"]["desiredUpdate"]["image"]
                .as_str()
                .unwrap()
                .contains("@sha256:1212")
        );
        assert!(update.changes[0].contains("oc adm upgrade --to 4.17.12"));
        assert!(update.risks.is_empty());
        assert!(update.irreversible);

        let conditional = plan(&status, &Scope::ControlPlane, "4.17.13", "ocp").unwrap();
        assert_eq!(conditional.risks.len(), 1);
        assert!(conditional.changes[0].contains("--allow-not-recommended"));

        assert!(
            plan(&status, &Scope::ControlPlane, "4.18.2", "ocp")
                .unwrap_err()
                .contains("AdminAckRequired")
        );
        assert!(plan(&status, &Scope::ControlPlane, "9.9.9", "ocp").is_err());

        let channel = plan(&status, &Scope::Channel("stable-4.18".into()), "", "ocp").unwrap();
        assert_eq!(
            channel.request["patch"],
            json!({"spec": {"channel": "stable-4.18"}})
        );
        assert!(!channel.irreversible);
        assert!(plan(&status, &Scope::Channel("stable-4.17".into()), "", "ocp").is_err());
    }

    #[test]
    fn admin_acks() {
        let status = parse_cluster_version(&cluster_version());
        let gates = json!({"data": {"ack-4.17-kube-1.31-api-removals-in-4.18": "Kubernetes 1.31 removes APIs.", "ack-4.16-old": "old"}});
        let check = upgradeable_check(
            &status,
            "4.18.2",
            Some(&gates),
            Some(&json!({"data": {}})),
            false,
        );
        assert_eq!(check.status, CheckStatus::Fail);
        assert_eq!(check.details.len(), 1);
        match &check.details[0].fix {
            Some(Fix::Copy { text, .. }) => {
                assert!(text.contains("ack-4.17-kube-1.31-api-removals-in-4.18"))
            }
            other => panic!("{other:?}"),
        }
    }
}
