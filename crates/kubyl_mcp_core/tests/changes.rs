//! The change tools against a small fake API server: what they send, and above all what they
//! don't send when the user rejects or nothing would change.

use std::sync::Arc;

use futures::future::BoxFuture;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper_util::rt::TokioIo;
use kubyl_base::{ClusterId, Gvk, Gvr};
use kubyl_kube_core::discovery::{ApiResourceInfo, Discovery};
use kubyl_mcp_core::approval::{Approver, Decision, Proposal};
use kubyl_mcp_core::changes;
use kubyl_mcp_core::tools::ToolContext;
use parking_lot::Mutex;
use serde_json::{Value, json};

type Log = Arc<Mutex<Vec<String>>>;

fn deployment(replicas: u64) -> Value {
    json!({
        "apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": {"name": "web", "namespace": "shop", "resourceVersion": "7", "uid": "u-1"},
        "spec": {"replicas": replicas, "template": {"spec": {"containers": [{"name": "web", "image": "web:1"}]}}},
        "status": {"readyReplicas": replicas},
    })
}

fn body_named(name: &str, mut object: Value) -> Value {
    object["metadata"]["name"] = json!(name);
    object
}

fn status(code: u16, message: &str) -> Value {
    let reason = if code == 404 { "NotFound" } else { "x" };
    json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "message": message, "reason": reason, "code": code})
}

async fn handle(log: Log, request: http::Request<Incoming>) -> http::Response<Full<Bytes>> {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if method == "DELETE" || method == "PATCH" || method == "POST" {
        log.lock().push(format!("BODY {body}"));
    }
    log.lock().push(format!(
        "{method} {path}{}",
        if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        }
    ));
    let base = "/apis/apps/v1/namespaces/shop/deployments/web";
    // `api` doesn't exist until a test logs "CREATED api" (someone else created it).
    let new = "/apis/apps/v1/namespaces/shop/deployments/api";
    let created = log.lock().iter().any(|line| line == "CREATED api");
    let (code, answer) = match (method.as_str(), path.as_str()) {
        ("GET", p) if p == new && created => (200, body_named("api", deployment(1))),
        // Creating `api`: refused once someone else created it.
        ("POST", "/apis/apps/v1/namespaces/shop/deployments") if created => {
            (409, status(409, "deployments.apps \"api\" already exists"))
        }
        ("POST", "/apis/apps/v1/namespaces/shop/deployments") => (201, body.clone()),
        ("PATCH", p) if p == new => {
            let mut object = body.clone();
            object["metadata"]["uid"] = json!("u-2");
            object["metadata"]["resourceVersion"] = json!("1");
            (200, object)
        }
        ("GET", p) if p == base => (200, deployment(2)),
        ("GET", p) if p == format!("{base}/scale") => (
            200,
            json!({"apiVersion": "autoscaling/v1", "kind": "Scale", "metadata": {"name": "web", "namespace": "shop", "resourceVersion": "9"},
                   "spec": {"replicas": 3}}),
        ),
        ("PATCH", p) if p == format!("{base}/scale") => (
            200,
            json!({"apiVersion": "autoscaling/v1", "kind": "Scale", "metadata": {"name": "web", "namespace": "shop"},
                   "spec": {"replicas": body["spec"]["replicas"]}}),
        ),
        ("PATCH", p) if p == base => {
            let mut object = deployment(2);
            if let Some(replicas) = body["spec"]["replicas"].as_u64() {
                object["spec"]["replicas"] = json!(replicas);
            }
            (200, object)
        }
        ("DELETE", p) if p == base => (
            200,
            json!({"kind": "Status", "apiVersion": "v1", "status": "Success"}),
        ),
        _ => (404, status(404, "not found")),
    };
    http::Response::builder()
        .status(code)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(answer.to_string())))
        .unwrap()
}

async fn fake_api() -> (kube::Client, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let log: Log = Arc::default();
    let served = log.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let log = served.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    let log = log.clone();
                    async move { Ok::<_, std::convert::Infallible>(handle(log, request).await) }
                });
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                    .ok();
            });
        }
    });
    let config = kube::Config::new(format!("http://{addr}").parse().unwrap());
    (kube::Client::try_from(config).unwrap(), log)
}

fn info(
    group: &str,
    version: &str,
    kind: &str,
    plural: &str,
    subresources: &[&str],
) -> ApiResourceInfo {
    ApiResourceInfo {
        gvk: Gvk::new(group, version, kind),
        gvr: Gvr::new(group, version, plural),
        singular: kind.to_lowercase(),
        namespaced: true,
        verbs: ["get", "list", "patch", "delete", "create", "update"]
            .map(String::from)
            .to_vec(),
        short_names: if kind == "Deployment" {
            vec!["deploy".into()]
        } else {
            Vec::new()
        },
        categories: Vec::new(),
        subresources: subresources.iter().map(|s| s.to_string()).collect(),
        preferred: true,
    }
}

fn context(client: kube::Client) -> ToolContext {
    ToolContext {
        cluster: Some(ClusterId::new("kind-dev")),
        cluster_name: "kind-dev".into(),
        client: Some(client),
        discovery: Some(Arc::new(Discovery {
            groups: Vec::new(),
            resources: vec![
                info(
                    "apps",
                    "v1",
                    "Deployment",
                    "deployments",
                    &["scale", "status"],
                ),
                info("", "v1", "Secret", "secrets", &[]),
            ],
            aggregated: true,
        })),
        max_output: 64 * 1024,
        ..ToolContext::default()
    }
}

/// Records every proposal and answers with a fixed decision.
struct Recording {
    decision: Decision,
    seen: Arc<Mutex<Vec<Proposal>>>,
}

impl Approver for Recording {
    fn approve(&self, proposal: Proposal) -> BoxFuture<'static, Decision> {
        self.seen.lock().push(proposal);
        let decision = self.decision;
        Box::pin(async move { decision })
    }
}

fn approver(decision: Decision) -> (Arc<dyn Approver>, Arc<Mutex<Vec<Proposal>>>) {
    let seen: Arc<Mutex<Vec<Proposal>>> = Arc::default();
    (
        Arc::new(Recording {
            decision,
            seen: seen.clone(),
        }),
        seen,
    )
}

/// The JSON bodies of the PATCH and DELETE requests, in order.
fn bodies(log: &Log) -> Vec<Value> {
    log.lock()
        .iter()
        .filter_map(|line| line.strip_prefix("BODY "))
        .map(|body| serde_json::from_str(body).unwrap_or(Value::Null))
        .collect()
}

fn writes(log: &Log) -> Vec<String> {
    log.lock()
        .iter()
        .filter(|line| !line.starts_with("GET") && !line.starts_with("BODY"))
        .cloned()
        .collect()
}

#[tokio::test]
async fn an_approved_scale_is_sent_once_and_shows_the_before_and_after() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Approved);
    let output = changes::run(
        &context,
        &approver,
        "scale_resource",
        &json!({"kind": "deploy", "name": "web", "namespace": "shop", "replicas": 5}),
    )
    .await;
    assert!(!output.is_error, "{}", output.text);
    assert!(output.text.contains("Scaled web to 5"));
    assert_eq!(
        writes(&log),
        ["PATCH /apis/apps/v1/namespaces/shop/deployments/web/scale?&fieldManager=kubyl"]
    );
    // The scale endpoint got exactly the count that was approved.
    assert_eq!(
        bodies(&log),
        [json!({"metadata": {"resourceVersion": "9"}, "spec": {"replicas": 5}})]
    );
    let seen = seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].title, "Scale deployment shop/web from 3 to 5");
    assert_eq!(seen[0].cluster, ClusterId::new("kind-dev"));
    assert!(
        seen[0].docs[0]
            .old
            .as_deref()
            .unwrap()
            .contains("replicas: 3")
    );
    assert!(seen[0].docs[0].new.contains("replicas: 5"));
}

#[tokio::test]
async fn a_rejected_change_sends_nothing_and_tells_the_agent_not_to_retry() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Rejected);
    for (tool, args) in [
        (
            "scale_resource",
            json!({"kind": "deploy", "name": "web", "namespace": "shop", "replicas": 9}),
        ),
        (
            "delete_resource",
            json!({"kind": "deploy", "name": "web", "namespace": "shop"}),
        ),
        (
            "restart_resource",
            json!({"kind": "deploy", "name": "web", "namespace": "shop"}),
        ),
        (
            "patch_resource",
            json!({"kind": "deploy", "name": "web", "namespace": "shop", "patch": {"spec": {"replicas": 4}}}),
        ),
    ] {
        let output = changes::run(&context, &approver, tool, &args).await;
        assert!(output.is_error, "{tool}");
        assert!(
            output.text.contains("did not approve"),
            "{tool}: {}",
            output.text
        );
    }
    assert_eq!(seen.lock().len(), 4);
    // Only the dry run of the patch reached the server; no real write.
    let sent = writes(&log);
    assert!(sent.iter().all(|l| l.contains("dryRun=All")), "{sent:?}");
}

#[tokio::test]
async fn nothing_to_change_never_asks() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Approved);
    let output = changes::run(
        &context,
        &approver,
        "scale_resource",
        &json!({"kind": "deploy", "name": "web", "namespace": "shop", "replicas": 3}),
    )
    .await;
    assert!(!output.is_error);
    assert!(output.text.contains("Nothing would change"));
    assert!(seen.lock().is_empty());
    assert!(writes(&log).is_empty());
}

#[tokio::test]
async fn a_patch_is_dry_run_then_shown_as_a_diff_then_applied() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Approved);
    let output = changes::run(
        &context,
        &approver,
        "patch_resource",
        &json!({"kind": "deployments", "name": "web", "namespace": "shop", "patch": "{\"spec\":{\"replicas\":4}}"}),
    )
    .await;
    assert!(!output.is_error, "{}", output.text);
    let sent = writes(&log);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].contains("dryRun=All"));
    assert!(!sent[1].contains("dryRun"));
    // The real patch carries the reviewed version as a precondition.
    assert_eq!(bodies(&log)[1]["metadata"]["resourceVersion"], "7");
    let seen = seen.lock();
    let doc = &seen[0].docs[0];
    assert!(doc.old.as_deref().unwrap().contains("replicas: 2"));
    assert!(doc.new.contains("replicas: 4"));
    // Server-set fields and status are not part of the diff.
    assert!(!doc.new.contains("resourceVersion") && !doc.new.contains("readyReplicas"));
}

#[tokio::test]
async fn deleting_shows_what_goes_away_and_secrets_cannot_be_patched() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Approved);
    let output = changes::run(
        &context,
        &approver,
        "delete_resource",
        &json!({"kind": "deploy", "name": "web", "namespace": "shop"}),
    )
    .await;
    assert!(!output.is_error, "{}", output.text);
    assert_eq!(
        writes(&log),
        ["DELETE /apis/apps/v1/namespaces/shop/deployments/web"]
    );
    let seen_now = seen.lock().len();
    assert_eq!(seen_now, 1);
    assert_eq!(seen.lock()[0].docs[0].new, "");
    assert!(
        seen.lock()[0].docs[0]
            .old
            .as_deref()
            .unwrap()
            .contains("web:1")
    );

    let before = log.lock().len();
    let secret = changes::run(
        &context,
        &approver,
        "patch_resource",
        &json!({"kind": "secret", "name": "db", "namespace": "shop", "patch": {"data": {"k": "dg=="}}}),
    )
    .await;
    assert!(secret.is_error);
    assert!(secret.text.contains("Secrets"));
    assert_eq!(log.lock().len(), before, "refused before any request");
    assert_eq!(seen.lock().len(), seen_now);
}

#[tokio::test]
async fn only_workloads_restart_and_only_scalable_kinds_scale() {
    let (client, _) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Approved);
    let restart = changes::run(
        &context,
        &approver,
        "restart_resource",
        &json!({"kind": "secret", "name": "x", "namespace": "shop"}),
    )
    .await;
    assert!(restart.is_error && restart.text.contains("Deployments, StatefulSets"));
    let scale = changes::run(
        &context,
        &approver,
        "scale_resource",
        &json!({"kind": "secret", "name": "x", "namespace": "shop", "replicas": 1}),
    )
    .await;
    assert!(scale.is_error && scale.text.contains("no scale subresource"));
    assert!(seen.lock().is_empty());
}

#[tokio::test]
async fn an_approved_change_is_dropped_when_the_cluster_went_read_only_meanwhile() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let (approver, seen) = approver(Decision::Approved);
    let output = changes::run_with(
        &context,
        &approver,
        "scale_resource",
        &json!({"kind": "deploy", "name": "web", "namespace": "shop", "replicas": 5}),
        &|| Err("kind-dev became read-only in Kubyl while the change waited.".into()),
    )
    .await;
    // The user was asked (the cluster was writable then), but nothing was sent afterwards.
    assert_eq!(seen.lock().len(), 1);
    assert!(output.is_error);
    assert!(output.text.contains("became read-only"), "{}", output.text);
    assert!(writes(&log).is_empty(), "{:?}", writes(&log));
}

/// Approves, after running `meanwhile` (what happens while the user decides).
struct ApproveAfter<F: Fn() + Send + Sync>(F);

impl<F: Fn() + Send + Sync> Approver for ApproveAfter<F> {
    fn approve(&self, _: Proposal) -> BoxFuture<'static, Decision> {
        (self.0)();
        Box::pin(async { Decision::Approved })
    }
}

const WEB: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: shop\nspec:\n  replicas: 4\n";
const API: &str = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\n  namespace: shop\nspec:\n  replicas: 1\n";

#[tokio::test]
async fn an_existing_object_is_applied_and_a_new_one_created_after_a_dry_run() {
    for (yaml, last) in [
        (WEB, "PATCH /apis/apps/v1/namespaces/shop/deployments/web"),
        (API, "POST /apis/apps/v1/namespaces/shop/deployments"),
    ] {
        let (client, log) = fake_api().await;
        let context = context(client);
        let (approver, seen) = approver(Decision::Approved);
        let output = changes::run(
            &context,
            &approver,
            "apply_manifest",
            &json!({"yaml": yaml}),
        )
        .await;
        assert!(!output.is_error, "{}", output.text);
        assert_eq!(seen.lock().len(), 1);
        let sent = writes(&log);
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent[0].contains("dryRun=All"));
        assert!(
            sent[1].starts_with(last) && !sent[1].contains("dryRun"),
            "{sent:?}"
        );
    }
}

#[tokio::test]
async fn an_object_created_by_someone_else_meanwhile_is_not_merged_into() {
    let (client, log) = fake_api().await;
    let context = context(client);
    let created = log.clone();
    let approver: Arc<dyn Approver> = Arc::new(ApproveAfter(move || {
        created.lock().push("CREATED api".into());
    }));
    let output = changes::run(&context, &approver, "apply_manifest", &json!({"yaml": API})).await;
    assert!(output.is_error);
    assert!(
        output.text.contains("created by someone else"),
        "{}",
        output.text
    );
    // The dry run, then a create the server refused: no apply that would merge into it.
    let sent: Vec<_> = writes(&log)
        .into_iter()
        .filter(|l| l != "CREATED api")
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].contains("dryRun=All"));
    assert!(sent[1].starts_with("POST "), "{sent:?}");
}

/// The handler's own last check, not just `run_with`'s plumbing: the cluster turns read-only,
/// or disappears, while the user decides.
#[tokio::test]
async fn the_handler_drops_a_change_when_its_cluster_went_read_only_or_away_meanwhile() {
    use kubyl_mcp_core::handler::{Scope, ToolsHandler, Writes};
    use kubyl_mcp_core::server::Handler as _;

    for vanish in [false, true] {
        let (client, log) = fake_api().await;
        let (clusters, receiver) = tokio::sync::watch::channel(Arc::new(vec![context(client)]));
        let (_enabled, enabled) = tokio::sync::watch::channel(true);
        let approver: Arc<dyn Approver> = Arc::new(ApproveAfter(move || {
            clusters.send_modify(|list| {
                let mut next = (**list).clone();
                if vanish {
                    next.clear();
                } else {
                    next[0].caps.read_only = true;
                }
                *list = Arc::new(next);
            });
        }));
        let handler = ToolsHandler::new(
            Scope::Clusters(receiver),
            Some(Writes {
                enabled,
                approver,
                off_message: "off".into(),
            }),
        );
        let output = handler
            .call(
                "scale_resource",
                json!({"cluster": "kind-dev", "kind": "deploy", "name": "web", "namespace": "shop", "replicas": 5}),
            )
            .await;
        assert!(output.is_error, "{}", output.text);
        let expected = if vanish {
            "no longer in"
        } else {
            "became read-only"
        };
        assert!(output.text.contains(expected), "{}", output.text);
        assert!(writes(&log).is_empty(), "{:?}", writes(&log));
    }
}
