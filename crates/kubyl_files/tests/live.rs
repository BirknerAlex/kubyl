//! File browsing and transfers against a real cluster. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_files --test live -- --ignored --nocapture
//! ```
//!
//! Creates (and deletes) the namespace `kubyl-live-files` with a busybox pod (ConfigMap,
//! Secret and emptyDir mounts) and a pod running only `pause` (no shell, like distroless).

use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod, Secret};
use kube::api::{Api, DeleteParams, PostParams};
use kubyl_core::ClusterId;
use kubyl_files::remote::{self, RemoteTarget};
use kubyl_files::transfer::{self, Direction, TransferJob, Verification};
use kubyl_files::{mounts, remote::DEBUG_ROOT};

const NAMESPACE: &str = "kubyl-live-files";

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

async fn create<K>(api: &Api<K>, value: serde_json::Value)
where
    K: kube::Resource + Clone + serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug,
{
    let object: K = serde_json::from_value(value).unwrap();
    api.create(&PostParams::default(), &object).await.ok();
}

async fn setup(client: &kube::Client) {
    create(
        &Api::<Namespace>::all(client.clone()),
        serde_json::json!({"metadata": {"name": NAMESPACE}}),
    )
    .await;
    create(
        &Api::<ConfigMap>::namespaced(client.clone(), NAMESPACE),
        serde_json::json!({"metadata": {"name": "app-config"}, "data": {"application.yaml": "port: 8080\n"}}),
    )
    .await;
    create(
        &Api::<Secret>::namespaced(client.clone(), NAMESPACE),
        serde_json::json!({"metadata": {"name": "app-secret"}, "stringData": {"password": "dummy"}}),
    )
    .await;
    let pods: Api<Pod> = Api::namespaced(client.clone(), NAMESPACE);
    create(
        &pods,
        serde_json::json!({
            "metadata": {"name": "box"},
            "spec": {
                "terminationGracePeriodSeconds": 0,
                "containers": [{
                    "name": "main", "image": "busybox:1.37", "command": ["sleep", "3600"],
                    "volumeMounts": [
                        {"name": "config", "mountPath": "/app/config"},
                        {"name": "secret", "mountPath": "/app/secrets"},
                        {"name": "data", "mountPath": "/data"}
                    ]
                }],
                "volumes": [
                    {"name": "config", "configMap": {"name": "app-config"}},
                    {"name": "secret", "secret": {"secretName": "app-secret"}},
                    {"name": "data", "emptyDir": {}}
                ]
            }
        }),
    )
    .await;
    create(
        &pods,
        serde_json::json!({
            "metadata": {"name": "distroless"},
            "spec": {
                "terminationGracePeriodSeconds": 0,
                "containers": [{"name": "app", "image": "registry.k8s.io/pause:3.10"}]
            }
        }),
    )
    .await;
    for (pod, container) in [("box", "main"), ("distroless", "app")] {
        kubyl_terminal::exec::wait_running(&pods, pod, container, Duration::from_secs(120))
            .await
            .expect("pods run");
    }
}

async fn teardown(client: &kube::Client) {
    Api::<Namespace>::all(client.clone())
        .delete(NAMESPACE, &DeleteParams::default())
        .await
        .ok();
}

async fn run(job: TransferJob) -> (anyhow::Result<Verification>, u64) {
    let (tx, mut rx) = mpsc::unbounded();
    let result = transfer::run(job, tx).await;
    let mut bytes = 0;
    while let Some(n) = rx.next().await {
        bytes += n;
    }
    (result, bytes)
}

fn job(
    target: &RemoteTarget,
    direction: Direction,
    remote: &str,
    local: std::path::PathBuf,
    name: &str,
    is_dir: bool,
    size: u64,
) -> TransferJob {
    TransferJob {
        direction,
        target: target.clone(),
        remote: remote.into(),
        local,
        dest_name: name.into(),
        is_dir,
        size,
        verify: true,
        chunk_size: 4 * 1024 * 1024,
    }
}

#[tokio::test]
#[ignore = "needs a cluster (script/dev-cluster.sh)"]
async fn browse_upload_download_and_distroless() {
    let client = client().await;
    setup(&client).await;
    let cluster = ClusterId::new("live");

    let target = remote::open(
        cluster.clone(),
        client.clone(),
        NAMESPACE.into(),
        "box".into(),
        "main".into(),
    )
    .await
    .expect("probe");
    assert!(
        target.caps.shell && target.caps.tar && target.caps.sha256sum,
        "{:?}",
        target.caps
    );
    println!(
        "busybox: {} via {:?}",
        target.caps.summary(),
        target.caps.listing
    );

    // Mounts from the pod spec: the Secret mount refuses writes.
    let pod = target.api().get("box").await.unwrap();
    let pod_mounts = mounts::mounts(&pod, "main");
    assert!(mounts::write_blocked(&pod_mounts, "/app/secrets").is_some());
    assert!(mounts::write_blocked(&pod_mounts, "/data").is_none());
    let listing = target.list("/app/config").await.expect("list config");
    assert!(
        listing.iter().any(|e| e.name == "application.yaml"),
        "{listing:?}"
    );

    // Upload a folder of 100 files, verified.
    let local = tempfile::tempdir().unwrap();
    let source = local.path().join("batch");
    std::fs::create_dir(&source).unwrap();
    for i in 0..100 {
        std::fs::write(
            source.join(format!("file-{i:03}.txt")),
            format!("line {i}\n").repeat(i + 1),
        )
        .unwrap();
    }
    let size = kubyl_files::local::tree_size(&source);
    let (result, _) = run(job(
        &target,
        Direction::Upload,
        "/data/in",
        source.clone(),
        "batch",
        true,
        size,
    ))
    .await;
    assert_eq!(result.expect("upload"), Verification::Verified);
    let uploaded = target.list("/data/in/batch").await.unwrap();
    assert_eq!(uploaded.len(), 100);

    // Download a 20 MB file in chunks, then resume a partial download.
    target
        .run(
            "dd if=/dev/urandom of=/data/big.bin bs=1048576 count=20 2>/dev/null",
            &[],
        )
        .await
        .unwrap();
    let size = 20 * 1024 * 1024;
    let out = tempfile::tempdir().unwrap();
    let (result, bytes) = run(job(
        &target,
        Direction::Download,
        "/data/big.bin",
        out.path().into(),
        "big.bin",
        false,
        size,
    ))
    .await;
    assert_eq!(result.expect("download"), Verification::Verified);
    assert_eq!(bytes, size);
    assert_eq!(
        std::fs::metadata(out.path().join("big.bin")).unwrap().len(),
        size
    );

    // A part file with two complete chunks and a torn third: the retry resumes at 8 MiB.
    let full = std::fs::read(out.path().join("big.bin")).unwrap();
    let part = out.path().join(".resumed.bin.kubyl-part");
    std::fs::write(&part, &full[..(8 * 1024 * 1024 + 1234)]).unwrap();
    let (result, bytes) = run(job(
        &target,
        Direction::Download,
        "/data/big.bin",
        out.path().into(),
        "resumed.bin",
        false,
        size,
    ))
    .await;
    assert_eq!(result.expect("resume"), Verification::Verified);
    assert_eq!(bytes, size, "resumed bytes count as progress");
    assert_eq!(std::fs::read(out.path().join("resumed.bin")).unwrap(), full);

    // Download the uploaded folder back as a tar stream.
    let (result, _) = run(job(
        &target,
        Direction::Download,
        "/data/in/batch",
        out.path().into(),
        "batch",
        true,
        0,
    ))
    .await;
    assert_eq!(result.expect("folder download"), Verification::Verified);
    assert_eq!(
        std::fs::read_dir(out.path().join("batch")).unwrap().count(),
        100
    );

    // Distroless: no shell, reached through a debug container.
    let bare = remote::open(
        cluster,
        client.clone(),
        NAMESPACE.into(),
        "distroless".into(),
        "app".into(),
    )
    .await
    .expect("probe distroless");
    assert!(!bare.caps.shell);
    let debug = remote::through_debug_container(&bare, "busybox:1.37")
        .await
        .expect("debug container");
    assert_eq!(debug.root, DEBUG_ROOT);
    let root = debug.list("/").await.expect("list through /proc/1/root");
    assert!(root.iter().any(|e| e.name == "pause"), "{root:?}");
    // Reuses the running debug container the second time.
    let again = remote::through_debug_container(&bare, "busybox:1.37")
        .await
        .unwrap();
    assert_eq!(again.exec_container, debug.exec_container);

    teardown(&client).await;
}
