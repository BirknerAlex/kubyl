//! Argo CD against a real cluster. Ignored by default; run against the kind dev cluster with
//! Argo CD installed:
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBECONFIG=/tmp/kubyl-dev/kubeconfig script/argocd-dev.sh           # or v3.4.9
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
//!   cargo test -p kubyl_argocd --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Uses the sample apps of `script/argocd-dev.sh` (`argocd/guestbook`, manual sync, two
//! history entries) and creates throwaway apps `kubyl-live-*` for the delete test. API mode
//! signs in as `admin` with the password from `argocd-initial-admin-secret` (the test reads it;
//! Kubyl never reads Secrets). The UI tests (the Argo CD section appears and disappears with the
//! CRDs) are in `tests/live_ui.rs`.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use kube::api::{Api, DeleteParams, DynamicObject, Patch, PatchParams, PostParams};
use kube::discovery::ApiResource;
use kubyl_argocd::api::{ArgoApi, Transport};
use kubyl_argocd::detect;
use kubyl_argocd::model::{Application, Health, SyncStatus, short_revision};
use kubyl_argocd::ops::{self, AppTarget, Cascade, PolicyChange, SyncRequest};
use secrecy::SecretString;
use serde_json::{Value, json};

const NAMESPACE: &str = "argocd";
const OLD_REVISION: &str = "68657670d9131dc5bc5f538b14c1de3377d74591";

async fn client() -> kube::Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let kubeconfig = kube::config::Kubeconfig::read_from(path).expect("kubeconfig");
    let options = kube::config::KubeConfigOptions {
        context: Some(std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())),
        ..Default::default()
    };
    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .expect("config");
    kube::Client::try_from(config).expect("client")
}

fn applications() -> ApiResource {
    ApiResource {
        group: "argoproj.io".into(),
        version: "v1alpha1".into(),
        api_version: "argoproj.io/v1alpha1".into(),
        kind: "Application".into(),
        plural: "applications".into(),
    }
}

async fn app(client: &kube::Client, namespace: &str, name: &str) -> Value {
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &applications());
    serde_json::to_value(api.get(name).await.expect("app")).unwrap()
}

/// Polls `check` on the app until it returns true or `timeout` passes.
async fn wait_app(
    client: &kube::Client,
    target: &AppTarget,
    timeout: Duration,
    what: &str,
    check: impl Fn(&Value) -> bool,
) -> Value {
    let start = Instant::now();
    loop {
        let value = app(client, &target.namespace, &target.name).await;
        if check(&value) {
            println!("{what}: after {:?}", start.elapsed());
            return value;
        }
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The last operation finished and nothing is requested.
fn operation_done(app: &Value) -> bool {
    app.get("operation").is_none_or(Value::is_null)
        && matches!(
            app.pointer("/status/operationState/phase")
                .and_then(Value::as_str),
            Some("Succeeded" | "Failed" | "Error")
        )
}

async fn sync_and_wait(client: &kube::Client, target: &AppTarget, revision: Option<&str>) -> Value {
    let request = SyncRequest {
        revisions: vec![revision.map(String::from)],
        ..Default::default()
    };
    ops::start_operation(client.clone(), applications(), target.clone(), |app| {
        ops::sync_operation(app, &request, "kubyl-live-test")
    })
    .await
    .expect("sync");
    let done = wait_app(
        client,
        target,
        Duration::from_secs(180),
        "sync",
        operation_done,
    )
    .await;
    assert_eq!(
        done.pointer("/status/operationState/phase")
            .and_then(Value::as_str),
        Some("Succeeded"),
        "{:?}",
        done.pointer("/status/operationState/message")
    );
    done
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn detects_the_install() {
    let client = client().await;
    let installs = detect::detect(client, vec![NAMESPACE.into()])
        .await
        .unwrap();
    println!("{installs:#?}");
    let install = installs
        .iter()
        .find(|i| i.namespace == NAMESPACE)
        .expect("argocd install");
    let server = install.server.as_ref().expect("argocd-server");
    assert_eq!(server.name, "argocd-server");
    assert_eq!(server.https_port, Some(443));
    assert!(!server.uid.is_empty());
    assert_eq!(
        install.controller.as_ref().unwrap().name,
        "argocd-application-controller"
    );
    assert!(install.version.as_deref().unwrap().starts_with("v3."));
    assert_eq!(install.app_namespaces, ["argocd-apps"]);
    assert!(install.manages_namespace("argocd-apps"));
    assert!(!install.partial);
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn refresh_sync_and_rollback_in_kubernetes_mode() {
    let client = client().await;
    let target = AppTarget::new(NAMESPACE, "guestbook");
    // The sample app is manual; rollback needs that.
    ops::set_policy(
        client.clone(),
        applications(),
        target.clone(),
        PolicyChange::AutoSync(false),
    )
    .await
    .unwrap();

    // Refresh: the annotation is set, then removed by the controller once it compared.
    ops::refresh(client.clone(), applications(), target.clone(), false)
        .await
        .unwrap();
    wait_app(
        &client,
        &target,
        Duration::from_secs(60),
        "refresh",
        |app| {
            app.pointer("/metadata/annotations/argocd.argoproj.io~1refresh")
                .is_none()
        },
    )
    .await;

    // Sync to HEAD.
    let synced = sync_and_wait(&client, &target, None).await;
    let parsed = Application::parse(&synced).unwrap();
    let head = parsed
        .status
        .operation_state
        .as_ref()
        .unwrap()
        .revision()
        .unwrap();
    println!("synced to {}", short_revision(&head));
    assert_eq!(parsed.sync(), SyncStatus::Synced);
    assert!(parsed.status.history.len() >= 2);

    // A second operation while one is requested is refused, like the API server does.
    let request = SyncRequest::default();
    ops::start_operation(client.clone(), applications(), target.clone(), |app| {
        ops::sync_operation(app, &request, "kubyl-live-test")
    })
    .await
    .unwrap();
    let again = ops::start_operation(client.clone(), applications(), target.clone(), |app| {
        ops::sync_operation(app, &request, "kubyl-live-test")
    })
    .await;
    // The controller may have finished the first one already.
    if let Err(err) = again {
        assert_eq!(err, ops::OpError::AnotherOperation);
    }
    wait_app(
        &client,
        &target,
        Duration::from_secs(120),
        "second sync",
        operation_done,
    )
    .await;

    // Roll back to the entry with the old revision.
    let current = Application::parse(&app(&client, NAMESPACE, "guestbook").await).unwrap();
    let entry = current
        .history_newest_first()
        .into_iter()
        .find(|h| h.revision == OLD_REVISION)
        .expect("a history entry with the old revision (script/argocd-dev.sh syncs it first)");
    ops::start_operation(client.clone(), applications(), target.clone(), |app| {
        ops::rollback_operation(app, entry.id, false, false, "kubyl-live-test")
    })
    .await
    .unwrap();
    let rolled = wait_app(
        &client,
        &target,
        Duration::from_secs(180),
        "rollback",
        operation_done,
    )
    .await;
    let rolled = Application::parse(&rolled).unwrap();
    assert_eq!(
        rolled.status.operation_state.as_ref().unwrap().phase(),
        Some(kubyl_argocd::model::OperationPhase::Succeeded)
    );
    // The deployed revision (`status.sync.revision` is the one compared against: the target).
    assert_eq!(
        rolled
            .status
            .operation_state
            .as_ref()
            .unwrap()
            .revision()
            .as_deref(),
        Some(OLD_REVISION)
    );
    // HEAD's guestbook uses another image: once compared again, the app is out of sync.
    wait_app(
        &client,
        &target,
        Duration::from_secs(60),
        "OutOfSync",
        |app| app.pointer("/status/sync/status").and_then(Value::as_str) == Some("OutOfSync"),
    )
    .await;
    let newest = rolled.history_newest_first()[0].clone();
    assert_eq!(newest.revision, OLD_REVISION);
    assert_eq!(
        newest.initiated_by.unwrap().username.as_deref(),
        Some("kubyl-live-test")
    );
    // Back to HEAD for the next run.
    sync_and_wait(&client, &target, None).await;
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn auto_sync_policy_round_trip() {
    let client = client().await;
    let target = AppTarget::new(NAMESPACE, "guestbook");
    for change in [
        PolicyChange::AutoSync(true),
        PolicyChange::Prune(true),
        PolicyChange::SelfHeal(true),
    ] {
        ops::set_policy(client.clone(), applications(), target.clone(), change)
            .await
            .unwrap();
    }
    let parsed = Application::parse(&app(&client, NAMESPACE, "guestbook").await).unwrap();
    let policy = parsed.policy();
    assert!(policy.auto_sync() && policy.prune() && policy.self_heal());
    // Rollback refuses while auto-sync is on.
    let id = parsed.status.history[0].id;
    let refused = ops::start_operation(client.clone(), applications(), target.clone(), |app| {
        ops::rollback_operation(app, id, false, false, "kubyl-live-test")
    })
    .await;
    assert_eq!(refused, Err(ops::OpError::AutoSyncOn));
    ops::set_policy(
        client.clone(),
        applications(),
        target.clone(),
        PolicyChange::AutoSync(false),
    )
    .await
    .unwrap();
    let parsed = Application::parse(&app(&client, NAMESPACE, "guestbook").await).unwrap();
    assert!(!parsed.policy().auto_sync());
}

async fn admin_password(client: &kube::Client) -> SecretString {
    let secrets: Api<Secret> = Api::namespaced(client.clone(), NAMESPACE);
    let secret = secrets
        .get("argocd-initial-admin-secret")
        .await
        .expect("admin secret");
    let bytes = secret.data.unwrap()["password"].0.clone();
    SecretString::from(String::from_utf8(bytes).unwrap())
}

struct PortForward(Child);

impl Drop for PortForward {
    fn drop(&mut self) {
        self.0.kill().ok();
    }
}

/// `kubectl port-forward` to argocd-server (the app uses kubyl_portforward; this checks the
/// transport).
fn kubectl_forward(port: u16) -> PortForward {
    let kubeconfig = std::env::var("KUBYL_TEST_KUBECONFIG").unwrap();
    let child = Command::new("kubectl")
        .args([
            "--kubeconfig",
            &kubeconfig,
            "-n",
            NAMESPACE,
            "port-forward",
            "svc/argocd-server",
            &format!("{port}:443"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("kubectl");
    std::thread::sleep(Duration::from_secs(2));
    PortForward(child)
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn api_mode_signs_in_through_the_proxy_and_a_forward() {
    let client = client().await;
    let password = admin_password(&client).await;
    let proxy = ArgoApi::new(
        Transport::proxy(client.clone(), NAMESPACE, "argocd-server", 443, true, ""),
        None,
    );
    assert!(proxy.version().await.unwrap().starts_with("v3."));
    // Without a token, Argo CD says so.
    assert_eq!(
        proxy.user_info().await,
        Err(kubyl_argocd::api::ApiError::Unauthorized)
    );
    let wrong = proxy
        .login("admin", &SecretString::from("wrong".to_string()))
        .await;
    assert!(matches!(
        wrong,
        Err(kubyl_argocd::api::ApiError::Forbidden(_))
    ));
    let token = proxy.login("admin", &password).await.unwrap();
    let proxy = proxy.with_token(token.clone());
    let me = proxy.user_info().await.unwrap();
    assert_eq!(me.username, "admin");

    let _forward = kubectl_forward(18443);
    let forward = ArgoApi::new(Transport::forward(18443, true, "").unwrap(), Some(token));
    assert_eq!(forward.user_info().await.unwrap().username, "admin");
    let tree = forward
        .resource_tree(&AppTarget::new(NAMESPACE, "guestbook"))
        .await
        .unwrap();
    // The full tree includes the pods (children of the managed Deployment).
    let pods: Vec<_> = tree.nodes.iter().filter(|n| n.node.kind == "Pod").collect();
    println!("tree: {} nodes, {} pods", tree.nodes.len(), pods.len());
    assert!(!pods.is_empty());
    assert!(pods.iter().all(|p| !p.parent_refs.is_empty()));

    // Signing out revokes the token on the server: copies of it (a web view's cookie) stop
    // working through either transport.
    forward.logout().await.unwrap();
    assert_eq!(
        proxy.user_info().await,
        Err(kubyl_argocd::api::ApiError::Unauthorized)
    );
    assert_eq!(
        forward.user_info().await,
        Err(kubyl_argocd::api::ApiError::Unauthorized)
    );
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn api_mode_diff_after_an_out_of_band_edit() {
    let client = client().await;
    let password = admin_password(&client).await;
    let api = ArgoApi::new(
        Transport::proxy(client.clone(), NAMESPACE, "argocd-server", 443, true, ""),
        None,
    );
    let api = api.with_token(api.login("admin", &password).await.unwrap());
    let target = AppTarget::new(NAMESPACE, "guestbook");

    // Start from a synced app (another test may have rolled it back).
    api.sync(&target, &SyncRequest::default(), false)
        .await
        .unwrap();
    wait_app(
        &client,
        &target,
        Duration::from_secs(120),
        "Synced",
        |app| {
            operation_done(app)
                && app.pointer("/status/sync/status").and_then(Value::as_str) == Some("Synced")
        },
    )
    .await;

    // What `kubectl edit` would do: change the replicas behind Argo CD's back.
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), "guestbook");
    deployments
        .patch(
            "guestbook-ui",
            &PatchParams::default(),
            &Patch::Merge(json!({"spec": {"replicas": 3}})),
        )
        .await
        .unwrap();
    api.refresh(&target, false).await.unwrap();
    wait_app(
        &client,
        &target,
        Duration::from_secs(60),
        "OutOfSync",
        |app| app.pointer("/status/sync/status").and_then(Value::as_str) == Some("OutOfSync"),
    )
    .await;

    let items = api.managed_resources(&target).await.unwrap();
    let deployment = items
        .iter()
        .find(|i| i.kind == "Deployment" && i.name == "guestbook-ui")
        .expect("the deployment");
    let (live, desired) = (deployment.live().unwrap(), deployment.desired().unwrap());
    assert_eq!(live.pointer("/spec/replicas"), Some(&json!(3)));
    assert_eq!(desired.pointer("/spec/replicas"), Some(&json!(1)));
    let diff = kubyl_argocd::diff::resource_diff(deployment);
    println!("{:?}", diff.summary);
    assert!(diff.summary.iter().any(|s| s.contains("spec.replicas")));
    assert!(!diff.is_empty());

    // Sync through the API puts it back.
    api.sync(&target, &SyncRequest::default(), false)
        .await
        .unwrap();
    wait_app(
        &client,
        &target,
        Duration::from_secs(120),
        "API sync",
        |app| {
            operation_done(app)
                && app.pointer("/status/sync/status").and_then(Value::as_str) == Some("Synced")
        },
    )
    .await;
    let replicas = deployments
        .get("guestbook-ui")
        .await
        .unwrap()
        .spec
        .unwrap()
        .replicas;
    assert_eq!(replicas, Some(1));
}

async fn throwaway_app(client: &kube::Client, name: &str) -> AppTarget {
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), NAMESPACE, &applications());
    let object: DynamicObject = serde_json::from_value(json!({
        "apiVersion": "argoproj.io/v1alpha1",
        "kind": "Application",
        "metadata": {"name": name, "namespace": NAMESPACE},
        "spec": {
            "project": "default",
            "source": {"repoURL": "https://github.com/argoproj/argocd-example-apps.git",
                       "path": "guestbook", "targetRevision": "HEAD"},
            "destination": {"server": "https://kubernetes.default.svc", "namespace": name},
            "syncPolicy": {"syncOptions": ["CreateNamespace=true"]}
        }
    }))
    .unwrap();
    api.create(&PostParams::default(), &object).await.unwrap();
    let target = AppTarget::new(NAMESPACE, name);
    sync_and_wait(client, &target, None).await;
    target
}

async fn deployment_exists(client: &kube::Client, namespace: &str) -> bool {
    let deployments: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    deployments.get_opt("guestbook-ui").await.unwrap().is_some()
}

async fn wait_gone(client: &kube::Client, target: &AppTarget) {
    let api: Api<DynamicObject> =
        Api::namespaced_with(client.clone(), &target.namespace, &applications());
    let start = Instant::now();
    while api.get_opt(&target.name).await.unwrap().is_some() {
        assert!(
            start.elapsed() < Duration::from_secs(120),
            "app not deleted"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn delete_cascading_and_non_cascading() {
    let client = client().await;
    for name in ["kubyl-live-cascade", "kubyl-live-keep"] {
        // Leftovers of an aborted run.
        let api: Api<DynamicObject> =
            Api::namespaced_with(client.clone(), NAMESPACE, &applications());
        if api.get_opt(name).await.unwrap().is_some() {
            ops::delete(
                client.clone(),
                applications(),
                AppTarget::new(NAMESPACE, name),
                Cascade::Foreground,
            )
            .await
            .unwrap();
            wait_gone(&client, &AppTarget::new(NAMESPACE, name)).await;
        }
    }
    let cascade = throwaway_app(&client, "kubyl-live-cascade").await;
    let keep = throwaway_app(&client, "kubyl-live-keep").await;
    assert!(deployment_exists(&client, "kubyl-live-cascade").await);
    assert!(deployment_exists(&client, "kubyl-live-keep").await);

    ops::delete(
        client.clone(),
        applications(),
        cascade.clone(),
        Cascade::Foreground,
    )
    .await
    .unwrap();
    ops::delete(client.clone(), applications(), keep.clone(), Cascade::None)
        .await
        .unwrap();
    wait_gone(&client, &cascade).await;
    wait_gone(&client, &keep).await;
    // Cascading: the finalizer deleted the resources first.
    assert!(!deployment_exists(&client, "kubyl-live-cascade").await);
    // Non-cascading: the resources stay.
    assert!(deployment_exists(&client, "kubyl-live-keep").await);

    let namespaces: Api<Namespace> = Api::all(client.clone());
    for name in ["kubyl-live-cascade", "kubyl-live-keep"] {
        namespaces.delete(name, &DeleteParams::default()).await.ok();
    }
}

#[tokio::test]
#[ignore = "needs Argo CD on kind (script/argocd-dev.sh)"]
async fn health_of_live_objects_matches_argo_cd() {
    let client = client().await;
    let app = Application::parse(&app(&client, NAMESPACE, "guestbook").await).unwrap();
    let deployments: Api<DynamicObject> = Api::namespaced_with(
        client.clone(),
        "guestbook",
        &ApiResource::erase::<Deployment>(&()),
    );
    let live = serde_json::to_value(deployments.get("guestbook-ui").await.unwrap()).unwrap();
    let (health, _) = kubyl_argocd::health::assess("apps", "Deployment", &live).unwrap();
    // The app's own health aggregates its resources'.
    if app.health() == Health::Healthy {
        assert_eq!(health, Health::Healthy);
    }
    println!("app {} · deployment {health}", app.health());
}
