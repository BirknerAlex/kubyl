//! Kubyl's agent tools against a real cluster, through the MCP server over HTTP. Ignored by
//! default; run against the kind dev cluster:
//!
//! ```sh
//! script/dev-cluster.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev/kubeconfig
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_agent_core --test live -- --ignored --nocapture
//! ```
//!
//! The test creates the namespace `kubyl-agent-test` with a Secret whose value must never come
//! back unmasked, and deletes it again.

use std::sync::Arc;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::api::{Api, DeleteParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_agent_core::mcp::{self, McpServer};
use kubyl_agent_core::tools::ToolContext;
use serde_json::{Value, json};
use tokio::sync::watch;

const NAMESPACE: &str = "kubyl-agent-test";
const SECRET_VALUE: &str = "hunter2-kubyl-agent-test-value";

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

async fn call(server: &McpServer, name: &str, arguments: Value) -> (String, bool) {
    let stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(connection);
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let request = http::Request::post("/mcp")
        .header("host", server.addr.to_string())
        .header("authorization", format!("Bearer {}", server.token()))
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    (
        value["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        value["result"]["isError"].as_bool().unwrap_or(true),
    )
}

#[tokio::test]
#[ignore]
async fn tools_read_the_cluster_and_never_return_secrets() {
    let client = client().await;
    let discovery = kubyl_kube_core::discovery::discover(&client).await.unwrap();

    let namespaces: Api<Namespace> = Api::all(client.clone());
    namespaces
        .create(
            &PostParams::default(),
            &serde_json::from_value(json!({"metadata": {"name": NAMESPACE}})).unwrap(),
        )
        .await
        .ok();
    let secrets: Api<Secret> = Api::namespaced(client.clone(), NAMESPACE);
    secrets
        .create(
            &PostParams::default(),
            &serde_json::from_value(json!({
                "metadata": {"name": "db", "annotations": {"note": "test"}},
                "stringData": {"password": SECRET_VALUE},
            }))
            .unwrap(),
        )
        .await
        .ok();

    let context = ToolContext {
        cluster_name: "kind-kubyl-dev".into(),
        client: Some(client.clone()),
        discovery: Some(Arc::new(discovery)),
        max_output: 64 * 1024,
        log_lines: 50,
        ..ToolContext::default()
    };
    let (_tx, rx) = watch::channel(context);
    let server = mcp::serve(rx, None).await.unwrap();

    let (info, error) = call(&server, "cluster_info", json!({})).await;
    println!("{info}");
    assert!(!error && info.contains("Kubernetes: v1."), "{info}");

    let (pods, error) = call(
        &server,
        "list_resources",
        json!({"kind": "po", "namespace": "kube-system"}),
    )
    .await;
    assert!(
        !error && pods.contains("coredns") && pods.contains("STATUS"),
        "{pods}"
    );

    let (deployments, error) = call(
        &server,
        "list_resources",
        json!({"kind": "deployments.apps"}),
    )
    .await;
    assert!(!error && deployments.contains("NAMESPACE"), "{deployments}");

    for (tool, args) in [
        (
            "get_resource",
            json!({"kind": "secret", "namespace": NAMESPACE, "name": "db"}),
        ),
        (
            "describe",
            json!({"kind": "secret", "namespace": NAMESPACE, "name": "db"}),
        ),
        (
            "list_resources",
            json!({"kind": "secrets", "namespace": NAMESPACE}),
        ),
    ] {
        let (text, _) = call(&server, tool, args).await;
        println!("--- {tool}\n{text}");
        assert!(
            !text.contains(SECRET_VALUE),
            "{tool} leaked the secret: {text}"
        );
        assert!(!text.contains("aHVudGVy"), "{tool} leaked base64: {text}");
    }
    let (yaml, _) = call(
        &server,
        "get_resource",
        json!({"kind": "secret", "namespace": NAMESPACE, "name": "db"}),
    )
    .await;
    assert!(yaml.contains("••••••••"), "{yaml}");

    let (helm, error) = call(
        &server,
        "get_resource",
        json!({"kind": "secret", "namespace": "kube-system", "name": "sh.helm.release.v1.metrics-server.v1"}),
    )
    .await;
    assert!(error && helm.contains("Helm release"), "{helm}");
    let (helm_list, _) = call(
        &server,
        "list_resources",
        json!({"kind": "secrets", "namespace": "kube-system"}),
    )
    .await;
    assert!(!helm_list.contains("sh.helm.release"), "{helm_list}");

    let (events, error) = call(
        &server,
        "events",
        json!({"namespace": "kube-system", "limit": 5}),
    )
    .await;
    assert!(!error, "{events}");

    let (pods_json, _) = call(
        &server,
        "list_resources",
        json!({"kind": "pods", "namespace": "kube-system", "label_selector": "k8s-app=kube-dns"}),
    )
    .await;
    let pod = pods_json
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().next())
        .unwrap()
        .to_string();
    let (logs, error) = call(
        &server,
        "logs",
        json!({"namespace": "kube-system", "pod": pod, "tail_lines": 5}),
    )
    .await;
    assert!(!error, "{logs}");
    let (described, error) = call(
        &server,
        "describe",
        json!({"kind": "pod", "namespace": "kube-system", "name": pod}),
    )
    .await;
    assert!(!error && described.contains("Containers"), "{described}");

    let (top, _) = call(&server, "top", json!({"kind": "nodes"})).await;
    println!("--- top\n{top}");

    let (can, error) = call(
        &server,
        "can_i",
        json!({"verb": "list", "kind": "pods", "namespace": "default"}),
    )
    .await;
    assert!(!error && can.starts_with("yes"), "{can}");

    let (kinds, error) = call(&server, "api_resources", json!({"filter": "apps"})).await;
    assert!(!error && kinds.contains("deployments"), "{kinds}");

    let (missing, error) = call(
        &server,
        "get_resource",
        json!({"kind": "pod", "namespace": "default", "name": "does-not-exist"}),
    )
    .await;
    assert!(error && missing.starts_with("Not found"), "{missing}");

    namespaces
        .delete(NAMESPACE, &DeleteParams::default())
        .await
        .ok();
}

/// What the canned "Ask agent" questions tell an agent to do, done by hand: the tools they name
/// answer for a real Deployment of the cluster, and a Secret next to it stays masked.
#[tokio::test]
#[ignore]
async fn canned_prompts_name_tools_that_answer_for_a_real_deployment() {
    use kubyl_agent_core::prompts::{Prompt, Subject};

    let client = client().await;
    let discovery = kubyl_kube_core::discovery::discover(&client).await.unwrap();
    let context = ToolContext {
        cluster_name: "kind-kubyl-dev".into(),
        client: Some(client.clone()),
        discovery: Some(Arc::new(discovery)),
        max_output: 64 * 1024,
        log_lines: 50,
        ..ToolContext::default()
    };
    let (_tx, rx) = watch::channel(context);
    let server = mcp::serve(rx, None).await.unwrap();

    // CoreDNS is on every kind cluster.
    let subject = Subject {
        cluster: "kind-kubyl-dev".into(),
        kind: "Deployment".into(),
        group: "apps".into(),
        resource: "deployments".into(),
        namespace: Some("kube-system".into()),
        name: "coredns".into(),
    };
    for prompt in Prompt::ALL {
        assert!(prompt.applies_to("apps", "deployments"));
        let text = prompt.text(&subject);
        assert!(
            text.contains("Deployment.apps `kube-system/coredns`"),
            "{text}"
        );
    }
    let (described, error) = call(
        &server,
        "describe",
        json!({"kind": "deployments.apps", "namespace": "kube-system", "name": "coredns"}),
    )
    .await;
    assert!(!error && described.contains("coredns"), "{described}");
    let (events, error) = call(
        &server,
        "events",
        json!({"namespace": "kube-system", "kind": "Deployment", "name": "coredns", "limit": 10}),
    )
    .await;
    assert!(!error, "{events}");
    let (related, error) = call(
        &server,
        "list_resources",
        json!({"kind": "pods", "namespace": "kube-system", "label_selector": "k8s-app=kube-dns"}),
    )
    .await;
    assert!(!error && related.contains("coredns"), "{related}");
    let (usage, _) = call(
        &server,
        "top",
        json!({"kind": "pods", "namespace": "kube-system"}),
    )
    .await;
    println!("top: {usage}");
    // The summary of a Secret has no values.
    let (secret, _) = call(
        &server,
        "describe",
        json!({"kind": "secret", "namespace": NAMESPACE, "name": "db"}),
    )
    .await;
    assert!(!secret.contains(SECRET_VALUE), "{secret}");
}
