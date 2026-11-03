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

/// A log view of one pod keeps going when a StatefulSet recreates the pod under its name.
#[tokio::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
async fn single_pod_logs_follow_a_recreated_statefulset_pod() {
    use k8s_openapi::api::apps::v1::StatefulSet;
    const NS: &str = "kubyl-live-logs-sts";
    let client = client().await;
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns: Namespace =
        serde_json::from_value(serde_json::json!({"metadata": {"name": NS}})).unwrap();
    namespaces.create(&PostParams::default(), &ns).await.ok();
    let sets: Api<StatefulSet> = Api::namespaced(client.clone(), NS);
    let set: StatefulSet = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "ticker"},
        "spec": {
            "replicas": 1,
            "serviceName": "ticker",
            "selector": {"matchLabels": {"app": "ticker"}},
            "template": {
                "metadata": {"labels": {"app": "ticker"}},
                "spec": {
                    "terminationGracePeriodSeconds": 1,
                    "containers": [{
                        "name": "ticker",
                        "image": "busybox:1.37",
                        "command": ["sh", "-c",
                            "i=0; while true; do i=$((i+1)); echo \"$HOSTNAME $i\"; sleep 0.2; done"]
                    }]
                }
            }
        }
    }))
    .unwrap();
    sets.create(&PostParams::default(), &set).await.ok();
    let pods: Api<Pod> = Api::namespaced(client.clone(), NS);
    let uid_of_running = |pod: Option<Pod>| {
        pod.filter(|p| {
            p.metadata.deletion_timestamp.is_none()
                && p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
        })
        .and_then(|p| p.metadata.uid)
    };
    let mut first_uid = None;
    for _ in 0..120 {
        first_uid = uid_of_running(pods.get_opt("ticker-0").await.unwrap());
        if first_uid.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let first_uid = first_uid.expect("ticker-0 runs");
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (tx, mut rx) = mpsc::unbounded();
    let task = tokio::spawn(stream::run(
        client.clone(),
        NS.into(),
        LogSource::Pod {
            pod: "ticker-0".into(),
        },
        ContainerFilter::default(),
        LogOptions::default(),
        tx,
    ));
    // Lines arrive from the first instance.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut before = 0;
    while tokio::time::Instant::now() < deadline && before < 10 {
        if let Ok(Some(StreamEvent::Lines(lines))) =
            tokio::time::timeout(Duration::from_secs(1), rx.next()).await
        {
            before += lines.len();
        }
    }
    assert!(before >= 10, "lines before the delete: {before}");

    pods.delete("ticker-0", &DeleteParams::default().grace_period(0))
        .await
        .unwrap();
    // Lines written after this are the new pod's (the old one dies at once).
    let deleted_at = jiff::Timestamp::now() + Duration::from_secs(2);
    // Lines after the rejoin come from the new pod.
    let mut events = Vec::new();
    let mut rejoined = false;
    let mut after = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while tokio::time::Instant::now() < deadline && after < 10 {
        match tokio::time::timeout(Duration::from_secs(5), rx.next()).await {
            Ok(Some(StreamEvent::PodJoined { initial, .. })) => {
                assert!(!initial);
                rejoined = true;
                events.push("joined".to_string());
            }
            Ok(Some(StreamEvent::Lines(lines))) => {
                after += lines
                    .iter()
                    .filter(|l| l.timestamp.is_some_and(|t| t > deleted_at))
                    .count();
            }
            Ok(Some(other)) => events.push(format!("{other:?}")),
            Ok(None) => {
                events.push("stream ended".into());
                break;
            }
            Err(_) => {}
        }
    }
    let second_uid = uid_of_running(pods.get_opt("ticker-0").await.unwrap());
    println!("recreated at once: {events:?}; lines after: {after}");
    let recreated = (rejoined, after);

    // The pod is gone for a while (scaled to 0 and back): the view waits for it.
    let scale = |replicas: i32| {
        let sets = sets.clone();
        async move {
            sets.patch(
                "ticker",
                &kube::api::PatchParams::default(),
                &kube::api::Patch::Merge(serde_json::json!({"spec": {"replicas": replicas}})),
            )
            .await
            .unwrap();
        }
    };
    scale(0).await;
    for _ in 0..60 {
        if pods.get_opt("ticker-0").await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    scale(1).await;
    let back_at = jiff::Timestamp::now() + Duration::from_secs(1);
    let mut events = Vec::new();
    let mut back = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    while tokio::time::Instant::now() < deadline && back < 10 {
        match tokio::time::timeout(Duration::from_secs(5), rx.next()).await {
            Ok(Some(StreamEvent::Lines(lines))) => {
                back += lines
                    .iter()
                    .filter(|l| l.timestamp.is_some_and(|t| t > back_at))
                    .count();
            }
            Ok(Some(other)) => events.push(format!("{other:?}")),
            Ok(None) => {
                events.push("stream ended".into());
                break;
            }
            Err(_) => {}
        }
    }
    println!("gone for a while: {events:?}; lines after: {back}");
    task.abort();
    namespaces.delete(NS, &DeleteParams::default()).await.ok();
    assert_ne!(second_uid.as_deref(), Some(first_uid.as_str()), "a new pod");
    assert!(
        recreated.1 >= 10,
        "lines from the recreated pod: {}",
        recreated.1
    );
    assert!(recreated.0, "the recreated pod joins the view");
    assert!(
        back >= 10,
        "lines after the pod was gone for a while: {back}"
    );
}
