//! Exec, debug containers and node shells against a real cluster. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_terminal --test live -- --ignored --nocapture --test-threads 1
//! ```
//!
//! Creates (and deletes) the namespace `kubyl-live-term` with one busybox pod; the node-shell
//! test creates a privileged pod in `kube-system` and deletes it.

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use k8s_openapi::api::core::v1::{Namespace, Pod};
use kube::api::{Api, DeleteParams, PostParams};
use kubyl_terminal::exec::{self, DebugSpec, ExecTarget, Mode};

const NAMESPACE: &str = "kubyl-live-term";

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

/// Creates the namespace and a running busybox pod; returns the pod's node.
async fn setup(client: &kube::Client) -> String {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    let ns: Namespace =
        serde_json::from_value(serde_json::json!({"metadata": {"name": NAMESPACE}})).unwrap();
    namespaces.create(&PostParams::default(), &ns).await.ok();
    let pods: Api<Pod> = Api::namespaced(client.clone(), NAMESPACE);
    let pod: Pod = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "box"},
        "spec": {
            "terminationGracePeriodSeconds": 0,
            "containers": [{"name": "main", "image": "busybox:1.37", "command": ["sleep", "3600"]}]
        }
    }))
    .unwrap();
    pods.create(&PostParams::default(), &pod).await.ok();
    exec::wait_running(&pods, "box", "main", Duration::from_secs(120))
        .await
        .expect("box runs");
    pods.get("box")
        .await
        .unwrap()
        .spec
        .unwrap()
        .node_name
        .unwrap()
}

async fn teardown(client: &kube::Client) {
    let namespaces: Api<Namespace> = Api::all(client.clone());
    namespaces
        .delete(NAMESPACE, &DeleteParams::default())
        .await
        .ok();
}

/// Runs a session, types `input` once connected and waits until the output contains `expect`.
async fn session_output(
    client: kube::Client,
    target: ExecTarget,
    size: (u16, u16),
    input: &str,
    expect: &str,
) -> String {
    let (input_tx, input_rx) = mpsc::unbounded();
    let (output_tx, mut output_rx) = mpsc::unbounded();
    let (resize_tx, resize_rx) = mpsc::unbounded();
    let (connected_tx, connected_rx) = oneshot::channel();
    resize_tx.unbounded_send(size).unwrap();
    let task = tokio::spawn(exec::run(
        client,
        target,
        input_rx,
        output_tx,
        resize_rx,
        connected_tx,
    ));
    connected_rx.await.unwrap().expect("connected");
    // Give the shell a moment to start and apply the size.
    tokio::time::sleep(Duration::from_millis(500)).await;
    input_tx.unbounded_send(input.as_bytes().to_vec()).unwrap();
    let mut output = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !output.contains(expect) && tokio::time::Instant::now() < deadline {
        if let Ok(Some(bytes)) =
            tokio::time::timeout(Duration::from_secs(1), output_rx.next()).await
        {
            output.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    drop(input_tx);
    task.abort();
    output
}

#[tokio::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
async fn exec_debug_container_and_node_shell() {
    let client = client().await;
    let node = setup(&client).await;

    // Exec: the TTY gets the size we send.
    let output = session_output(
        client.clone(),
        ExecTarget {
            namespace: NAMESPACE.into(),
            pod: "box".into(),
            container: Some("main".into()),
            mode: Mode::Exec {
                command: vec!["sh".into()],
            },
        },
        (100, 30),
        "stty size\r",
        "30 100",
    )
    .await;
    assert!(output.contains("30 100"), "stty size: {output:?}");

    // Debug container sharing main's processes: /proc/1/root is main's filesystem.
    let debugger = exec::create_debug_container(
        &client,
        NAMESPACE,
        "box",
        &DebugSpec {
            image: "busybox:1.37".into(),
            target: Some("main".into()),
            command: vec!["sh".into()],
        },
    )
    .await
    .expect("debug container");
    let output = session_output(
        client.clone(),
        ExecTarget {
            namespace: NAMESPACE.into(),
            pod: "box".into(),
            container: Some(debugger.clone()),
            mode: Mode::Attach {
                tty: true,
                stdin: true,
            },
        },
        (80, 24),
        "echo debug-$((40+2)); ps | grep -c 'sleep 3600'\r",
        "debug-42",
    )
    .await;
    assert!(output.contains("debug-42"), "debug container: {output:?}");

    // Node shell: a privileged pod on the node, nsenter into the host.
    let shell_pod = exec::create_node_shell(&client, "kube-system", &node, "busybox:1.37")
        .await
        .expect("node shell pod");
    let shell_pod_name = shell_pod.name().to_string();
    let output = session_output(
        client.clone(),
        ExecTarget {
            namespace: "kube-system".into(),
            pod: shell_pod_name.clone(),
            container: Some("shell".into()),
            mode: Mode::Exec {
                command: exec::node_shell_command(),
            },
        },
        (80, 24),
        "cat /etc/hostname\r",
        &node,
    )
    .await;
    // Dropping the guard deletes the pod; wait until it's gone.
    drop(shell_pod);
    let pods: Api<Pod> = Api::namespaced(client.clone(), "kube-system");
    for _ in 0..30 {
        if pods
            .get_opt(&shell_pod_name)
            .await
            .ok()
            .flatten()
            .is_none_or(|p| p.metadata.deletion_timestamp.is_some())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let left = pods.get_opt(&shell_pod_name).await.ok().flatten();
    assert!(
        left.is_none_or(|p| p.metadata.deletion_timestamp.is_some()),
        "the node-shell pod is deleted with its guard"
    );
    teardown(&client).await;
    assert!(output.contains(&node), "node shell on {node}: {output:?}");
}
