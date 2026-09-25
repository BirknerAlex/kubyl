//! The events stream against a real cluster. Ignored by default; run against the kind dev
//! cluster:
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_overview --test live -- --ignored --nocapture
//! ```
//!
//! Creates (and deletes) a pod in `payments` that is OOM-killed.

use std::time::{Duration, Instant};

use futures::StreamExt as _;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::ApiResource;
use kube::runtime::watcher;
use kube::{Client, Config};
use kubyl_overview::events::model::{event_row, group, oom_rows};

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

/// The pod's OOM kill becomes a Warning row about as fast as the watch delivers the status.
#[tokio::test]
#[ignore]
async fn oom_kills_show_up_within_two_seconds() {
    let client = client().await;
    let pods: Api<Pod> = Api::namespaced(client.clone(), NS);
    let name = format!("kubyl-oom-test-{}", std::process::id());
    let pod: Pod = serde_json::from_value(serde_json::json!({
        "metadata": {"name": name, "labels": {"app": "kubyl-oom-test"}},
        "spec": {
            "restartPolicy": "Never",
            "containers": [{
                "name": "hog",
                "image": "python:3.14-alpine",
                "command": ["python3", "-c", "import time\nb=[]\nwhile True:\n  b.append(bytearray(8<<20)); time.sleep(0.2)"],
                "resources": {"limits": {"memory": "48Mi"}}
            }]
        }
    }))
    .unwrap();
    pods.create(&PostParams::default(), &pod).await.unwrap();

    let config = watcher::Config::default().fields(&format!("metadata.name={name}"));
    let mut stream = watcher::watcher(pods.clone(), config).boxed();
    let started = Instant::now();
    let mut seen = None;
    while started.elapsed() < Duration::from_secs(120) {
        let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(5), stream.next()).await
        else {
            continue;
        };
        let objects = match event.unwrap() {
            watcher::Event::Apply(pod) | watcher::Event::InitApply(pod) => vec![pod],
            _ => continue,
        };
        for pod in objects {
            let value = serde_json::to_value(&pod).unwrap();
            if let Some(row) = oom_rows(&value).into_iter().next() {
                seen = Some((jiff::Timestamp::now(), row));
                break;
            }
        }
        if seen.is_some() {
            break;
        }
    }
    pods.delete(&name, &DeleteParams::default()).await.ok();

    let (at, row) = seen.expect("no OOMKilled row within 2 minutes");
    let killed = row.last.expect("finishedAt");
    // finishedAt has one-second resolution.
    let lag = at.duration_since(killed).as_secs_f64();
    println!("OOMKilled row {lag:.2}s after the kill: {}", row.message);
    assert!(lag < 3.0, "row appeared {lag:.2}s after the kill");
    assert!(row.warning && row.derived);
    assert_eq!(row.reason.as_ref(), "OOMKilled");
    assert_eq!(
        row.message.as_ref(),
        "Container hog was OOM-killed (memory limit 48Mi)."
    );
}

/// Both Event APIs parse into rows with objects, and repeats fold.
#[tokio::test]
#[ignore]
async fn events_parse_from_both_apis() {
    let client = client().await;
    for (group_name, version) in [("events.k8s.io", "v1"), ("", "v1")] {
        let resource = ApiResource {
            group: group_name.into(),
            version: version.into(),
            api_version: if group_name.is_empty() {
                version.into()
            } else {
                format!("{group_name}/{version}")
            },
            kind: "Event".into(),
            plural: "events".into(),
        };
        let api: Api<kube::api::DynamicObject> =
            Api::namespaced_with(client.clone(), NS, &resource);
        let list = api.list(&ListParams::default()).await.unwrap();
        let rows: Vec<_> = list
            .items
            .iter()
            .map(|e| event_row(&serde_json::to_value(e).unwrap()))
            .collect();
        assert!(!rows.is_empty(), "{group_name}: no events in {NS}");
        assert!(
            rows.iter()
                .all(|r| !r.kind.is_empty() && !r.name.is_empty())
        );
        assert!(
            rows.iter().all(|r| r.last.is_some()),
            "{group_name}: missing times"
        );
        let total: u64 = rows.iter().map(|r| r.count).sum();
        let grouped = group(rows.clone(), true);
        assert!(grouped.len() <= rows.len());
        assert_eq!(grouped.iter().map(|r| r.count).sum::<u64>(), total);
        println!(
            "{}: {} events, {} rows grouped, {} warnings",
            resource.api_version,
            rows.len(),
            grouped.len(),
            grouped.iter().filter(|r| r.warning).count()
        );
    }
}
