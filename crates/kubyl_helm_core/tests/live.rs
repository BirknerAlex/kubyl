//! Live tests with the real `helm` against the kind cluster, after `script/helm-dev.sh`.
//! Ignored by default. Run them with:
//!
//! ```sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_helm_core --test live -- --ignored --test-threads=1
//! ```
//!
//! `KUBYL_TEST_CONTEXT` (default `kind-kubyl-dev`) picks the context. Use the same
//! `HELM_REPOSITORY_CONFIG`/`HELM_REPOSITORY_CACHE`/`HELM_REGISTRY_CONFIG` as the script. The
//! tests own the namespace `kubyl-helm-live` (labelled `kubyl.dev/owner=helm-live`) and delete
//! it at the end.

use std::path::PathBuf;
use std::time::Duration;

use kubyl_helm_core::cli::{self, HelmInfo, Probe};
use kubyl_helm_core::cmd::{
    self, InstallSpec, Mode, RollbackSpec, UninstallSpec, UpgradeSpec, ValuesMode, WriteOptions,
};
use kubyl_helm_core::decode::Driver;
use kubyl_helm_core::preview::{self, Change};
use kubyl_helm_core::release;
use kubyl_helm_core::repo::{self, ChartRef};
use kubyl_kube_core::cli::CliTarget;

const NS: &str = "kubyl-helm-live";

fn kubeconfig() -> PathBuf {
    std::env::var_os("KUBYL_TEST_KUBECONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/tmp/kubyl-dev/kubeconfig".into())
}

fn context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or_else(|_| "kind-kubyl-dev".into())
}

fn target() -> CliTarget {
    CliTarget::new(kubeconfig(), context())
}

async fn helm() -> HelmInfo {
    let path = std::env::var_os("PATH");
    match cli::probe(None, path).await {
        Probe::Ready(info) => info,
        other => panic!("no usable helm: {other:?}"),
    }
}

async fn client() -> kube::Client {
    let config = kube::config::Kubeconfig::read_from(kubeconfig()).unwrap();
    let options = kube::config::KubeConfigOptions {
        context: Some(context()),
        ..Default::default()
    };
    let config = kube::Config::from_custom_kubeconfig(config, &options)
        .await
        .unwrap();
    kube::Client::try_from(config).unwrap()
}

fn demo() -> ChartRef {
    ChartRef::Repo {
        repo: "kubyl-dev".into(),
        name: "kubyl-demo".into(),
    }
}

async fn run(helm: &HelmInfo, invocation: cli::Invocation) -> cli::Output {
    match cli::run(helm, Some(&target()), invocation, None, None).await {
        Ok(output) => output,
        Err(err) => panic!("{}: {}", err.message, err.detail),
    }
}

async fn ensure_namespace(helm: &HelmInfo) {
    // `kubectl` isn't a dependency: let an install create the namespace, then label it.
    let _ = helm;
    let client = client().await;
    let api: kube::Api<k8s_openapi::api::core::v1::Namespace> = kube::Api::all(client);
    // A previous run's cleanup may still be deleting it.
    for _ in 0..90 {
        match api.get(NS).await {
            Ok(ns) if ns.metadata.deletion_timestamp.is_some() => {
                tokio::time::sleep(Duration::from_secs(1)).await
            }
            _ => break,
        }
    }
    let ns = serde_json::from_value(serde_json::json!({
        "metadata": {"name": NS, "labels": {"kubyl.dev/owner": "helm-live"}}
    }))
    .unwrap();
    let _ = api.create(&Default::default(), &ns).await;
    // Only ever in a namespace these tests made.
    let owner = api.get(NS).await.ok().and_then(|ns| {
        ns.metadata
            .labels
            .and_then(|l| l.get("kubyl.dev/owner").cloned())
    });
    assert_eq!(
        owner.as_deref(),
        Some("helm-live"),
        "namespace {NS} exists and isn't marked kubyl.dev/owner=helm-live"
    );
}

async fn delete_namespace() {
    let client = client().await;
    let api: kube::Api<k8s_openapi::api::core::v1::Namespace> = kube::Api::all(client);
    if let Ok(ns) = api.get(NS).await
        && ns
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get("kubyl.dev/owner"))
            .is_some_and(|o| o == "helm-live")
    {
        let _ = api.delete(NS, &Default::default()).await;
    }
}

async fn latest_object(name: &str) -> String {
    let client = client().await;
    let api: kube::Api<k8s_openapi::api::core::v1::Secret> = kube::Api::namespaced(client, NS);
    let list = api
        .list_metadata(&kube::api::ListParams::default().labels(&format!("owner=helm,name={name}")))
        .await
        .unwrap();
    let mut objects: Vec<(u32, String)> = list
        .items
        .iter()
        .filter_map(|o| {
            let version = o.metadata.labels.as_ref()?.get("version")?.parse().ok()?;
            Some((version, o.metadata.name.clone()?))
        })
        .collect();
    objects.sort();
    objects.last().unwrap().1.clone()
}

#[tokio::test]
#[ignore]
async fn install_upgrade_rollback_uninstall() {
    let helm = helm().await;
    ensure_namespace(&helm).await;
    let name = "live-web";
    let install = InstallSpec {
        name: name.into(),
        namespace: NS.into(),
        create_namespace: false,
        chart: demo(),
        version: Some("0.1.0".into()),
        options: WriteOptions {
            wait: true,
            timeout: "3m".into(),
            ..WriteOptions::default()
        },
    };
    let values = "replicaCount: 1\nauth:\n  password: live-secret\n";

    // Preview: the server-side dry run lists the objects, the Secret masked; nothing applied.
    let dry = run(
        &helm,
        cmd::install(&install, values, Mode::DryRunServer, helm.version),
    )
    .await;
    let preview = preview::install_preview(&dry.stdout, false).unwrap();
    let kinds: Vec<&str> = preview
        .changes
        .iter()
        .map(|c| c.key.kind.as_str())
        .collect();
    assert!(
        kinds.contains(&"Deployment") && kinds.contains(&"Secret"),
        "{kinds:?}"
    );
    assert!(
        !preview
            .changes
            .iter()
            .any(|c| c.text.contains("live-secret"))
    );
    let status = cli::run(
        &helm,
        Some(&target()),
        cmd::status(name, NS, Driver::Secret),
        None,
        None,
    )
    .await;
    assert!(status.is_err(), "a dry run must not install");

    // Install, then the release is in its storage Secret.
    let out = run(
        &helm,
        cmd::install(&install, values, Mode::Apply, helm.version),
    )
    .await;
    assert!(out.stdout.contains("\"version\":1"));
    let current = release::load(
        client().await,
        Driver::Secret,
        NS.into(),
        latest_object(name).await,
    )
    .await
    .unwrap();
    assert_eq!(current.summary.status, "deployed");
    assert_eq!(current.values["replicaCount"], 1);

    // Upgrade to 0.2.0 with a values change: the preview diffs per object and the values.
    let upgrade = UpgradeSpec {
        name: name.into(),
        namespace: NS.into(),
        driver: Driver::Secret,
        chart: demo(),
        version: Some("0.2.0".into()),
        values_mode: ValuesMode::Replace,
        options: WriteOptions {
            wait: true,
            timeout: "3m".into(),
            ..WriteOptions::default()
        },
    };
    let new_values = "replicaCount: 2\nauth:\n  password: live-secret\n";
    let dry = run(
        &helm,
        cmd::upgrade(&upgrade, new_values, Mode::DryRunServer, helm.version),
    )
    .await;
    let preview = preview::upgrade_preview(&dry.stdout, &current, false).unwrap();
    let deployment = preview
        .changes
        .iter()
        .find(|c| c.key.kind == "Deployment")
        .unwrap();
    assert_eq!(deployment.change, Change::Changed);
    assert!(
        preview
            .changes
            .iter()
            .any(|c| c.change == Change::Added && c.key.kind == "PersistentVolumeClaim")
    );
    assert!(preview.values.as_ref().is_some_and(|v| !v.is_empty()));
    assert_eq!(preview.crds, ["widgets.demo.kubyl.dev"]);
    assert!(!preview.hooks.is_empty());
    let out = run(
        &helm,
        cmd::upgrade(&upgrade, new_values, Mode::Apply, helm.version),
    )
    .await;
    assert!(out.stdout.contains("\"version\":2"));

    // Rollback: preview from the stored revisions, then roll back to 1.
    let kube = client().await;
    let v1 = release::load(
        kube.clone(),
        Driver::Secret,
        NS.into(),
        format!("sh.helm.release.v1.{name}.v1"),
    )
    .await
    .unwrap();
    let v2 = release::load(
        kube,
        Driver::Secret,
        NS.into(),
        format!("sh.helm.release.v1.{name}.v2"),
    )
    .await
    .unwrap();
    let preview = preview::rollback_preview(&v2, &v1);
    assert!(preview.changes.iter().any(|c| c.change == Change::Removed));
    run(
        &helm,
        cmd::rollback(&RollbackSpec {
            name: name.into(),
            namespace: NS.into(),
            driver: Driver::Secret,
            revision: 1,
            wait: true,
            timeout: "3m".into(),
            cleanup_on_fail: true,
            no_hooks: false,
        }),
    )
    .await;
    let rolled = release::load(
        client().await,
        Driver::Secret,
        NS.into(),
        latest_object(name).await,
    )
    .await
    .unwrap();
    assert_eq!(rolled.summary.revision, 3);
    assert_eq!(rolled.summary.chart.version, "0.1.0");

    // Uninstall: the plan says what stays; afterwards the release is gone.
    let plan = preview::uninstall_plan(&rolled);
    assert!(plan.deleted.iter().any(|k| k.kind == "Deployment"));
    run(
        &helm,
        cmd::uninstall(&UninstallSpec {
            name: name.into(),
            namespace: NS.into(),
            driver: Driver::Secret,
            keep_history: false,
            no_hooks: false,
            wait: true,
            timeout: "2m".into(),
        }),
    )
    .await;
    assert!(
        cli::run(
            &helm,
            Some(&target()),
            cmd::status(name, NS, Driver::Secret),
            None,
            None
        )
        .await
        .is_err()
    );
}

#[tokio::test]
#[ignore]
async fn oci_install_and_errors() {
    let helm = helm().await;
    ensure_namespace(&helm).await;
    let (chart, version) =
        repo::parse_reference("oci://localhost:5022/charts/kubyl-demo:0.2.0").unwrap();
    // The chart's details come from a pull (schema, CRDs).
    let details = repo::details(&helm, &chart, version.as_deref())
        .await
        .unwrap();
    assert_eq!(details.chart.version, "0.2.0");
    assert!(details.schema.is_some());
    let spec = InstallSpec {
        name: "live-oci".into(),
        namespace: NS.into(),
        create_namespace: false,
        chart,
        version,
        options: WriteOptions {
            timeout: "3m".into(),
            skip_crds: true,
            ..WriteOptions::default()
        },
    };
    // The schema rejects a bad value before anything is installed.
    let err = cli::run(
        &helm,
        Some(&target()),
        cmd::install(
            &spec,
            "replicaCount: lots\n",
            Mode::DryRunServer,
            helm.version,
        ),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, cli::ErrorKind::Schema, "{err:?}");
    run(
        &helm,
        cmd::install(&spec, "replicaCount: 1\n", Mode::Apply, helm.version),
    )
    .await;
    // Installing the same name again: "cannot re-use a name".
    let err = cli::run(
        &helm,
        Some(&target()),
        cmd::install(&spec, "", Mode::Apply, helm.version),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, cli::ErrorKind::NameInUse, "{err:?}");
    run(
        &helm,
        cmd::uninstall(&UninstallSpec {
            name: "live-oci".into(),
            namespace: NS.into(),
            driver: Driver::Secret,
            keep_history: false,
            no_hooks: true,
            wait: false,
            timeout: "2m".into(),
        }),
    )
    .await;
}

/// Kubyl quitting during a write (`--wait` here): the runtime, the future and the readers of
/// `helm`'s pipes go away, `helm` finishes and records the release as deployed.
#[test]
#[ignore]
fn writes_finish_after_kubyl_goes_away() {
    let runtime = || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    };
    let name = "live-quit";
    let kubyl = runtime();
    let helm = kubyl.block_on(async {
        let helm = helm().await;
        ensure_namespace(&helm).await;
        helm
    });
    let spec = InstallSpec {
        name: name.into(),
        namespace: NS.into(),
        create_namespace: false,
        chart: demo(),
        version: Some("0.1.0".into()),
        options: WriteOptions {
            wait: true,
            timeout: "3m".into(),
            ..WriteOptions::default()
        },
    };
    let invocation = cmd::install(&spec, "replicaCount: 1\n", Mode::Apply, helm.version);
    let (progress, _lines) = tokio::sync::mpsc::unbounded_channel();
    let running = helm.clone();
    kubyl.spawn(async move {
        cli::run(&running, Some(&target()), invocation, Some(progress), None).await
    });
    std::thread::sleep(Duration::from_millis(1500));
    kubyl.shutdown_background();

    let after = runtime();
    let deployed = after.block_on(async {
        for _ in 0..90 {
            if let Ok(out) = cli::run(
                &helm,
                Some(&target()),
                cmd::status(name, NS, Driver::Secret),
                None,
                None,
            )
            .await
                && out.stdout.contains("\"status\":\"deployed\"")
            {
                return true;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        false
    });
    assert!(deployed, "helm didn't finish the install on its own");
    after.block_on(run(
        &helm,
        cmd::uninstall(&UninstallSpec {
            name: name.into(),
            namespace: NS.into(),
            driver: Driver::Secret,
            keep_history: false,
            no_hooks: true,
            wait: false,
            timeout: "2m".into(),
        }),
    ));
}

#[tokio::test]
#[ignore]
async fn stuck_releases_are_reported_and_repositories_list() {
    let helm = helm().await;
    // The script's kubyl-stuck release stays pending-upgrade: Helm refuses a new upgrade.
    let spec = UpgradeSpec {
        name: "kubyl-stuck".into(),
        namespace: "kubyl-helm".into(),
        driver: Driver::Secret,
        chart: demo(),
        version: Some("0.1.0".into()),
        values_mode: ValuesMode::Reuse,
        options: WriteOptions::default(),
    };
    let err = cli::run(
        &helm,
        Some(&target()),
        cmd::upgrade(&spec, "", Mode::DryRunServer, helm.version),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, cli::ErrorKind::InProgress, "{err:?}");
    let repos = cli::run(&helm, None, repo::list_repositories(), None, None)
        .await
        .map(|o| repo::parse_repositories(&o.stdout).unwrap())
        .unwrap();
    assert!(repos.iter().any(|r| r.name == "kubyl-dev"));
    let hits = cli::run(
        &helm,
        None,
        repo::versions("kubyl-dev", "kubyl-demo"),
        None,
        None,
    )
    .await
    .map(|o| repo::parse_versions(&o.stdout, "kubyl-dev", "kubyl-demo").unwrap())
    .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.version.as_str()).collect::<Vec<_>>(),
        ["0.2.0", "0.1.0"]
    );
}

#[tokio::test]
#[ignore]
async fn zz_cleanup() {
    tokio::time::sleep(Duration::from_millis(100)).await;
    delete_namespace().await;
}
