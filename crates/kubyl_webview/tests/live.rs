//! Web-view forwards against a real cluster. Ignored by default; run against the kind dev
//! cluster:
//!
//! ```sh
//! script/dev-cluster.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_webview --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Creates (and deletes) the namespace `kubyl-live-web` with a one-replica nginx Deployment
//! and a Service, then drives [`WebForwards`] the way tabs do, on phase 05's port-forward
//! manager: a tab opening and closing, two tabs sharing a forward, a pod being killed. Cookie
//! isolation between clusters needs real web views: see `tests/live_webview.rs`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyWeakEntity, App, AppContext as _, Entity, TestAppContext};
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use kube::api::{Api, DeleteParams, ListParams, PostParams};
use kubyl_core::ClusterId;
use kubyl_logs::sessions::{SessionKind, SessionRegistry};
use kubyl_portforward::favorites::SavedForwards;
use kubyl_portforward::manager::{ForwardId, ForwardInfo, PortForwardManager};
use kubyl_webview::forward::{ForwardBackend, ForwardStatus, WebForwards, spec};
use kubyl_webview::target::{Scheme, TargetKind, WebTarget};

const NAMESPACE: &str = "kubyl-live-web";

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    kubyl_core::runtime::runtime().block_on(future)
}

fn client() -> kube::Client {
    block_on(async {
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
    })
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

fn setup(client: &kube::Client) {
    block_on(async {
        let namespaces: Api<Namespace> = Api::all(client.clone());
        let ns: Namespace =
            serde_json::from_value(serde_json::json!({"metadata": {"name": NAMESPACE}})).unwrap();
        namespaces.create(&PostParams::default(), &ns).await.ok();
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
        Api::<Deployment>::namespaced(client.clone(), NAMESPACE)
            .create(&PostParams::default(), &deployment)
            .await
            .ok();
        let service: Service = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "web"},
            "spec": {"selector": {"app": "web"}, "ports": [{"name": "http", "port": 80, "targetPort": "http"}]}
        }))
        .unwrap();
        Api::<Service>::namespaced(client.clone(), NAMESPACE)
            .create(&PostParams::default(), &service)
            .await
            .ok();
        for _ in 0..120 {
            if !ready_pods(client).await.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("nginx didn't get ready");
    });
}

fn teardown(client: &kube::Client) {
    block_on(async {
        let namespaces = Api::<Namespace>::all(client.clone());
        namespaces
            .delete(NAMESPACE, &DeleteParams::default())
            .await
            .ok();
        // The next test's setup can't create anything in a terminating namespace.
        for _ in 0..120 {
            if matches!(namespaces.get_opt(NAMESPACE).await, Ok(None)) {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("namespace {NAMESPACE} wasn't deleted");
    });
}

/// One HTTP GET to the local port; the status line, or `None` when nothing answers.
fn get(port: u16) -> Option<String> {
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: web\r\n\r\n")
        .ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    let text = String::from_utf8_lossy(&response);
    text.lines().next().map(str::to_string)
}

/// Forwards of phase 05's manager with the test's client (the app gets it from the
/// connection manager).
struct LiveBackend {
    client: kube::Client,
    ids: RefCell<HashMap<u64, ForwardId>>,
    next: Cell<u64>,
}

impl ForwardBackend for LiveBackend {
    fn start(&self, target: &WebTarget, https: bool, cx: &mut App) -> Result<u64, String> {
        let forward =
            kubyl_portforward::start_forward(self.client.clone(), spec(target, https, 0), cx);
        let id = self.next.get() + 1;
        self.next.set(id);
        self.ids.borrow_mut().insert(id, forward);
        Ok(id)
    }

    fn stop(&self, id: u64, cx: &mut App) {
        if let Some(forward) = self.ids.borrow_mut().remove(&id) {
            PortForwardManager::global(cx).update(cx, |m, cx| m.stop(forward, cx));
        }
    }

    fn info(&self, id: u64, cx: &App) -> Option<ForwardInfo> {
        let forward = *self.ids.borrow().get(&id)?;
        PortForwardManager::global(cx).read(cx).info(forward)
    }
}

/// Stands in for a web-view tab.
struct Tab;

struct Fixture {
    forwards: Entity<WebForwards>,
    client: kube::Client,
    _dir: tempfile::TempDir,
}

fn fixture(cx: &mut TestAppContext) -> Fixture {
    cx.executor().allow_parking();
    let client = client();
    setup(&client);
    let dir = tempfile::tempdir().unwrap();
    let forwards = cx.update(|cx| {
        kubyl_core::init(cx);
        kubyl_settings::init_with_dir(cx, dir.path());
        SessionRegistry::install(cx);
        PortForwardManager::install(cx);
        SavedForwards::install(cx);
        WebForwards::install_with(
            Rc::new(LiveBackend {
                client: client.clone(),
                ids: RefCell::default(),
                next: Cell::new(0),
            }),
            cx,
        )
    });
    Fixture {
        forwards,
        client,
        _dir: dir,
    }
}

fn target() -> WebTarget {
    WebTarget {
        cluster: ClusterId::new("kind-kubyl-dev"),
        namespace: NAMESPACE.into(),
        kind: TargetKind::Service,
        name: "web".into(),
        port: 80,
    }
}

fn open_tab(fixture: &Fixture, cx: &mut TestAppContext) -> Entity<Tab> {
    let tab = cx.update(|cx| cx.new(|_| Tab));
    let weak: AnyWeakEntity = tab.downgrade().into();
    fixture
        .forwards
        .update(cx, |f, cx| f.hold(&target(), weak, true, Scheme::Http, cx));
    tab
}

fn close_tab(fixture: &Fixture, tab: Entity<Tab>, cx: &mut TestAppContext) {
    let id = tab.entity_id();
    fixture
        .forwards
        .update(cx, |f, cx| f.release(&target(), id, cx));
    drop(tab);
}

/// Runs the app until `f` answers (the forward runs on Tokio; its events arrive async).
fn wait_for<T>(
    cx: &mut TestAppContext,
    what: &str,
    mut f: impl FnMut(&mut TestAppContext) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        // GPUI timers (the manager batches events every 100 ms) run on the test clock.
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.run_until_parked();
        if let Some(value) = f(cx) {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn local_port(fixture: &Fixture, cx: &mut TestAppContext) -> u16 {
    wait_for(cx, "the forward to listen", |cx| {
        fixture
            .forwards
            .read_with(cx, |f, cx| match f.status(&target(), cx) {
                Some(ForwardStatus::Ready { local_port, .. }) => Some(local_port),
                _ => None,
            })
    })
}

fn web_sessions(cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        SessionRegistry::global(cx)
            .read(cx)
            .all()
            .iter()
            .filter(|s| s.kind == SessionKind::PortForward)
            .map(|s| s.title.to_string())
            .collect()
    })
}

#[gpui::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
fn open_and_close_stops_the_forward(cx: &mut TestAppContext) {
    let fixture = fixture(cx);
    let tab = open_tab(&fixture, cx);
    let port = local_port(&fixture, cx);
    assert_eq!(get(port).as_deref(), Some("HTTP/1.1 200 OK"));
    let sessions = wait_for(cx, "the session row", |cx| {
        let sessions = web_sessions(cx);
        sessions
            .iter()
            .any(|s| s.starts_with("web view · svc/web:80"))
            .then_some(sessions)
    });
    println!("Active sessions: {sessions:?}");
    assert!(
        cx.update(|cx| kubyl_core::forwards::ActiveForwards::all(cx).is_empty()),
        "web-view forwards aren't listed as regular (saveable) forwards"
    );

    close_tab(&fixture, tab, cx);
    wait_for(cx, "the forward to stop", |cx| {
        web_sessions(cx).is_empty().then_some(())
    });
    assert_eq!(get(port), None, "the local port is closed");
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(),
        "the local port is free again"
    );
    teardown(&fixture.client);
}

#[gpui::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
fn two_tabs_share_one_forward(cx: &mut TestAppContext) {
    let fixture = fixture(cx);
    let first = open_tab(&fixture, cx);
    let port = local_port(&fixture, cx);
    let second = open_tab(&fixture, cx);
    assert_eq!(
        local_port(&fixture, cx),
        port,
        "the second tab uses the same port"
    );
    fixture
        .forwards
        .read_with(cx, |f, _| assert_eq!(f.tab_count(&target()), 2));
    wait_for(cx, "one session row", |cx| {
        (web_sessions(cx).len() == 1).then_some(())
    });

    close_tab(&fixture, first, cx);
    cx.run_until_parked();
    assert_eq!(
        get(port).as_deref(),
        Some("HTTP/1.1 200 OK"),
        "closing one tab keeps the other working"
    );
    assert_eq!(web_sessions(cx).len(), 1);

    close_tab(&fixture, second, cx);
    wait_for(cx, "the forward to stop", |cx| {
        web_sessions(cx).is_empty().then_some(())
    });
    assert_eq!(get(port), None);
    teardown(&fixture.client);
}

#[gpui::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
fn killing_the_pod_reconnects(cx: &mut TestAppContext) {
    let fixture = fixture(cx);
    let _tab = open_tab(&fixture, cx);
    let port = local_port(&fixture, cx);
    assert_eq!(get(port).as_deref(), Some("HTTP/1.1 200 OK"));

    let old = block_on(ready_pods(&fixture.client));
    block_on(async {
        let pods: Api<Pod> = Api::namespaced(fixture.client.clone(), NAMESPACE);
        for pod in &old {
            pods.delete(pod, &DeleteParams::default().grace_period(0))
                .await
                .unwrap();
        }
    });
    let started = Instant::now();
    let status = wait_for(cx, "the replacement pod to answer", |_| {
        let ready = block_on(ready_pods(&fixture.client));
        if !ready.iter().any(|p| !old.contains(p)) {
            return None;
        }
        get(port)
    });
    println!("answered again after {:?}", started.elapsed());
    assert_eq!(
        status, "HTTP/1.1 200 OK",
        "the same local port reaches the new pod"
    );
    let pod = wait_for(cx, "the forward to report the new pod", |cx| {
        fixture
            .forwards
            .read_with(cx, |f, cx| match f.status(&target(), cx) {
                Some(ForwardStatus::Ready { info, .. }) => info.pod,
                _ => None,
            })
            .filter(|pod| !old.contains(pod))
    });
    println!("now via {pod}");
    teardown(&fixture.client);
}
