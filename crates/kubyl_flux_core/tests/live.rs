//! Flux against a real cluster. Ignored by default; run against the kind dev cluster with
//! `script/flux-dev.sh`:
//!
//! ```sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev/kubeconfig
//! KUBECONFIG=/tmp/kubyl-dev/kubeconfig script/flux-dev.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_flux_core --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Uses the samples of `script/flux-dev.sh` in `flux-demo`: detection of the controllers, the
//! states of the samples (failing, suspended, waiting), and reconcile / suspend / resume round
//! trips on `apps` (left resumed and ready), reset and force on `podinfo-helm`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use kube::api::{Api, DynamicObject, ListParams};
use kube::discovery::ApiResource;
use kubyl_flux_core::deps::Graph;
use kubyl_flux_core::detect;
use kubyl_flux_core::inventory;
use kubyl_flux_core::kinds::FluxKind;
use kubyl_flux_core::model::{FluxObject, State};
use kubyl_flux_core::ops::{self, Action, Target};
use kubyl_flux_core::overview::{Attention, Summary};

const NAMESPACE: &str = "flux-demo";

/// The round trips change `apps` and `podinfo-helm`: one at a time, also without
/// `--test-threads=1`.
static WRITES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Resumes `apps` if an earlier run stopped between suspend and resume.
async fn ensure_resumed(client: &kube::Client) {
    if get(client, FluxKind::Kustomization, "apps").await.suspended {
        let apps = target(client, FluxKind::Kustomization, "apps").await;
        ops::run(client.clone(), apps, None, Action::Resume)
            .await
            .expect("resume apps left suspended");
        wait(
            client,
            FluxKind::Kustomization,
            "apps",
            "apps resumed",
            |o| !o.suspended && o.state() == State::Ready,
        )
        .await;
    }
}

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

/// The served version of a kind, from discovery.
async fn resource(client: &kube::Client, kind: FluxKind) -> ApiResource {
    let discovery = kube::Discovery::new(client.clone())
        .filter(&[kind.group()])
        .run()
        .await
        .expect("discovery");
    for group in discovery.groups() {
        for (resource, _) in group.recommended_resources() {
            if resource.plural == kind.plural() {
                return resource;
            }
        }
    }
    panic!("{kind} isn't served: run script/flux-dev.sh");
}

async fn list(client: &kube::Client, kind: FluxKind) -> Vec<FluxObject> {
    let resource = resource(client, kind).await;
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), NAMESPACE, &resource);
    api.list(&ListParams::default())
        .await
        .expect("list")
        .items
        .into_iter()
        .filter_map(|o| FluxObject::parse_as(kind, &Arc::new(serde_json::to_value(o).unwrap())))
        .collect()
}

async fn get(client: &kube::Client, kind: FluxKind, name: &str) -> FluxObject {
    let resource = resource(client, kind).await;
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), NAMESPACE, &resource);
    let object = api.get(name).await.expect("get");
    FluxObject::parse_as(kind, &Arc::new(serde_json::to_value(object).unwrap())).unwrap()
}

async fn target(client: &kube::Client, kind: FluxKind, name: &str) -> Target {
    Target {
        resource: resource(client, kind).await,
        namespace: NAMESPACE.into(),
        name: name.into(),
    }
}

async fn wait(
    client: &kube::Client,
    kind: FluxKind,
    name: &str,
    what: &str,
    check: impl Fn(&FluxObject) -> bool,
) -> FluxObject {
    let start = Instant::now();
    loop {
        let object = get(client, kind, name).await;
        if check(&object) {
            println!("{what}: after {:?}", start.elapsed());
            return object;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

#[tokio::test]
#[ignore]
async fn detects_the_controllers_and_their_versions() {
    let client = client().await;
    let install = detect::detect(client).await.expect("detect");
    assert_eq!(install.namespace.as_deref(), Some("flux-system"));
    for name in [
        "source-controller",
        "kustomize-controller",
        "helm-controller",
        "notification-controller",
    ] {
        let controller = install.controller(name).unwrap_or_else(|| panic!("{name}"));
        assert!(controller.is_ready(), "{name} isn't ready");
        assert!(
            controller
                .version
                .as_deref()
                .is_some_and(|v| v.starts_with('v'))
        );
    }
    println!("Flux controllers: {:?}", install.version());
}

#[tokio::test]
#[ignore]
async fn lists_show_the_samples_states() {
    let client = client().await;
    let kustomizations = list(&client, FluxKind::Kustomization).await;
    let state = |name: &str| {
        kustomizations
            .iter()
            .find(|k| k.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .state()
    };
    assert_eq!(state("broken"), State::Failed);
    assert_eq!(state("paused"), State::Suspended);
    assert_eq!(state("apps-late"), State::Failed);
    assert!(matches!(
        state("podinfo"),
        State::Ready | State::Reconciling
    ));
    let graph = Graph::new(&kustomizations);
    assert_eq!(
        graph.waiting_for("flux-demo/apps-late").unwrap().key,
        "flux-demo/broken"
    );
    let podinfo = kustomizations.iter().find(|k| k.name == "podinfo").unwrap();
    let entries = inventory::entries(&podinfo.raw).expect("inventory");
    assert!(
        entries
            .iter()
            .any(|e| e.kind == "Deployment" && e.name == "podinfo")
    );
    let summary = Summary::new(&kustomizations, jiff::Timestamp::now());
    assert!(summary.problems.iter().any(|p| p.object.name == "broken"
        && p.why == Attention::Failed
        && p.message.contains("path not found")));

    let releases = list(&client, FluxKind::HelmRelease).await;
    let helm = releases.iter().find(|r| r.name == "podinfo-helm").unwrap();
    assert_eq!(helm.state(), State::Ready);
    assert!(helm.revision().is_some_and(|r| r.starts_with("6.")));
    let sources = list(&client, FluxKind::GitRepository).await;
    let podinfo_source = sources
        .iter()
        .find(|s| s.name == "podinfo")
        .expect("podinfo");
    assert!(podinfo_source.artifact.is_some());
    // OCI HelmRepositories are static: no status, Ready, nothing to reconcile or to fetch.
    let repositories = list(&client, FluxKind::HelmRepository).await;
    let oci = repositories
        .iter()
        .find(|r| r.name == "podinfo-oci")
        .expect("podinfo-oci (script/flux-dev.sh)");
    assert!(oci.is_static() && oci.conditions.is_empty());
    assert_eq!(oci.state(), State::Ready);
    assert!(!Action::Reconcile.applies_to(oci));
    let summary = Summary::new(&repositories, jiff::Timestamp::now());
    assert!(summary.problems.is_empty(), "{:?}", summary.problems);
    let policies = list(&client, FluxKind::ImagePolicy).await;
    assert!(
        policies
            .iter()
            .find(|p| p.name == "podinfo")
            .expect("podinfo")
            .revision()
            .is_some_and(|r| r.contains("podinfo:6."))
    );
}

#[tokio::test]
#[ignore]
async fn reconcile_with_source_round_trip() {
    let _writes = WRITES.lock().await;
    let client = client().await;
    ensure_resumed(&client).await;
    let source = target(&client, FluxKind::GitRepository, "podinfo").await;
    let apps = target(&client, FluxKind::Kustomization, "apps").await;
    ops::run(
        client.clone(),
        apps,
        Some(source),
        Action::ReconcileWithSource,
    )
    .await
    .expect("reconcile");
    let object = get(&client, FluxKind::Kustomization, "apps").await;
    let requested = object.requested_at.clone().expect("requestedAt");
    wait(
        &client,
        FluxKind::Kustomization,
        "apps",
        "the reconcile handled",
        |o| o.last_handled.as_ref() == Some(&requested) && o.state() == State::Ready,
    )
    .await;
    // The source handled the same request first (`ops::run` waited for it).
    let source = get(&client, FluxKind::GitRepository, "podinfo").await;
    assert_eq!(source.last_handled.as_ref(), Some(&requested));
}

#[tokio::test]
#[ignore]
async fn suspend_and_resume_round_trip() {
    let _writes = WRITES.lock().await;
    let client = client().await;
    ensure_resumed(&client).await;
    let apps = target(&client, FluxKind::Kustomization, "apps").await;
    ops::run(client.clone(), apps.clone(), None, Action::Suspend)
        .await
        .expect("suspend");
    wait(&client, FluxKind::Kustomization, "apps", "suspended", |o| {
        o.suspended && o.state() == State::Suspended
    })
    .await;
    // A suspended object takes no reconcile requests.
    let suspended = get(&client, FluxKind::Kustomization, "apps").await;
    assert!(!Action::Reconcile.applies_to(&suspended));
    ops::run(client.clone(), apps, None, Action::Resume)
        .await
        .expect("resume");
    let resumed = wait(
        &client,
        FluxKind::Kustomization,
        "apps",
        "resumed and ready",
        |o| {
            !o.suspended
                && o.requested_at.is_some()
                && o.last_handled == o.requested_at
                && o.state() == State::Ready
        },
    )
    .await;
    assert!(resumed.message().starts_with("Applied revision"));
}

#[tokio::test]
#[ignore]
async fn helm_release_force_and_reset() {
    let _writes = WRITES.lock().await;
    let client = client().await;
    let release = target(&client, FluxKind::HelmRelease, "podinfo-helm").await;
    // helm-controller acts on forceAt/resetAt only when they equal requestedAt, and records
    // what it handled in lastHandledForceAt/lastHandledResetAt.
    for (action, annotation, handled) in [
        (Action::Reset, "resetAt", "lastHandledResetAt"),
        (Action::Force, "forceAt", "lastHandledForceAt"),
    ] {
        ops::run(client.clone(), release.clone(), None, action)
            .await
            .unwrap_or_else(|err| panic!("{}: {err}", action.label()));
        let object = get(&client, FluxKind::HelmRelease, "podinfo-helm").await;
        let at = object.requested_at.clone().unwrap();
        assert_eq!(
            object
                .raw
                .pointer(&format!(
                    "/metadata/annotations/reconcile.fluxcd.io~1{annotation}"
                ))
                .and_then(|v| v.as_str()),
            Some(at.as_str())
        );
        wait(
            &client,
            FluxKind::HelmRelease,
            "podinfo-helm",
            &format!("{} handled", action.label()),
            |o| {
                o.raw
                    .pointer(&format!("/status/{handled}"))
                    .and_then(|v| v.as_str())
                    == Some(at.as_str())
                    && o.state() == State::Ready
            },
        )
        .await;
    }
}
