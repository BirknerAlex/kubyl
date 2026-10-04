//! k3s and RKE2 through system-upgrade-controller: its `Plan`s (`upgrade.cattle.io/v1`) say
//! which version the matching nodes run. Kubyl reads the Plans and their progress (the nodes
//! whose `plan.upgrade.cattle.io/<plan>` label carries the Plan's `latestHash` are done) and
//! updates by setting `spec.version` (a merge patch); new Plans go through the YAML editor.
//!
//! Versions come from the channel server the Plans use (`update.k3s.io`, `update.rke2.io`):
//! a public, anonymous GET, like the controller itself makes.

use std::time::Duration;

use kubyl_core::Gvr;
use serde_json::{Value, json};

use crate::kube_api::{self, array_at, conditions, items, str_at};
use crate::model::{
    Current, ObjectLink, Plan, Pool, PoolKind, PoolState, Progress, ProviderKind, Scope, Status,
    Target, TargetKind, Writes,
};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};
use crate::version::{Version, compare};

const PLANS: &str = "/apis/upgrade.cattle.io/v1/plans";

/// The provider of a k3s or RKE2 cluster with system-upgrade-controller.
pub struct Suc {
    client: kube::Client,
    kind: ProviderKind,
}

impl Suc {
    pub fn new(client: kube::Client, kind: ProviderKind) -> Self {
        Self { client, kind }
    }
}

/// The channel server of a distribution.
pub fn channel_server(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Rke2 => "https://update.rke2.io/v1-release/channels",
        _ => "https://update.k3s.io/v1-release/channels",
    }
}

impl UpdateProvider for Suc {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>> {
        let client = self.client.clone();
        let kind = self.kind;
        Box::pin(async move {
            let version = client
                .apiserver_version()
                .await
                .map_err(|e| ProviderError::from_kube(&e, "get", "/version"))?;
            let plans = kube_api::get(&client, PLANS)
                .await
                .map_err(|e| ProviderError::from_kube(&e, "list", "plans.upgrade.cattle.io"))?;
            let nodes = kube_api::get(&client, "/api/v1/nodes")
                .await
                .map_err(|e| ProviderError::from_kube(&e, "list", "nodes"))?;
            let channels = fetch_channels(kind).await;
            Ok(status(
                kind,
                &version.git_version,
                items(&plans),
                items(&nodes),
                channels,
            ))
        })
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
            let version = request["version"].as_str().unwrap_or_default().to_string();
            let mut done = Vec::new();
            for target in array_at(&request, "/plans") {
                let (Some(ns), Some(name)) =
                    (str_at(target, "/namespace"), str_at(target, "/name"))
                else {
                    continue;
                };
                let path = format!("/apis/upgrade.cattle.io/v1/namespaces/{ns}/plans/{name}");
                // A version replaces a channel: the controller resolves one or the other.
                let patch = json!({"spec": {"version": version, "channel": null}});
                kube_api::merge_patch(&client, &path, &patch)
                    .await
                    .map_err(|e| {
                        let err = ProviderError::from_kube(&e, "patch", "plans.upgrade.cattle.io");
                        if done.is_empty() {
                            err
                        } else {
                            ProviderError::Other(format!(
                                "{err} (already updated: {})",
                                done.join(", ")
                            ))
                        }
                    })?;
                done.push(format!("{ns}/{name}"));
            }
            Ok(format!("Plans {} now target {version}.", done.join(", ")))
        })
    }
}

/// `(channel, latest version)` from the channel server; empty when it can't be reached.
async fn fetch_channels(kind: ProviderKind) -> Vec<(String, String)> {
    let Ok(http) = openidconnect::reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(10))
        .redirect(openidconnect::reqwest::redirect::Policy::limited(3))
        .build()
    else {
        return Vec::new();
    };
    let Ok(response) = http.get(channel_server(kind)).send().await else {
        return Vec::new();
    };
    let Ok(bytes) = response.bytes().await else {
        return Vec::new();
    };
    serde_json::from_slice::<Value>(&bytes)
        .map(|body| parse_channels(&body))
        .unwrap_or_default()
}

/// The `data[].{id, latest}` of a channel server answer.
pub fn parse_channels(body: &Value) -> Vec<(String, String)> {
    array_at(body, "/data")
        .iter()
        .filter_map(|c| {
            Some((
                str_at(c, "/id")?.to_string(),
                str_at(c, "/latest")?.to_string(),
            ))
        })
        .collect()
}

/// Whether a node's labels match a Plan's `nodeSelector`.
pub fn selects(selector: Option<&Value>, node: &Value) -> bool {
    let Some(selector) = selector else {
        return true;
    };
    let labels = node.pointer("/metadata/labels").and_then(Value::as_object);
    let label = |k: &str| labels.and_then(|l| l.get(k)).and_then(Value::as_str);
    let labels_ok = selector
        .get("matchLabels")
        .and_then(Value::as_object)
        .is_none_or(|m| m.iter().all(|(k, v)| label(k) == v.as_str()));
    let expressions_ok = array_at(selector, "/matchExpressions").iter().all(|e| {
        let key = str_at(e, "/key").unwrap_or_default();
        let values: Vec<&str> = array_at(e, "/values")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        match str_at(e, "/operator") {
            Some("In") => label(key).is_some_and(|v| values.contains(&v)),
            Some("NotIn") => label(key).is_none_or(|v| !values.contains(&v)),
            Some("Exists") => label(key).is_some(),
            Some("DoesNotExist") => label(key).is_none(),
            _ => false,
        }
    });
    labels_ok && expressions_ok
}

/// The status from the Plans, the nodes and the channels.
pub fn status(
    kind: ProviderKind,
    git_version: &str,
    plans: &[Value],
    nodes: &[Value],
    channels: Vec<(String, String)>,
) -> Status {
    let current = Version::parse(git_version);
    let mut pools = Vec::new();
    let mut applying: Vec<String> = Vec::new();
    let mut plan_target: Option<String> = None;
    for plan in plans {
        let ns = str_at(plan, "/metadata/namespace").unwrap_or_default();
        let name = str_at(plan, "/metadata/name").unwrap_or_default();
        let selected: Vec<&Value> = nodes
            .iter()
            .filter(|n| selects(plan.pointer("/spec/nodeSelector"), n))
            .collect();
        let hash = str_at(plan, "/status/latestHash");
        let label = format!("plan.upgrade.cattle.io/{name}");
        let updated = selected
            .iter()
            .filter(|n| {
                hash.is_some()
                    && n.pointer("/metadata/labels")
                        .and_then(|l| l.get(&label))
                        .and_then(Value::as_str)
                        == hash
            })
            .count();
        let now: Vec<String> = array_at(plan, "/status/applying")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        let version = str_at(plan, "/spec/version")
            .map(str::to_string)
            .or_else(|| str_at(plan, "/status/latestVersion").map(str::to_string))
            .or_else(|| {
                str_at(plan, "/spec/channel")
                    .map(|c| format!("channel {}", c.rsplit('/').next().unwrap_or(c)))
            });
        let mut pool = Pool::new(
            format!("plan:{ns}/{name}"),
            name.to_string(),
            PoolKind::Plan,
        );
        pool.version = version;
        pool.nodes = Some(selected.len());
        pool.updated = Some(updated);
        pool.surge = Some(format!(
            "concurrency {}{}",
            plan.pointer("/spec/concurrency")
                .and_then(Value::as_i64)
                .unwrap_or(1),
            if plan.pointer("/spec/drain").is_some() {
                " · drain"
            } else if plan.pointer("/spec/cordon").and_then(Value::as_bool) == Some(true) {
                " · cordon"
            } else {
                ""
            }
        ));
        pool.state = if !now.is_empty() {
            PoolState::Updating
        } else if updated < selected.len() {
            PoolState::Queued
        } else {
            PoolState::Idle
        };
        pool.draining = now.first().map(|n| format!("applying on {n}"));
        pool.message = conditions(plan)
            .into_iter()
            .find(|c| c.status == Some(false))
            .and_then(|c| c.message.or(c.reason));
        pool.updatable = true;
        pool.object = Some(ObjectLink::new(
            Gvr::new("upgrade.cattle.io", "v1", "plans"),
            Some(ns),
            name,
        ));
        if !now.is_empty() {
            applying.extend(now);
            plan_target = str_at(plan, "/status/latestVersion")
                .map(str::to_string)
                .or(plan_target);
        }
        pools.push(pool);
    }
    let progress = (!applying.is_empty()).then(|| {
        let total: usize = pools.iter().filter_map(|p| p.nodes).sum();
        let done: usize = pools.iter().filter_map(|p| p.updated).sum();
        Progress {
            target: plan_target.clone().unwrap_or_default(),
            percent: (total > 0).then(|| done as f32 * 100.0 / total as f32),
            message: format!(
                "{done} of {total} nodes updated · applying on {}",
                applying.join(", ")
            ),
            started: None,
            failing: None,
        }
    });
    let mut targets: Vec<Target> = Vec::new();
    if let Some(current) = &current {
        for (channel, latest) in &channels {
            let Some(version) = Version::parse(latest) else {
                continue;
            };
            if version <= *current {
                continue;
            }
            if let Some(existing) = targets.iter_mut().find(|t| &t.version == latest) {
                existing.channels.push(channel.clone());
                continue;
            }
            let mut target = Target::new(latest.clone(), TargetKind::Available);
            target.channels = vec![channel.clone()];
            target.minor = version.minor_key() > current.minor_key();
            if current.minors_to(&version) > 1 {
                target.kind = TargetKind::Blocked;
                target.blocked.push(format!(
                    "Kubernetes updates one minor at a time: {} first.",
                    crate::version::next_minor(git_version).unwrap_or_default()
                ));
            }
            if progress.is_some() {
                target.kind = TargetKind::Blocked;
                target
                    .blocked
                    .push("Plans are applying an update. Wait for it to finish.".into());
            }
            targets.push(target);
        }
    }
    targets.sort_by(|a, b| compare(&b.version, &a.version));
    // The stable channel's version, else the newest startable one.
    let stable = targets
        .iter()
        .position(|t| t.kind == TargetKind::Available && t.channels.iter().any(|c| c == "stable"))
        .or_else(|| targets.iter().position(|t| t.kind == TargetKind::Available));
    if let Some(ix) = stable {
        targets[ix].kind = TargetKind::Recommended;
    }
    let label = kind.label();
    Status {
        provider: format!("{label} (system-upgrade-controller Plans)"),
        current: Current {
            version: git_version.to_string(),
            channel: channels
                .iter()
                .any(|(c, _)| c == "stable")
                .then(|| "stable".into()),
            channels: channels.iter().map(|(c, _)| c.clone()).collect(),
            ..Current::default()
        },
        targets,
        progress,
        pools,
        writes: Writes {
            pools: !plans.is_empty(),
            ..Writes::default()
        },
        docs: vec![(
            format!("{label} automated upgrades"),
            if kind == ProviderKind::Rke2 {
                "https://docs.rke2.io/upgrades/automated_upgrade".into()
            } else {
                "https://docs.k3s.io/upgrades/automated".into()
            },
        )],
        notes: if plans.is_empty() {
            vec![crate::model::Note {
                warning: false,
                title: "No upgrade Plans yet".into(),
                text: "Create a server and an agent Plan to update this cluster (New plans…)."
                    .into(),
                command: None,
                url: None,
            }]
        } else {
            Vec::new()
        },
        ..Status::default()
    }
}

/// Points the Plans (all, or one) at `target`.
pub fn plan(status: &Status, scope: &Scope, target: &str, cluster: &str) -> Result<Plan, String> {
    if Version::parse(target).is_none() {
        return Err(format!("{target} isn't a version."));
    }
    // Only versions from the fetched channel list (newer than the current one): a typed or
    // stale version could be a downgrade, and without the list nothing can be checked.
    let Some(found) = status.target(target) else {
        return Err(if status.targets.is_empty() {
            "The channel list isn't available (update server unreachable), so no version can be \
             checked. Try again later."
                .to_string()
        } else {
            format!("{target} isn't an update offered by the channel list.")
        });
    };
    if !found.startable() {
        return Err(found.blocked.join(" "));
    }
    if status.progress.is_some() || status.pools.iter().any(|p| p.state == PoolState::Updating) {
        return Err("Plans are applying an update. Wait for it to finish.".into());
    }
    let pools: Vec<&Pool> = match scope {
        Scope::AllPools | Scope::ControlPlane => status
            .pools
            .iter()
            .filter(|p| p.kind == PoolKind::Plan && p.version.as_deref() != Some(target))
            .collect(),
        Scope::Pool(id) => status.pools.iter().filter(|p| &p.id == id).collect(),
        _ => Vec::new(),
    };
    if pools.is_empty() {
        return Err("No Plan to change.".into());
    }
    let refs: Vec<Value> = pools
        .iter()
        .filter_map(|p| p.object.as_ref())
        .map(|o| json!({"namespace": o.namespace, "name": o.name}))
        .collect();
    Ok(Plan {
        scope: scope.clone(),
        title: format!("Update {cluster} to {target}?"),
        from: status.current.version.clone(),
        to: target.to_string(),
        kind_label: "system-upgrade-controller Plans".into(),
        changes: pools
            .iter()
            .map(|p| {
                format!(
                    "Plan {}: spec.version {} → {target}",
                    p.object
                        .as_ref()
                        .map(|o| format!("{}/{}", o.namespace.clone().unwrap_or_default(), o.name))
                        .unwrap_or_else(|| p.name.clone()),
                    p.version.clone().unwrap_or_else(|| "unset".into())
                )
            })
            .collect(),
        risks: Vec::new(),
        notes: vec![
            "The controller cordons, drains and updates the selected nodes, as many at a time as each Plan's concurrency allows.".into(),
            "Server nodes first: agent Plans usually wait for the server Plan (their prepare step).".into(),
        ],
        irreversible: true,
        request: json!({"version": target, "plans": refs}),
    })
}

/// YAML for new server and agent Plans at `version` (the YAML editor opens it).
pub fn plan_templates(kind: ProviderKind, version: &str) -> String {
    let image = if kind == ProviderKind::Rke2 {
        "rancher/rke2-upgrade"
    } else {
        "rancher/k3s-upgrade"
    };
    format!(
        "# system-upgrade-controller Plans: servers first, then agents.\n\
apiVersion: upgrade.cattle.io/v1\n\
kind: Plan\n\
metadata:\n  name: server-plan\n  namespace: system-upgrade\n\
spec:\n  concurrency: 1\n  cordon: true\n  nodeSelector:\n    matchExpressions:\n      - key: node-role.kubernetes.io/control-plane\n        operator: In\n        values: [\"true\"]\n  serviceAccountName: system-upgrade\n  upgrade:\n    image: {image}\n  version: {version}\n\
---\n\
apiVersion: upgrade.cattle.io/v1\n\
kind: Plan\n\
metadata:\n  name: agent-plan\n  namespace: system-upgrade\n\
spec:\n  concurrency: 1\n  cordon: true\n  drain:\n    force: true\n  nodeSelector:\n    matchExpressions:\n      - key: node-role.kubernetes.io/control-plane\n        operator: DoesNotExist\n  prepare:\n    args: [prepare, server-plan]\n    image: {image}\n  serviceAccountName: system-upgrade\n  upgrade:\n    image: {image}\n  version: {version}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plans() -> Vec<Value> {
        vec![
            json!({"metadata": {"namespace": "system-upgrade", "name": "server-plan"},
                   "spec": {"concurrency": 1, "cordon": true, "version": "v1.33.4+k3s1",
                            "nodeSelector": {"matchExpressions": [{"key": "node-role.kubernetes.io/control-plane", "operator": "In", "values": ["true"]}]}},
                   "status": {"latestVersion": "v1.33.4+k3s1", "latestHash": "abc"}}),
            json!({"metadata": {"namespace": "system-upgrade", "name": "agent-plan"},
                   "spec": {"concurrency": 1, "drain": {"force": true}, "version": "v1.33.4+k3s1",
                            "nodeSelector": {"matchExpressions": [{"key": "node-role.kubernetes.io/control-plane", "operator": "DoesNotExist"}]}},
                   "status": {"latestVersion": "v1.33.4+k3s1", "latestHash": "def", "applying": ["k3s-agent-2"]}}),
        ]
    }

    fn nodes() -> Vec<Value> {
        vec![
            json!({"metadata": {"name": "k3s-server-0", "labels": {"node-role.kubernetes.io/control-plane": "true", "plan.upgrade.cattle.io/server-plan": "abc"}}}),
            json!({"metadata": {"name": "k3s-agent-1", "labels": {"plan.upgrade.cattle.io/agent-plan": "def"}}}),
            json!({"metadata": {"name": "k3s-agent-2", "labels": {}}}),
        ]
    }

    #[test]
    fn plans_progress_and_targets() {
        let channels = parse_channels(&json!({"data": [
            {"id": "stable", "latest": "v1.33.6+k3s1"},
            {"id": "latest", "latest": "v1.34.2+k3s1"},
            {"id": "v1.33", "latest": "v1.33.6+k3s1"},
            {"id": "v1.35", "latest": "v1.35.0+k3s1"},
            {"id": "v1.32", "latest": "v1.32.9+k3s1"}
        ]}));
        let status = status(
            ProviderKind::K3s,
            "v1.33.4+k3s1",
            &plans(),
            &nodes(),
            channels,
        );
        let server = &status.pools[0];
        assert_eq!((server.updated, server.nodes), (Some(1), Some(1)));
        let agent = &status.pools[1];
        assert_eq!((agent.updated, agent.nodes), (Some(1), Some(2)));
        assert_eq!(agent.state, PoolState::Updating);
        assert_eq!(agent.draining.as_deref(), Some("applying on k3s-agent-2"));
        assert!(status.progress.is_some());
        // While Plans apply, nothing else starts.
        assert!(status.targets.iter().all(|t| t.kind == TargetKind::Blocked));
        let versions: Vec<&str> = status.targets.iter().map(|t| t.version.as_str()).collect();
        assert_eq!(versions, ["v1.35.0+k3s1", "v1.34.2+k3s1", "v1.33.6+k3s1"]);
        assert_eq!(
            status.target("v1.33.6+k3s1").unwrap().channels,
            ["stable", "v1.33"]
        );
    }

    #[test]
    fn recommends_stable_and_blocks_skips() {
        let mut idle = plans();
        idle[1]["status"]["applying"] = json!([]);
        let channels = vec![
            ("stable".to_string(), "v1.33.6+k3s1".to_string()),
            ("v1.35".to_string(), "v1.35.0+k3s1".to_string()),
        ];
        let status = status(ProviderKind::K3s, "v1.33.4+k3s1", &idle, &nodes(), channels);
        assert!(status.progress.is_none());
        assert_eq!(status.suggested().unwrap().version, "v1.33.6+k3s1");
        assert_eq!(
            status.target("v1.35.0+k3s1").unwrap().kind,
            TargetKind::Blocked
        );
        let update = plan(&status, &Scope::AllPools, "v1.33.6+k3s1", "k3s-edge").unwrap();
        assert_eq!(update.request["plans"].as_array().unwrap().len(), 2);
        assert!(
            update.changes[0].contains("server-plan: spec.version v1.33.4+k3s1 → v1.33.6+k3s1")
        );
        assert!(plan(&status, &Scope::AllPools, "v1.35.0+k3s1", "k3s-edge").is_err());
        // Not in the channel list (a downgrade or a typo), or no list at all.
        for version in ["v1.32.0+k3s1", "v1.33.4+k3s1", "v1.99.0+k3s1"] {
            let err = plan(&status, &Scope::AllPools, version, "k3s-edge").unwrap_err();
            assert!(err.contains("channel list"), "{err}");
        }
        let offline = super::status(ProviderKind::K3s, "v1.33.4+k3s1", &idle, &nodes(), vec![]);
        let err = plan(&offline, &Scope::AllPools, "v1.34", "k3s-edge").unwrap_err();
        assert!(err.contains("isn't available"), "{err}");
        // Plans still applying: even a listed version waits.
        let mut busy = status.clone();
        busy.pools[0].state = PoolState::Updating;
        let err = plan(&busy, &Scope::AllPools, "v1.33.6+k3s1", "k3s-edge").unwrap_err();
        assert!(err.contains("applying"), "{err}");
        let yaml = plan_templates(ProviderKind::K3s, "v1.33.6+k3s1");
        assert!(kubyl_yaml::parse::parse(&yaml).error.is_none());
        assert_eq!(kubyl_yaml::parse::parse(&yaml).roots().count(), 2);
    }

    #[test]
    fn node_selectors() {
        let selector = json!({"matchLabels": {"a": "1"}, "matchExpressions": [{"key": "b", "operator": "NotIn", "values": ["x"]}]});
        assert!(selects(
            Some(&selector),
            &json!({"metadata": {"labels": {"a": "1"}}})
        ));
        assert!(!selects(
            Some(&selector),
            &json!({"metadata": {"labels": {"a": "1", "b": "x"}}})
        ));
        assert!(!selects(
            Some(&selector),
            &json!({"metadata": {"labels": {}}})
        ));
        assert!(selects(None, &json!({})));
    }
}
