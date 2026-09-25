//! Port-forwarding against a real cluster. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_portforward --test live -- --ignored --nocapture
//! ```
//!
//! Creates (and deletes) the namespace `kubyl-live-pf` with a one-replica nginx deployment and
//! a Service, forwards to the Service, deletes the pod and checks the forward still answers
//! once the replacement is ready.

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kubyl_portforward::listener::{self, ForwardEvent};
use kubyl_portforward::resolve::{ForwardKind, RemotePort};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const NAMESPACE: &str = "kubyl-live-pf";

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

async fn ready_pods(client: &kube::Client) -> Vec<String> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), NAMESPACE);
    pods.list(&ListParams::default().labels("app=web"))
        .await
        .map(|l| l.items)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| {
            p.metadata.deletion_timestamp.is_none()
                && p.status
                    .as_ref()
                    .and_then(|s| s.conditions.as_ref())
                    .is_some_and(|c| c.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
        })
        .filter_map(|p| p.metadata.name)
        .collect()
}

async fn setup(client: &kube::Client) {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns: Namespace =
        serde_json::from_value(serde_json::json!({"metadata": {"name": NAMESPACE}})).unwrap();
    namespaces.create(&PostParams::default(), &ns).await.ok();
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), NAMESPACE);
    let deployment: Deployment = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "web"},
        "spec": {
            "replicas": 1,
            "selector": {"matchLabels": {"app": "web"}},
            "template": {
                "metadata": {"labels": {"app": "web"}},
                "spec": {
                    "terminationGracePeriodSeconds": 0,
                    "containers": [{
                        "name": "nginx",
                        "image": "nginx:1.29-alpine",
                        "ports": [{"name": "http", "containerPort": 80}],
                        "readinessProbe": {"httpGet": {"path": "/", "port": "http"}, "periodSeconds": 1}
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
    let services: Api<Service> = Api::namespaced(client.clone(), NAMESPACE);
    let service: Service = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "web"},
        "spec": {"selector": {"app": "web"}, "ports": [{"name": "http", "port": 80, "targetPort": "http"}]}
    }))
    .unwrap();
    services.create(&PostParams::default(), &service).await.ok();
    for _ in 0..120 {
        if !ready_pods(client).await.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("nginx didn't get ready");
}

async fn teardown(client: &kube::Client) {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    namespaces
        .delete(NAMESPACE, &DeleteParams::default())
        .await
        .ok();
}

/// One HTTP GET through the forward; returns the status line.
async fn get(port: u16) -> Option<String> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .ok()?;
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: web\r\n\r\n")
        .await
        .ok()?;
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut response))
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&response);
    text.lines().next().map(str::to_string)
}

#[tokio::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
async fn service_forward_survives_a_pod_restart() {
    let client = client().await;
    setup(&client).await;

    let (events_tx, mut events_rx) = mpsc::unbounded();
    let task = tokio::spawn(listener::run(
        client.clone(),
        NAMESPACE.into(),
        ForwardKind::Service {
            service: "web".into(),
        },
        RemotePort::Service(Some(80)),
        "127.0.0.1".into(),
        0,
        events_tx,
    ));
    let port = loop {
        match events_rx.next().await.expect("listener events") {
            ForwardEvent::Listening { local_port } => break local_port,
            other => println!("event: {other:?}"),
        }
    };
    let status = get(port).await;
    assert_eq!(
        status.as_deref(),
        Some("HTTP/1.1 200 OK"),
        "before the restart"
    );

    // Kill the only pod; the Deployment replaces it.
    let old = ready_pods(&client).await;
    let pods: Api<Pod> = Api::namespaced(client.clone(), NAMESPACE);
    for pod in &old {
        pods.delete(pod, &DeleteParams::default().grace_period(0))
            .await
            .unwrap();
    }
    let mut answered = None;
    for _ in 0..90 {
        let ready = ready_pods(&client).await;
        if ready.iter().any(|p| !old.contains(p))
            && let Some(status) = get(port).await
        {
            answered = Some(status);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    task.abort();
    teardown(&client).await;
    assert_eq!(
        answered.as_deref(),
        Some("HTTP/1.1 200 OK"),
        "the same local port reaches the replacement pod"
    );
}
