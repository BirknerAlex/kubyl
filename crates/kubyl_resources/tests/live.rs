//! Operations against a real cluster. Ignored by default; run against the kind dev cluster:
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBYL_TEST_KUBECONFIG=<kubeconfig with kind-kubyl-dev> \
//!   cargo test -p kubyl_resources --test live -- --ignored --nocapture
//! ```
//!
//! Works in the `payments` namespace created by the dev script and leaves it as it found it
//! (scales back, uncordons, deletes what it created).

use std::time::Duration;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::ApiResource;
use kube::{Client, Config, ResourceExt as _};
use kubyl_resources::ops::{self, DeleteOptions};

const NS: &str = "payments";

async fn client() -> Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let kubeconfig = Kubeconfig::read_from(path).unwrap();
    let options = KubeConfigOptions {
        context: Some(std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

fn deployments() -> ApiResource {
    ApiResource::erase::<Deployment>(&())
}

async fn wait_for(mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..60 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("condition not reached in 30 s");
}

#[tokio::test]
#[ignore]
async fn workload_actions() {
    let client = client().await;
    let api: Api<Deployment> = Api::namespaced(client.clone(), NS);
    let name = "checkout-api".to_string();
    let before = api.get(&name).await.unwrap();
    let replicas = before.spec.as_ref().unwrap().replicas.unwrap_or(1) as u32;

    // Scale up and back.
    ops::scale(
        client.clone(),
        deployments(),
        Some(NS.into()),
        name.clone(),
        replicas + 1,
    )
    .await
    .unwrap();
    let scaled = api.get(&name).await.unwrap();
    assert_eq!(scaled.spec.unwrap().replicas, Some(replicas as i32 + 1));
    ops::scale(
        client.clone(),
        deployments(),
        Some(NS.into()),
        name.clone(),
        replicas,
    )
    .await
    .unwrap();

    // Restart creates a new revision; undo goes back to the previous one.
    ops::rollout_restart(client.clone(), deployments(), Some(NS.into()), name.clone())
        .await
        .unwrap();
    let mut history = Vec::new();
    wait_for(async || {
        history = ops::rollout_history(client.clone(), NS.into(), name.clone())
            .await
            .unwrap();
        history.len() >= 2 && history[0].current
    })
    .await;
    let previous = history[1].revision;
    ops::rollout_undo(client.clone(), NS.into(), name.clone(), previous)
        .await
        .unwrap();
    wait_for(async || {
        let history = ops::rollout_history(client.clone(), NS.into(), name.clone())
            .await
            .unwrap();
        // The undone revision is re-numbered as the newest one.
        history[0].current && history.iter().all(|r| r.revision != previous)
    })
    .await;

    // Pause and resume.
    ops::set_paused(client.clone(), NS.into(), name.clone(), true)
        .await
        .unwrap();
    assert_eq!(
        api.get(&name).await.unwrap().spec.unwrap().paused,
        Some(true)
    );
    ops::set_paused(client.clone(), NS.into(), name.clone(), false)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore]
async fn cronjob_node_and_pod_actions() {
    let client = client().await;

    // Trigger and suspend a CronJob.
    let cronjobs: Api<CronJob> = Api::namespaced(client.clone(), NS);
    let cronjob = cronjobs
        .list(&ListParams::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .next()
        .expect("the dev cluster has a CronJob");
    let job = ops::trigger_cronjob(client.clone(), NS.into(), cronjob.name_any())
        .await
        .unwrap();
    let jobs: Api<Job> = Api::namespaced(client.clone(), NS);
    let created = jobs.get(&job).await.unwrap();
    assert_eq!(created.owner_references()[0].name, cronjob.name_any());
    jobs.delete(&job, &DeleteParams::background())
        .await
        .unwrap();
    let suspended = cronjob.spec.suspend.unwrap_or(false);
    ops::set_suspended(client.clone(), NS.into(), cronjob.name_any(), !suspended)
        .await
        .unwrap();
    ops::set_suspended(client.clone(), NS.into(), cronjob.name_any(), suspended)
        .await
        .unwrap();

    // Cordon and uncordon a node.
    let nodes: Api<Node> = Api::all(client.clone());
    let node = nodes.list(&ListParams::default()).await.unwrap().items[0].name_any();
    ops::cordon(client.clone(), node.clone(), true)
        .await
        .unwrap();
    assert_eq!(
        nodes.get(&node).await.unwrap().spec.unwrap().unschedulable,
        Some(true)
    );
    ops::cordon(client.clone(), node.clone(), false)
        .await
        .unwrap();

    // Kill (grace 0) a pod created for the test, and fetch one as JSON first.
    let pods: Api<Pod> = Api::namespaced(client.clone(), NS);
    let pod: Pod = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "kubyl-live-test"},
        "spec": {"containers": [{"name": "pause", "image": "registry.k8s.io/pause:3.10"}]}
    }))
    .unwrap();
    pods.create(&PostParams::default(), &pod).await.unwrap();
    let pod_resource = ApiResource::erase::<Pod>(&());
    let json = ops::get_json(
        client.clone(),
        pod_resource.clone(),
        Some(NS.into()),
        "kubyl-live-test".into(),
    )
    .await
    .unwrap();
    assert!(json["metadata"].get("managedFields").is_none());
    ops::delete(
        client.clone(),
        pod_resource,
        Some(NS.into()),
        "kubyl-live-test".into(),
        DeleteOptions {
            grace_period: None,
            force: true,
        },
    )
    .await
    .unwrap();
    wait_for(async || pods.get_opt("kubyl-live-test").await.unwrap().is_none()).await;
}

#[tokio::test]
#[ignore]
async fn drain_control_plane() {
    let client = client().await;
    let nodes: Api<Node> = Api::all(client.clone());
    let node = nodes
        .list(&ListParams::default().labels("node-role.kubernetes.io/control-plane"))
        .await
        .unwrap()
        .items[0]
        .name_any();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let result = ops::drain(client.clone(), node.clone(), Duration::from_secs(120), tx).await;
    ops::cordon(client.clone(), node.clone(), false)
        .await
        .unwrap();
    let progress = result.unwrap();
    assert!(progress.done);
    assert!(progress.skipped > 0, "static and DaemonSet pods stay");
    assert_eq!(progress.evicted, progress.total);
    let mut updates = 0;
    while rx.try_recv().is_ok() {
        updates += 1;
    }
    assert!(updates >= 2);
    println!("drained {node}: {progress:?}");
}

#[tokio::test]
#[ignore]
async fn statefulset_history_and_undo() {
    use k8s_openapi::api::apps::v1::StatefulSet;
    let client = client().await;
    let name = "ledger-writer".to_string();
    let resource = ApiResource::erase::<StatefulSet>(&());

    // A restart creates a second ControllerRevision.
    ops::rollout_restart(client.clone(), resource, Some(NS.into()), name.clone())
        .await
        .unwrap();
    let mut history = Vec::new();
    wait_for(async || {
        history = ops::workload_history(client.clone(), "statefulsets", NS.into(), name.clone())
            .await
            .unwrap();
        history.len() >= 2 && history[0].current
    })
    .await;
    assert!(history.iter().filter(|r| r.current).count() == 1);
    assert!(!history[0].images.is_empty());

    // Undo to the previous revision: the template matches it again.
    let previous = history[1].revision;
    ops::workload_undo(
        client.clone(),
        "statefulsets",
        NS.into(),
        name.clone(),
        previous,
    )
    .await
    .unwrap();
    wait_for(async || {
        let history =
            ops::workload_history(client.clone(), "statefulsets", NS.into(), name.clone())
                .await
                .unwrap();
        // The restored revision becomes the newest and current one.
        history[0].current && history[0].revision > previous
    })
    .await;
    let daemonsets = ops::workload_history(
        client.clone(),
        "daemonsets",
        "kube-system".into(),
        "kube-proxy".into(),
    )
    .await
    .unwrap();
    assert!(daemonsets.iter().any(|r| r.current));
}
