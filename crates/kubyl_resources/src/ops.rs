//! Mutating operations behind the first batch of actions. All of them are async and run on the
//! Tokio runtime (`kubyl_core::spawn_kube`); errors are user-facing strings.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use k8s_openapi::api::apps::v1::{ControllerRevision, Deployment, ReplicaSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{Node, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use kube::api::{
    Api, DeleteParams, DynamicObject, EvictParams, ListParams, Patch, PatchParams, PostParams,
};
use kube::discovery::ApiResource;
use kube::{Client, ResourceExt as _};
use serde_json::json;

/// Field manager for Kubyl's writes.
pub const FIELD_MANAGER: &str = "kubyl";

fn error(err: kube::Error) -> String {
    match err {
        kube::Error::Api(status) => status.message,
        other => other.to_string(),
    }
}

fn dynamic(client: Client, resource: &ApiResource, namespace: Option<&str>) -> Api<DynamicObject> {
    match namespace {
        Some(ns) => Api::namespaced_with(client, ns, resource),
        None => Api::all_with(client, resource),
    }
}

fn patch_params() -> PatchParams {
    PatchParams {
        field_manager: Some(FIELD_MANAGER.into()),
        ..Default::default()
    }
}

/// How a delete is done.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeleteOptions {
    /// `None`: the object's default grace period.
    pub grace_period: Option<u32>,
    /// Delete immediately (grace period 0), like `k9s` kill / `kubectl delete --force`.
    pub force: bool,
}

pub async fn delete(
    client: Client,
    resource: ApiResource,
    namespace: Option<String>,
    name: String,
    options: DeleteOptions,
) -> Result<(), String> {
    let api = dynamic(client, &resource, namespace.as_deref());
    let params = DeleteParams {
        grace_period_seconds: if options.force {
            Some(0)
        } else {
            options.grace_period
        },
        propagation_policy: Some(kube::api::PropagationPolicy::Background),
        ..Default::default()
    };
    match api.delete(&name, &params).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(status)) if status.code == 404 => Ok(()),
        Err(err) => Err(error(err)),
    }
}

/// Sets `spec.replicas` through the `scale` subresource.
pub async fn scale(
    client: Client,
    resource: ApiResource,
    namespace: Option<String>,
    name: String,
    replicas: u32,
) -> Result<(), String> {
    let api = dynamic(client, &resource, namespace.as_deref());
    api.patch_scale(
        &name,
        &patch_params(),
        &Patch::Merge(json!({"spec": {"replicas": replicas}})),
    )
    .await
    .map(|_| ())
    .map_err(error)
}

/// `kubectl rollout restart`: bumps the `restartedAt` template annotation.
pub async fn rollout_restart(
    client: Client,
    resource: ApiResource,
    namespace: Option<String>,
    name: String,
) -> Result<(), String> {
    let api = dynamic(client, &resource, namespace.as_deref());
    let now = jiff::Timestamp::now().to_string();
    let patch = json!({"spec": {"template": {"metadata": {"annotations": {
        "kubectl.kubernetes.io/restartedAt": now
    }}}}});
    api.patch(&name, &patch_params(), &Patch::Merge(patch))
        .await
        .map(|_| ())
        .map_err(error)
}

/// Pauses or resumes a Deployment rollout.
pub async fn set_paused(
    client: Client,
    namespace: String,
    name: String,
    paused: bool,
) -> Result<(), String> {
    let api: Api<Deployment> = Api::namespaced(client, &namespace);
    api.patch(
        &name,
        &patch_params(),
        &Patch::Merge(json!({"spec": {"paused": paused}})),
    )
    .await
    .map(|_| ())
    .map_err(error)
}

/// One revision of a Deployment (a ReplicaSet it owns).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub revision: i64,
    pub replica_set: String,
    pub images: Vec<String>,
    pub change_cause: Option<String>,
    pub created: Option<String>,
    pub current: bool,
}

const REVISION: &str = "deployment.kubernetes.io/revision";

async fn owned_replica_sets(
    client: &Client,
    namespace: &str,
    deployment: &Deployment,
) -> Result<Vec<ReplicaSet>, String> {
    let uid = deployment.metadata.uid.clone().unwrap_or_default();
    let selector = deployment
        .spec
        .as_ref()
        .and_then(|s| s.selector.match_labels.as_ref())
        .map(|labels| {
            labels
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let api: Api<ReplicaSet> = Api::namespaced(client.clone(), namespace);
    let list = api
        .list(&ListParams::default().labels(&selector))
        .await
        .map_err(error)?;
    Ok(list
        .items
        .into_iter()
        .filter(|rs| rs.owner_references().iter().any(|o| o.uid == uid))
        .collect())
}

fn revision_of(rs: &ReplicaSet) -> i64 {
    rs.annotations()
        .get(REVISION)
        .and_then(|r| r.parse().ok())
        .unwrap_or_default()
}

/// `kubectl rollout history`, newest first.
pub async fn rollout_history(
    client: Client,
    namespace: String,
    name: String,
) -> Result<Vec<Revision>, String> {
    let api: Api<Deployment> = Api::namespaced(client.clone(), &namespace);
    let deployment = api.get(&name).await.map_err(error)?;
    let current = deployment
        .annotations()
        .get(REVISION)
        .and_then(|r| r.parse::<i64>().ok());
    let mut revisions: Vec<Revision> = owned_replica_sets(&client, &namespace, &deployment)
        .await?
        .iter()
        .map(|rs| {
            let revision = revision_of(rs);
            Revision {
                revision,
                replica_set: rs.name_any(),
                images: rs
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.as_ref())
                    .and_then(|t| t.spec.as_ref())
                    .map(|s| {
                        s.containers
                            .iter()
                            .filter_map(|c| c.image.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                change_cause: rs.annotations().get("kubernetes.io/change-cause").cloned(),
                created: rs
                    .metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|t| t.0.to_string()),
                current: Some(revision) == current,
            }
        })
        .filter(|r| r.revision > 0)
        .collect();
    revisions.sort_by_key(|r| std::cmp::Reverse(r.revision));
    Ok(revisions)
}

/// `kubectl rollout undo --to-revision`: copies that ReplicaSet's pod template back.
pub async fn rollout_undo(
    client: Client,
    namespace: String,
    name: String,
    revision: i64,
) -> Result<(), String> {
    let api: Api<Deployment> = Api::namespaced(client.clone(), &namespace);
    let mut deployment = api.get(&name).await.map_err(error)?;
    let replica_sets = owned_replica_sets(&client, &namespace, &deployment).await?;
    let rs = replica_sets
        .iter()
        .find(|rs| revision_of(rs) == revision)
        .ok_or_else(|| format!("revision {revision} not found"))?;
    let mut template = rs
        .spec
        .as_ref()
        .and_then(|s| s.template.clone())
        .ok_or("the ReplicaSet has no pod template")?;
    if let Some(labels) = template.metadata.as_mut().and_then(|m| m.labels.as_mut()) {
        labels.remove("pod-template-hash");
    }
    let spec = deployment
        .spec
        .as_mut()
        .ok_or("the Deployment has no spec")?;
    if spec.paused == Some(true) {
        return Err("the rollout is paused; resume it first".into());
    }
    spec.template = template;
    api.replace(&name, &PostParams::default(), &deployment)
        .await
        .map(|_| ())
        .map_err(error)
}

/// Workloads with a rollout history: Deployments (ReplicaSets), StatefulSets and DaemonSets
/// (ControllerRevisions).
pub const WITH_HISTORY: &[&str] = &["deployments", "statefulsets", "daemonsets"];

/// `kubectl rollout history` for any workload in [`WITH_HISTORY`] (`resource` is the plural),
/// newest first.
pub async fn workload_history(
    client: Client,
    resource: &str,
    namespace: String,
    name: String,
) -> Result<Vec<Revision>, String> {
    match resource {
        "deployments" => rollout_history(client, namespace, name).await,
        "statefulsets" | "daemonsets" => {
            controller_history(client, resource, namespace, name).await
        }
        other => Err(format!("{other} have no rollout history")),
    }
}

/// `kubectl rollout undo --to-revision` for any workload in [`WITH_HISTORY`].
pub async fn workload_undo(
    client: Client,
    resource: &str,
    namespace: String,
    name: String,
    revision: i64,
) -> Result<(), String> {
    match resource {
        "deployments" => rollout_undo(client, namespace, name, revision).await,
        "statefulsets" | "daemonsets" => {
            controller_undo(client, resource, namespace, name, revision).await
        }
        other => Err(format!("{other} can't be rolled back")),
    }
}

/// The ControllerRevisions a StatefulSet or DaemonSet owns, with the pod template's images.
async fn owned_controller_revisions(
    client: &Client,
    resource: &str,
    namespace: &str,
    name: &str,
) -> Result<(serde_json::Value, Vec<ControllerRevision>), String> {
    let (group_version, kind) = match resource {
        "statefulsets" => ("apps/v1", "StatefulSet"),
        _ => ("apps/v1", "DaemonSet"),
    };
    let api_resource = ApiResource {
        group: "apps".into(),
        version: "v1".into(),
        api_version: group_version.into(),
        kind: kind.into(),
        plural: resource.into(),
    };
    let owner = dynamic(client.clone(), &api_resource, Some(namespace))
        .get(name)
        .await
        .map_err(error)?;
    let owner = serde_json::to_value(owner).map_err(|e| e.to_string())?;
    let uid = owner
        .pointer("/metadata/uid")
        .and_then(|u| u.as_str())
        .unwrap_or_default();
    let selector = owner
        .pointer("/spec/selector/matchLabels")
        .and_then(|m| m.as_object())
        .map(|labels| {
            labels
                .iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let api: Api<ControllerRevision> = Api::namespaced(client.clone(), namespace);
    let list = api
        .list(&ListParams::default().labels(&selector))
        .await
        .map_err(error)?;
    let revisions = list
        .items
        .into_iter()
        .filter(|cr| cr.owner_references().iter().any(|o| o.uid == uid))
        .collect();
    Ok((owner, revisions))
}

async fn controller_history(
    client: Client,
    resource: &str,
    namespace: String,
    name: String,
) -> Result<Vec<Revision>, String> {
    let (owner, revisions) =
        owned_controller_revisions(&client, resource, &namespace, &name).await?;
    // StatefulSets name their current revision; for DaemonSets the newest one is current.
    let current_name = owner
        .pointer("/status/updateRevision")
        .or_else(|| owner.pointer("/status/currentRevision"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let newest = revisions.iter().map(|r| r.revision).max();
    let mut out: Vec<Revision> = revisions
        .iter()
        .map(|cr| {
            let images = cr
                .data
                .as_ref()
                .and_then(|d| d.0.pointer("/spec/template/spec/containers"))
                .and_then(|c| c.as_array())
                .map(|containers| {
                    containers
                        .iter()
                        .filter_map(|c| c["image"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            Revision {
                revision: cr.revision,
                replica_set: cr.name_any(),
                images,
                change_cause: cr.annotations().get("kubernetes.io/change-cause").cloned(),
                created: cr
                    .metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|t| t.0.to_string()),
                current: match &current_name {
                    Some(current) => &cr.name_any() == current,
                    None => Some(cr.revision) == newest,
                },
            }
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.revision));
    Ok(out)
}

/// What `kubectl rollout undo` does for StatefulSets and DaemonSets: the revision's `data` is a
/// strategic merge patch of the pod template.
async fn controller_undo(
    client: Client,
    resource: &str,
    namespace: String,
    name: String,
    revision: i64,
) -> Result<(), String> {
    let (_, revisions) = owned_controller_revisions(&client, resource, &namespace, &name).await?;
    let cr = revisions
        .iter()
        .find(|cr| cr.revision == revision)
        .ok_or_else(|| format!("revision {revision} not found"))?;
    let patch = cr
        .data
        .as_ref()
        .map(|d| d.0.clone())
        .ok_or("the revision has no data")?;
    let api_resource = ApiResource {
        group: "apps".into(),
        version: "v1".into(),
        api_version: "apps/v1".into(),
        kind: if resource == "statefulsets" {
            "StatefulSet".into()
        } else {
            "DaemonSet".into()
        },
        plural: resource.into(),
    };
    dynamic(client, &api_resource, Some(&namespace))
        .patch(&name, &patch_params(), &Patch::Strategic(patch))
        .await
        .map(|_| ())
        .map_err(error)
}

/// Marks a node (un)schedulable.
pub async fn cordon(client: Client, node: String, unschedulable: bool) -> Result<(), String> {
    let api: Api<Node> = Api::all(client);
    api.patch(
        &node,
        &patch_params(),
        &Patch::Merge(json!({"spec": {"unschedulable": unschedulable}})),
    )
    .await
    .map(|_| ())
    .map_err(error)
}

/// Progress of a node drain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainProgress {
    /// Pods to evict.
    pub total: usize,
    pub evicted: usize,
    /// DaemonSet and mirror pods left in place.
    pub skipped: usize,
    /// Pods whose eviction a PodDisruptionBudget currently blocks (retried).
    pub blocked: Vec<String>,
    /// Evicted pods still terminating.
    pub terminating: usize,
    pub done: bool,
    pub error: Option<String>,
}

fn skip_on_drain(pod: &Pod) -> bool {
    let mirror = pod
        .annotations()
        .contains_key("kubernetes.io/config.mirror");
    let daemon = pod
        .owner_references()
        .iter()
        .any(|o| o.kind == "DaemonSet" && o.controller == Some(true));
    mirror || daemon
}

/// `kubectl drain --ignore-daemonsets --delete-emptydir-data`: cordons the node, then evicts
/// its pods through the Eviction API, so PodDisruptionBudgets are respected. Blocked evictions
/// are retried every 5 s until `timeout`. Reports progress on `tx`.
pub async fn drain(
    client: Client,
    node: String,
    timeout: Duration,
    tx: UnboundedSender<DrainProgress>,
) -> Result<DrainProgress, String> {
    cordon(client.clone(), node.clone(), true).await?;
    let pods_api: Api<Pod> = Api::all(client.clone());
    let on_node = ListParams::default().fields(&format!("spec.nodeName={node}"));
    let pods = pods_api.list(&on_node).await.map_err(error)?.items;
    let (skipped, mut pending): (Vec<Pod>, Vec<Pod>) = pods.into_iter().partition(skip_on_drain);
    let mut progress = DrainProgress {
        total: pending.len(),
        skipped: skipped.len(),
        ..Default::default()
    };
    tx.unbounded_send(progress.clone()).ok();

    let deadline = tokio::time::Instant::now() + timeout;
    while !pending.is_empty() {
        progress.blocked.clear();
        let mut still = Vec::new();
        for pod in pending {
            let namespace = pod.namespace().unwrap_or_default();
            let name = pod.name_any();
            let api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
            match api.evict(&name, &EvictParams::default()).await {
                Ok(_) => progress.evicted += 1,
                Err(kube::Error::Api(status)) if status.code == 404 => progress.evicted += 1,
                Err(kube::Error::Api(status)) if status.code == 429 => {
                    progress.blocked.push(format!("{namespace}/{name}"));
                    still.push(pod);
                }
                Err(err) => {
                    progress.error = Some(format!("{namespace}/{name}: {}", error(err)));
                    progress.done = true;
                    tx.unbounded_send(progress.clone()).ok();
                    return Err(progress.error.clone().unwrap_or_default());
                }
            }
        }
        pending = still;
        tx.unbounded_send(progress.clone()).ok();
        if pending.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            progress.done = true;
            progress.error = Some(format!(
                "timed out: {} pod(s) blocked by disruption budgets",
                pending.len()
            ));
            tx.unbounded_send(progress.clone()).ok();
            return Err(progress.error.clone().unwrap_or_default());
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // Wait for evicted pods to go away (they may have long grace periods).
    loop {
        let left = pods_api
            .list(&on_node)
            .await
            .map_err(error)?
            .items
            .iter()
            .filter(|p| !skip_on_drain(p))
            .count();
        progress.terminating = left;
        if left == 0 || tokio::time::Instant::now() >= deadline {
            break;
        }
        tx.unbounded_send(progress.clone()).ok();
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    progress.done = true;
    tx.unbounded_send(progress.clone()).ok();
    Ok(progress)
}

/// Creates a Job from a CronJob's template, like `kubectl create job --from=cronjob/<name>`.
/// Returns the Job name.
pub async fn trigger_cronjob(
    client: Client,
    namespace: String,
    name: String,
) -> Result<String, String> {
    let cronjobs: Api<CronJob> = Api::namespaced(client.clone(), &namespace);
    let cronjob = cronjobs.get(&name).await.map_err(error)?;
    let template = cronjob.spec.job_template.clone();
    let suffix = jiff::Timestamp::now().as_second() % 1_000_000;
    let base: String = name.chars().take(45).collect();
    let job_name = format!("{base}-manual-{suffix}");
    let mut annotations: BTreeMap<String, String> = template
        .metadata
        .as_ref()
        .and_then(|m| m.annotations.clone())
        .unwrap_or_default();
    annotations.insert("cronjob.kubernetes.io/instantiate".into(), "manual".into());
    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name.clone()),
            namespace: Some(namespace.clone()),
            labels: template.metadata.as_ref().and_then(|m| m.labels.clone()),
            annotations: Some(annotations),
            owner_references: Some(vec![OwnerReference {
                api_version: "batch/v1".into(),
                kind: "CronJob".into(),
                name: name.clone(),
                uid: cronjob.metadata.uid.clone().unwrap_or_default(),
                controller: Some(true),
                block_owner_deletion: Some(true),
            }]),
            ..Default::default()
        },
        spec: template.spec,
        status: None,
    };
    let jobs: Api<Job> = Api::namespaced(client, &namespace);
    let params = PostParams {
        field_manager: Some(FIELD_MANAGER.into()),
        ..Default::default()
    };
    jobs.create(&params, &job).await.map_err(error)?;
    Ok(job_name)
}

/// Suspends or resumes a CronJob.
pub async fn set_suspended(
    client: Client,
    namespace: String,
    name: String,
    suspend: bool,
) -> Result<(), String> {
    let api: Api<CronJob> = Api::namespaced(client, &namespace);
    api.patch(
        &name,
        &patch_params(),
        &Patch::Merge(json!({"spec": {"suspend": suspend}})),
    )
    .await
    .map(|_| ())
    .map_err(error)
}

/// Fetches one object as JSON (for Copy YAML of metadata-only rows, describe).
pub async fn get_json(
    client: Client,
    resource: ApiResource,
    namespace: Option<String>,
    name: String,
) -> Result<serde_json::Value, String> {
    let api = dynamic(client, &resource, namespace.as_deref());
    let object = api.get(&name).await.map_err(error)?;
    crate::store::to_json(&object).ok_or_else(|| "serialization failed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_skips_daemonset_and_mirror_pods() {
        let mut pod = Pod::default();
        assert!(!skip_on_drain(&pod));
        pod.metadata.owner_references = Some(vec![OwnerReference {
            kind: "DaemonSet".into(),
            controller: Some(true),
            ..Default::default()
        }]);
        assert!(skip_on_drain(&pod));
        let mut mirror = Pod::default();
        mirror.metadata.annotations = Some(BTreeMap::from([(
            "kubernetes.io/config.mirror".to_string(),
            "x".to_string(),
        )]));
        assert!(skip_on_drain(&mirror));
    }
}
