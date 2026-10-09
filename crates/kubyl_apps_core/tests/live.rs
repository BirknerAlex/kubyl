//! Applications against the kind dev cluster. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! script/apps-dev.sh
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_apps_core --test live -- --ignored --nocapture
//! ```
//!
//! Read-only. Reads the kinds the way Kubyl's watches do (only objects with the instance
//! label), groups them and checks the sample applications of `script/apps-dev.sh`, and the
//! Helm release of metrics-server when `script/prometheus-dev.sh` installed it.

use std::collections::HashSet;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{ConfigMap, PersistentVolumeClaim, Service};
use k8s_openapi::api::networking::v1::Ingress;
use kube::api::{Api, DynamicObject, ListParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::discovery::ApiResource;
use kube::{Client, Config};
use kubyl_apps_core::{App, Health, Kind, Manager, build};
use serde_json::Value;

async fn client() -> Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let options = KubeConfigOptions {
        context: Some(std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(Kubeconfig::read_from(path).unwrap(), &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

fn resource(kind: Kind) -> ApiResource {
    match kind {
        Kind::Deployment => ApiResource::erase::<Deployment>(&()),
        Kind::StatefulSet => ApiResource::erase::<StatefulSet>(&()),
        Kind::DaemonSet => ApiResource::erase::<DaemonSet>(&()),
        Kind::CronJob => ApiResource::erase::<CronJob>(&()),
        Kind::Job => ApiResource::erase::<Job>(&()),
        Kind::Service => ApiResource::erase::<Service>(&()),
        Kind::Ingress => ApiResource::erase::<Ingress>(&()),
        Kind::ConfigMap => ApiResource::erase::<ConfigMap>(&()),
        Kind::PersistentVolumeClaim => ApiResource::erase::<PersistentVolumeClaim>(&()),
    }
}

async fn apps(client: &Client) -> Vec<App> {
    let mut objects: Vec<(Kind, Value)> = Vec::new();
    for kind in Kind::ALL {
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &resource(kind));
        let list = api
            .list(&ListParams::default().labels(kubyl_apps_core::INSTANCE))
            .await
            .unwrap_or_else(|e| panic!("listing {}: {e}", kind.plural()));
        objects.extend(
            list.items
                .into_iter()
                .map(|o| (kind, serde_json::to_value(o).unwrap())),
        );
    }
    let refs: Vec<(Kind, &Value)> = objects.iter().map(|(k, o)| (*k, o)).collect();
    build(&refs, &HashSet::new())
}

fn find<'a>(apps: &'a [App], key: &str) -> &'a App {
    apps.iter()
        .find(|a| a.key() == key)
        .unwrap_or_else(|| panic!("no application {key}: run script/apps-dev.sh"))
}

#[tokio::test]
#[ignore = "needs a cluster with script/apps-dev.sh"]
async fn sample_applications_group_with_their_health_and_manager() {
    let apps = apps(&client().await).await;
    for app in &apps {
        println!(
            "{:<28} {:<14} {:<26} {:<16} {} objects",
            app.key(),
            app.health.label(),
            app.manager.label(),
            app.version,
            app.members.len()
        );
    }
    let shop = find(&apps, "kubyl-apps/shop");
    assert_eq!(shop.health, Health::Healthy);
    assert_eq!(shop.manager, Manager::Label("kustomize".into()));
    assert_eq!(shop.version, "2.3.0, 2.3.1");
    assert_eq!(shop.part_of.as_deref(), Some("webshop"));
    let kinds: Vec<Kind> = shop.members.iter().map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            Kind::Deployment,
            Kind::Deployment,
            Kind::Service,
            Kind::ConfigMap
        ]
    );
    assert_eq!(shop.log_sources().count(), 2);

    let ledger = find(&apps, "kubyl-apps/ledger");
    assert_eq!(ledger.health, Health::Degraded);
    assert_eq!(
        ledger.manager,
        Manager::Helm {
            namespace: "kubyl-apps".into(),
            release: "ledger".into()
        }
    );
    assert_eq!(ledger.version, "16.4");

    let report = find(&apps, "kubyl-apps/report");
    assert_eq!(report.health, Health::Suspended);
    assert_eq!(report.manager, Manager::None);
    assert_eq!(find(&apps, "kubyl-apps/idle").health, Health::Suspended);

    // Degraded first.
    let position = |key: &str| apps.iter().position(|a| a.key() == key).unwrap();
    assert!(position("kubyl-apps/ledger") < position("kubyl-apps/shop"));
    assert!(position("kubyl-apps/shop") < position("kubyl-apps/idle"));
}

#[tokio::test]
#[ignore = "needs a cluster with metrics-server (script/prometheus-dev.sh)"]
async fn a_real_helm_release_is_found_by_its_annotations() {
    let apps = apps(&client().await).await;
    let metrics = find(&apps, "kube-system/metrics-server");
    assert_eq!(
        metrics.manager,
        Manager::Helm {
            namespace: "kube-system".into(),
            release: "metrics-server".into()
        }
    );
    assert!(!metrics.version.is_empty());
    println!(
        "metrics-server {} {}",
        metrics.version,
        metrics.health.label()
    );
}
