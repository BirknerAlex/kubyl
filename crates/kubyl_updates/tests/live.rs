//! Cluster updates against the dev clusters of `script/updates-dev.sh`. Ignored by default;
//! each test skips when its kubeconfig variable isn't set. Run them with:
//!
//! ```sh
//! script/dev-cluster.sh && script/updates-dev.sh --all
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//! KUBYL_TEST_OCP_KUBECONFIG=/tmp/kubyl-dev/ocp-kubeconfig \
//! KUBYL_TEST_K3S_KUBECONFIG=/tmp/kubyl-dev/k3s-kubeconfig \
//!   cargo test -p kubyl_updates --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! - `kind_*` (kind-kubyl-dev): the read-only provider, and pre-flight flagging the PDB with
//!   0 allowed disruptions and the Helm release whose manifest uses removed APIs.
//! - `ocp_*` (the fake OpenShift, kind-kubyl-ocp): reading ClusterVersion, operators and
//!   pools; pre-flight from APIRequestCount and the admin gates. `ocp_update_is_tracked`
//!   starts an update to 4.17.10 like `oc adm upgrade --to`; with `script/updates-dev.sh
//!   --fake-cvo` running in another terminal (set `KUBYL_TEST_FAKE_CVO=1`) it follows the
//!   update to completion, else it checks the patch and resets the cluster
//!   (`script/updates-dev.sh --ocp-reset` resets it too).
//! - `k3s_*` (kubyl-k3s): system-upgrade-controller Plans and their progress.
//! - `openshift_read_only` reads a real OpenShift cluster, never writes, and prints what
//!   `oc adm upgrade` shows for comparison: set `KUBYL_TEST_OPENSHIFT_KUBECONFIG` and
//!   `KUBYL_TEST_OPENSHIFT_CONTEXT`. Keep its output out of the repo.

use std::time::{Duration, Instant};

use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_operators::helm::decode::Driver;
use kubyl_updates::check::CheckStatus;
use kubyl_updates::model::{PoolKind, ProviderKind, Scope, TargetKind};
use kubyl_updates::preflight::{self, HelmInput, Inputs, ReleaseRef};
use kubyl_updates::provider::UpdateProvider;

async fn client(var: &str, context: &str) -> Option<Client> {
    let path = std::env::var(var).ok()?;
    let kubeconfig = Kubeconfig::read_from(path).unwrap();
    let options = KubeConfigOptions {
        context: Some(context.to_string()),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .unwrap();
    Some(Client::try_from(config).unwrap())
}

async fn inputs(client: Client, provider: ProviderKind, target: &str, helm: HelmInput) -> Inputs {
    let discovery = kubyl_kube::discovery::discover(&client).await.unwrap();
    let current_kube = client.apiserver_version().await.unwrap().git_version;
    Inputs {
        provider,
        current_kube,
        target: target.to_string(),
        target_kube: preflight::target_kube_minor(provider, target),
        prometheus: None,
        api_request_counts: discovery
            .preferred()
            .any(|r| r.gvr.group == "apiserver.openshift.io" && r.gvr.resource == "apirequestcounts"),
        scan: preflight::scan_kinds(&discovery),
        helm,
        client,
    }
}

#[tokio::test]
#[ignore]
async fn kind_read_only_and_preflight() {
    let Some(client) = client("KUBYL_TEST_KUBECONFIG", "kind-kubyl-dev").await else {
        return;
    };
    let provider = kubyl_updates::fallback::ReadOnly::new(
        client.clone(),
        ProviderKind::SelfManaged,
        "kind",
        "not updatable",
        Vec::new(),
    );
    let status = provider.read().await.unwrap();
    println!("kind: {} · pools {:?}", status.current.version, status.pools.iter().map(|p| &p.name).collect::<Vec<_>>());
    assert!(status.current.version.starts_with("v1."));
    assert!(status.targets.is_empty());
    assert!(!status.writes.any());
    assert!(status.pools.iter().any(|p| p.kind == PoolKind::ControlPlane));

    let next = kubyl_updates::version::next_minor(&status.current.version).unwrap();
    let helm = HelmInput::Releases(vec![ReleaseRef {
        namespace: "kubyl-updates".into(),
        name: "legacy-app".into(),
        driver: Driver::Secret,
        object: "sh.helm.release.v1.legacy-app.v1".into(),
        revision: 1,
    }]);
    let checks = preflight::run(inputs(client, ProviderKind::SelfManaged, &next, helm).await).await;
    for check in &checks {
        println!("{:?} {}: {}", check.status, check.title, check.summary);
    }
    let pdb = checks.iter().find(|c| c.id == "pdb").unwrap();
    assert_eq!(pdb.status, CheckStatus::Fail);
    assert!(pdb.details.iter().any(|d| d.text.contains("kubyl-updates/ledger-writer-pdb")));
    let helm = checks.iter().find(|c| c.id == "removed-apis-helm").unwrap();
    assert!(matches!(helm.status, CheckStatus::Warn | CheckStatus::Fail), "{helm:?}");
    assert!(helm.details[0].sub.as_deref().unwrap().contains("extensions/v1beta1 Ingress"));
    let skew = checks.iter().find(|c| c.id == "version-skew").unwrap();
    assert_eq!(skew.status, CheckStatus::Pass, "{skew:?}");
}

#[tokio::test]
#[ignore]
async fn ocp_reads_cluster_version_operators_and_pools() {
    let Some(client) = client("KUBYL_TEST_OCP_KUBECONFIG", "kind-kubyl-ocp").await else {
        return;
    };
    let provider = kubyl_updates::openshift::OpenShift::new(client.clone());
    let status = provider.read().await.unwrap();
    assert_eq!(status.current.channel.as_deref(), Some("stable-4.17"));
    assert!(status.current.channels.iter().any(|c| c == "stable-4.18"));
    assert!(!status.components.is_empty());
    assert!(status.pools.iter().any(|p| p.name == "worker"));
    let recommended = status.targets.iter().find(|t| t.kind == TargetKind::Recommended).unwrap();
    println!(
        "ocp: {} → {:?}",
        status.current.version,
        status.targets.iter().map(|t| (&t.version, t.kind)).collect::<Vec<_>>()
    );
    assert!(status.targets.iter().any(|t| t.kind == TargetKind::Conditional && !t.risks.is_empty()));
    let blocked = status.targets.iter().find(|t| t.minor).unwrap();
    assert_eq!(blocked.kind, TargetKind::Blocked);

    let target = recommended.version.clone();
    let mut checks = preflight::run(
        inputs(client.clone(), ProviderKind::OpenShift, &target, HelmInput::Releases(Vec::new())).await,
    )
    .await;
    checks.extend(provider.preflight_extras(&status, &target).await);
    for check in &checks {
        println!("{:?} {}: {}", check.status, check.title, check.summary);
    }
    let deprecated = checks.iter().find(|c| c.id == "deprecated-apis").unwrap();
    assert!(deprecated.summary.contains("APIRequestCount"), "{deprecated:?}");
    assert!(checks.iter().any(|c| c.id == "cluster-operators" && c.status == CheckStatus::Pass));
    // The minor update needs the admin ack.
    let minor = provider.preflight_extras(&status, &blocked.version).await;
    assert!(minor.iter().any(|c| c.id == "upgradeable" && c.status == CheckStatus::Fail));
}

#[tokio::test]
#[ignore]
async fn ocp_update_is_tracked() {
    let Some(client) = client("KUBYL_TEST_OCP_KUBECONFIG", "kind-kubyl-ocp").await else {
        return;
    };
    let provider = kubyl_updates::openshift::OpenShift::new(client.clone());
    let status = provider.read().await.unwrap();
    assert!(status.progress.is_none(), "reset the fake cluster first (script/updates-dev.sh --ocp-reset)");
    let plan = provider.plan(&status, &Scope::ControlPlane, "4.17.10", "ocp").unwrap();
    provider.start(&plan).await.unwrap();
    let cv = kubyl_updates::kube_api::get(&client, "/apis/config.openshift.io/v1/clusterversions/version")
        .await
        .unwrap();
    assert_eq!(cv["spec"]["desiredUpdate"]["version"], "4.17.10");
    assert_eq!(cv["spec"]["desiredUpdate"]["force"], false);
    if std::env::var("KUBYL_TEST_FAKE_CVO").is_ok() {
        // The fake CVO plays the stages; Kubyl reads its progress until it completes.
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut seen_progress = false;
        loop {
            let status = provider.read().await.unwrap();
            if let Some(progress) = &status.progress {
                seen_progress = true;
                println!(
                    "{:?}% {} · operators {}/{} · pools {:?}",
                    progress.percent,
                    progress.message,
                    status.components.iter().filter(|c| c.updated).count(),
                    status.components.len(),
                    status.pools.iter().map(|p| (&p.name, p.updated, p.nodes, &p.draining)).collect::<Vec<_>>()
                );
            } else if seen_progress && status.current.version == "4.17.10" {
                assert!(status.history[0].completed());
                break;
            }
            assert!(Instant::now() < deadline, "the update didn't finish");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    } else {
        // Nothing runs the update: take the request back.
        kubyl_updates::kube_api::merge_patch(
            &client,
            "/apis/config.openshift.io/v1/clusterversions/version",
            &serde_json::json!({"spec": {"desiredUpdate": null}}),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
#[ignore]
async fn k3s_plans_and_progress() {
    let Some(client) = client("KUBYL_TEST_K3S_KUBECONFIG", "k3d-kubyl-k3s").await else {
        return;
    };
    let provider = kubyl_updates::suc::Suc::new(client, ProviderKind::K3s);
    let status = provider.read().await.unwrap();
    println!(
        "k3s: {} → {:?} · plans {:?}",
        status.current.version,
        status.targets.iter().map(|t| (&t.version, t.kind)).collect::<Vec<_>>(),
        status.pools.iter().map(|p| (&p.name, p.updated, p.nodes, &p.version)).collect::<Vec<_>>()
    );
    assert!(status.current.version.contains("+k3s"));
    let plans: Vec<_> = status.pools.iter().filter(|p| p.kind == PoolKind::Plan).collect();
    assert_eq!(plans.len(), 2);
    for plan in plans {
        assert_eq!(plan.updated, plan.nodes, "{plan:?}");
    }
    assert!(status.writes.pools);
}

#[tokio::test]
#[ignore]
async fn openshift_read_only() {
    let Ok(context) = std::env::var("KUBYL_TEST_OPENSHIFT_CONTEXT") else {
        return;
    };
    let Some(client) = client("KUBYL_TEST_OPENSHIFT_KUBECONFIG", &context).await else {
        return;
    };
    let provider = kubyl_updates::openshift::OpenShift::new(client);
    let status = provider.read().await.unwrap();
    println!("Cluster version is {}", status.current.version);
    println!("Channel: {}", status.current.channel.clone().unwrap_or_default());
    for target in &status.targets {
        println!("  {} {:?} {:?}", target.version, target.kind, target.risks.iter().map(|r| &r.name).collect::<Vec<_>>());
    }
    for entry in status.history.iter().take(5) {
        println!("  history {} {}", entry.version, entry.state);
    }
    println!(
        "operators {} · pools {:?}",
        status.components.len(),
        status.pools.iter().map(|p| (&p.name, p.updated, p.nodes)).collect::<Vec<_>>()
    );
    assert!(!status.current.version.is_empty());
}
