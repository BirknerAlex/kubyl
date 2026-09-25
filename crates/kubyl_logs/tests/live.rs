//! Log streaming against a real cluster. Ignored by default; run with the dev cluster:
//!
//! ```sh
//! script/dev-cluster.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_logs --test live -- --ignored --nocapture
//! ```
//!
//! The test creates (and deletes) its own namespace `kubyl-live-logs` with a 3-replica
//! deployment that logs a numbered line every 100 ms.

use std::collections::HashSet;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Pod};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kubyl_logs::stream::{self, ContainerFilter, LogOptions, LogSource, Since, StreamEvent};

const NAMESPACE: &str = "kubyl-live-logs";

async fn client() -> kube::Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("KUBYL_TEST_KUBECONFIG");
    let kubeconfig = kube::config::Kubeconfig::read_from(path).expect("kubeconfig");
    let options = kube::config::KubeConfigOptions {
        context: std::env::var("KUBYL_TEST_CONTEXT").ok(),
        ..Default::default()
    };
    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .expect("config");
    kube::Client::try_from(config).expect("client")
}

async fn setup(client: &kube::Client) {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns: Namespace = serde_json::from_value(serde_json::json!({
        "metadata": {"name": NAMESPACE}
    }))
    .unwrap();
    namespaces.create(&PostParams::default(), &ns).await.ok();
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), NAMESPACE);
    let deployment: Deployment = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "ticker"},
        "spec": {
            "replicas": 3,
            "selector": {"matchLabels": {"app": "ticker"}},
            "template": {
                "metadata": {"labels": {"app": "ticker"}},
                "spec": {
                    "terminationGracePeriodSeconds": 1,
                    "containers": [{
                        "name": "ticker",
                        "image": "busybox:1.37",
                        "command": ["sh", "-c",
                            "i=0; while true; do i=$((i+1)); echo \"{\\\"level\\\":\\\"info\\\",\\\"n\\\":$i,\\\"pod\\\":\\\"$HOSTNAME\\\"}\"; if [ $((i % 10)) -eq 0 ]; then echo \"WARN upstream timeout $i\"; fi; sleep 0.1; done"]
                    }]
                }
            }
        }
    }))
    .unwrap();
    deployments
        .create(&PostParams::default(), &deployment)
        .await
        .ok();
    // Wait for three running pods.
    let pods: Api<Pod> = Api::namespaced(client.clone(), NAMESPACE);
    for _ in 0..120 {
        let list = pods
            .list(&ListParams::default().labels("app=ticker"))
            .await
            .unwrap();
        let running = list
            .items
            .iter()
            .filter(|p| {
                p.metadata.deletion_timestamp.is_none()
                    && p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
            })
            .count();
        if running == 3 {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("the ticker pods didn't start");
}

async fn teardown(client: &kube::Client) {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    namespaces
        .delete(NAMESPACE, &DeleteParams::default())
        .await
        .ok();
}

#[tokio::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
async fn workload_logs_interleave_in_order_and_follow_pod_changes() {
    let client = client().await;
    setup(&client).await;
    // Let every pod write a backlog.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let (tx, mut rx) = mpsc::unbounded();
    let options = LogOptions {
        follow: true,
        since: Since::Start,
        tail_lines: Some(20),
        previous: false,
    };
    let task = tokio::spawn(stream::run(
        client.clone(),
        NAMESPACE.into(),
        LogSource::Selector {
            label_selector: "app=ticker".into(),
        },
        ContainerFilter::default(),
        options,
        tx,
    ));

    // The first batch is the merged backlog of all three pods, in timestamp order.
    let mut joined = HashSet::new();
    let backlog = loop {
        match tokio::time::timeout(Duration::from_secs(30), rx.next())
            .await
            .expect("no backlog")
            .expect("stream ended")
        {
            StreamEvent::PodJoined { pod, initial, .. } => {
                assert!(initial);
                joined.insert(pod);
            }
            StreamEvent::Lines(lines) => break lines,
            other => println!("event: {other:?}"),
        }
    };
    assert_eq!(joined.len(), 3, "three pods join");
    let pods: HashSet<&str> = backlog.iter().map(|l| l.pod.as_str()).collect();
    assert_eq!(pods.len(), 3, "the backlog has lines of every pod");
    assert!(backlog.len() >= 50, "tail 20 × 3 pods: {}", backlog.len());
    let timestamps: Vec<_> = backlog.iter().map(|l| l.timestamp.unwrap()).collect();
    assert!(
        timestamps.windows(2).all(|w| w[0] <= w[1]),
        "the backlog is interleaved in timestamp order"
    );
    // The timestamp prefix is split off: the text is the raw JSON line.
    assert!(backlog[0].text.starts_with('{'), "{}", backlog[0].text);

    // Live lines keep arriving, from every pod.
    let mut live_pods = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(StreamEvent::Lines(lines))) =
            tokio::time::timeout(Duration::from_secs(1), rx.next()).await
        {
            live_pods.extend(lines.into_iter().map(|l| l.pod));
        }
    }
    assert_eq!(live_pods.len(), 3, "live lines from every pod");

    // Killing a pod: it leaves, its replacement joins.
    let victim = joined.iter().next().unwrap().clone();
    let pods_api: Api<Pod> = Api::namespaced(client.clone(), NAMESPACE);
    pods_api
        .delete(&victim, &DeleteParams::default().grace_period(0))
        .await
        .unwrap();
    let mut gone = false;
    let mut replacement = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while tokio::time::Instant::now() < deadline && !(gone && replacement.is_some()) {
        match tokio::time::timeout(Duration::from_secs(5), rx.next()).await {
            Ok(Some(StreamEvent::PodGone(pod))) if pod == victim => gone = true,
            Ok(Some(StreamEvent::PodJoined { pod, initial, .. })) => {
                assert!(!initial);
                replacement = Some(pod);
            }
            _ => {}
        }
    }
    println!("{victim} left: {gone}; replacement: {replacement:?}");
    task.abort();
    teardown(&client).await;
    assert!(gone, "the deleted pod leaves");
    assert!(replacement.is_some(), "the replacement joins");
}
