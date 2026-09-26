//! OLM and Helm against the kind dev cluster. Ignored by default; run them with:
//!
//! ```sh
//! script/dev-cluster.sh && script/prometheus-dev.sh && script/olm-dev.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_operators --test live -- --ignored --nocapture --test-threads=1
//! ```
//!
//! - `approve_moves_the_install_forward` approves the upgrade `script/olm-dev.sh` leaves
//!   waiting in `kubyl-manual`; run `script/olm-dev.sh --delete && script/olm-dev.sh` to get a
//!   new one (the test skips when nothing waits).
//! - `install_cert_manager_and_create_a_certificate` installs cert-manager from the catalog
//!   (all namespaces) when it isn't installed, and leaves it installed.
//! - `uninstall_removes_the_operator_and_its_crds` installs `ack-recyclebin-controller` into
//!   `kubyl-live-uninstall` and uninstalls it with its CRD.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use k8s_openapi::api::core::v1::{Namespace, Secret};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, ListParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_operators::helm::decode;
use kubyl_operators::helm::present;
use kubyl_operators::helm::service::group;
use kubyl_operators::olm::hub;
use kubyl_operators::olm::join::{self, OperatorStatus};
use kubyl_operators::olm::model::{
    Approval, Csv, InstallMode, InstallPlan, OperatorGroup, Subscription,
};
use kubyl_operators::olm::ops::{self, InstallChoice, Target, UninstallSpec};
use kubyl_operators::olm::review::{self, ClusterFacts, CrdStatus};
use serde_json::{Value, json};

fn kubeconfig_path() -> String {
    std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG")
}

fn context() -> String {
    std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())
}

async fn client() -> Client {
    let kubeconfig = Kubeconfig::read_from(kubeconfig_path()).unwrap();
    let options = KubeConfigOptions {
        context: Some(context()),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

fn resource(group: &str, version: &str, kind: &str, plural: &str) -> ApiResource {
    ApiResource {
        group: group.into(),
        version: version.into(),
        api_version: format!("{group}/{version}"),
        kind: kind.into(),
        plural: plural.into(),
    }
}

async fn list(client: &Client, resource: &ApiResource, labels: Option<&str>) -> Vec<Value> {
    let api: Api<DynamicObject> = Api::all_with(client.clone(), resource);
    let mut params = ListParams::default();
    if let Some(labels) = labels {
        params = params.labels(labels);
    }
    api.list(&params)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|o| serde_json::to_value(o).unwrap())
        .collect()
}

struct Olm {
    subscriptions: Vec<Arc<Subscription>>,
    csvs: Vec<Arc<Csv>>,
    plans: Vec<Arc<InstallPlan>>,
    groups: Vec<OperatorGroup>,
}

async fn olm(client: &Client) -> Olm {
    let g = "operators.coreos.com";
    Olm {
        subscriptions: list(
            client,
            &resource(g, "v1alpha1", "Subscription", "subscriptions"),
            None,
        )
        .await
        .iter()
        .filter_map(|v| Subscription::parse(v).map(Arc::new))
        .collect(),
        csvs: list(
            client,
            &resource(
                g,
                "v1alpha1",
                "ClusterServiceVersion",
                "clusterserviceversions",
            ),
            Some("!olm.copiedFrom"),
        )
        .await
        .iter()
        .filter_map(|v| Csv::parse(v).map(Arc::new))
        .collect(),
        plans: list(
            client,
            &resource(g, "v1alpha1", "InstallPlan", "installplans"),
            None,
        )
        .await
        .iter()
        .filter_map(|v| InstallPlan::parse(v).map(Arc::new))
        .collect(),
        groups: list(
            client,
            &resource(g, "v1", "OperatorGroup", "operatorgroups"),
            None,
        )
        .await
        .iter()
        .filter_map(OperatorGroup::parse)
        .collect(),
    }
}

async fn wait_for<F, Fut>(what: &str, timeout: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while !check().await {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn namespaces(client: &Client) -> Vec<String> {
    Api::<Namespace>::all(client.clone())
        .list(&ListParams::default())
        .await
        .unwrap()
        .items
        .into_iter()
        .filter_map(|n| n.metadata.name)
        .collect()
}

#[tokio::test]
#[ignore]
async fn installed_operators_join_with_their_plans() {
    let client = client().await;
    let olm = olm(&client).await;
    let operators = join::join(&olm.subscriptions, &olm.csvs, &olm.plans);
    let cnpg = operators
        .iter()
        .find(|o| o.package() == Some("cloudnative-pg") && o.namespace() == "kubyl-manual")
        .expect("script/olm-dev.sh's manual subscription");
    println!(
        "cloudnative-pg: {:?} {:?} → {:?}",
        cnpg.status,
        cnpg.version(),
        cnpg.upgrade_to
    );
    assert!(matches!(
        cnpg.status,
        OperatorStatus::UpgradeAvailable | OperatorStatus::Succeeded | OperatorStatus::Installing
    ));
    if cnpg.status == OperatorStatus::UpgradeAvailable {
        assert!(cnpg.approvable().is_some());
        assert!(cnpg.upgrade_to.is_some());
    }
    // The packageserver CSV has no Subscription and still shows.
    assert!(operators.iter().any(|o| o.subscription.is_none()));
    // No copies (the label selector keeps originals only).
    assert!(olm.csvs.iter().all(|c| !c.copied));
}

#[tokio::test]
#[ignore]
async fn package_manifests_and_icons() {
    let client = client().await;
    let start = Instant::now();
    let packages = hub::fetch(client.clone()).await.unwrap();
    println!("{} packages in {:?}", packages.len(), start.elapsed());
    assert!(packages.len() > 100);
    let cm = packages.iter().find(|p| p.name == "cert-manager").unwrap();
    let head = cm.head().unwrap();
    assert!(head.install_modes.contains(&InstallMode::AllNamespaces));
    assert!(!head.entries.is_empty());
    assert!(!cm.examples().is_empty(), "cert-manager has alm-examples");
    let keys: BTreeSet<String> = packages.iter().map(|p| p.key()).collect();
    assert_eq!(
        keys.len(),
        packages.len(),
        "one entry per package and catalog"
    );
    let cnpg = packages
        .iter()
        .find(|p| p.name == "cloudnative-pg")
        .unwrap();
    let icon = hub::fetch_icon(client, cnpg.namespace.clone(), cnpg.name.clone()).await;
    let (format, bytes) = icon.expect("cloudnative-pg's icon");
    println!("icon: {format:?}, {} bytes", bytes.len());
}

async fn pending_manual_plan(client: &Client) -> Option<(Arc<InstallPlan>, Option<Arc<Csv>>)> {
    let olm = olm(client).await;
    let operators = join::join(&olm.subscriptions, &olm.csvs, &olm.plans);
    let op = operators
        .into_iter()
        .find(|o| o.package() == Some("cloudnative-pg") && o.namespace() == "kubyl-manual")?;
    let plan = op.approvable()?.clone();
    Some((plan, op.csv))
}

#[tokio::test]
#[ignore]
async fn upgrade_review_reads_the_bundle() {
    let client = client().await;
    let Some((plan, installed)) = pending_manual_plan(&client).await else {
        println!(
            "nothing waits for approval in kubyl-manual: run script/olm-dev.sh --delete && script/olm-dev.sh"
        );
        return;
    };
    let version = client.apiserver_version().await.unwrap().git_version;
    let review = review::review(
        client,
        (*plan).clone(),
        installed.map(|c| (*c).clone()),
        ClusterFacts {
            kube_version: Some(version),
            openshift_version: None,
        },
        false,
    )
    .await;
    println!(
        "review: version {:?}, {} CRDs ({} change), {} RBAC changes, notes {:?}, problems {:?}",
        review.version,
        review.crds.len(),
        review.changed_crds(),
        review.rbac.len(),
        review.notes,
        review.problems
    );
    assert!(review.problems.is_empty(), "{:?}", review.problems);
    assert!(!review.crds.is_empty());
    assert!(review.crds.iter().all(|c| c.status != CrdStatus::Unknown));
    assert!(review.version.is_some());
    assert!(
        review
            .notes
            .iter()
            .any(|n| n.text.contains("minKubeVersion"))
    );
}

#[tokio::test]
#[ignore]
async fn approve_moves_the_install_forward() {
    let client = client().await;
    let Some((plan, _)) = pending_manual_plan(&client).await else {
        println!(
            "nothing waits for approval in kubyl-manual: run script/olm-dev.sh --delete && script/olm-dev.sh"
        );
        return;
    };
    let target = plan.csv_names[0].clone();
    ops::approve(client.clone(), plan.namespace.clone(), plan.name.clone())
        .await
        .unwrap();
    wait_for(
        &format!("{target} to succeed"),
        Duration::from_secs(600),
        || {
            let client = client.clone();
            let target = target.clone();
            async move {
                let olm = olm(&client).await;
                let operators = join::join(&olm.subscriptions, &olm.csvs, &olm.plans);
                operators.iter().any(|o| {
                    o.csv.as_ref().is_some_and(|c| c.name == target)
                        && o.status == OperatorStatus::Succeeded
                })
            }
        },
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn install_cert_manager_and_create_a_certificate() {
    let client = client().await;
    let packages = hub::fetch(client.clone()).await.unwrap();
    let package = packages.iter().find(|p| p.name == "cert-manager").unwrap();
    let head = package.head().unwrap().clone();
    let olm_state = olm(&client).await;
    if !olm_state
        .subscriptions
        .iter()
        .any(|s| s.package == "cert-manager")
    {
        let choice = InstallChoice {
            package: package.name.clone(),
            catalog: package.catalog.clone(),
            catalog_namespace: package.catalog_namespace.clone(),
            channel: head.name.clone(),
            starting_csv: None,
            approval: Approval::Automatic,
            target: Target::AllNamespaces,
        };
        let steps = ops::plan_install(
            &choice,
            &head.install_modes,
            &olm_state.groups,
            &namespaces(&client).await,
            &olm_state.subscriptions,
        )
        .unwrap();
        println!("install steps: {steps:?}");
        assert_eq!(steps.namespace, "operators");
        ops::install(client.clone(), choice, steps).await.unwrap();
    }
    wait_for(
        "cert-manager's CSV to succeed",
        Duration::from_secs(900),
        || {
            let client = client.clone();
            async move {
                let olm = olm(&client).await;
                join::join(&olm.subscriptions, &olm.csvs, &olm.plans)
                    .iter()
                    .any(|o| {
                        o.package() == Some("cert-manager") && o.status == OperatorStatus::Succeeded
                    })
            }
        },
    )
    .await;
    // A self-signed Issuer and a Certificate from the operator's alm-examples.
    let olm_state = olm(&client).await;
    let csv = olm_state
        .csvs
        .iter()
        .find(|c| c.name.starts_with("cert-manager."))
        .unwrap();
    let examples = csv.examples();
    let example = |kind: &str| {
        examples
            .iter()
            .find(|e| e["kind"] == kind)
            .cloned()
            .unwrap_or_else(|| panic!("no {kind} example"))
    };
    let ns = "kubyl-live-certs";
    let namespaces_api: Api<Namespace> = Api::all(client.clone());
    let namespace: Namespace = serde_json::from_value(json!({"metadata": {"name": ns}})).unwrap();
    namespaces_api
        .create(&PostParams::default(), &namespace)
        .await
        .ok();
    let issuer_resource = resource("cert-manager.io", "v1", "Issuer", "issuers");
    let cert_resource = resource("cert-manager.io", "v1", "Certificate", "certificates");
    let mut issuer = example("Issuer");
    issuer["metadata"]["namespace"] = json!(ns);
    let mut certificate = example("Certificate");
    certificate["metadata"]["namespace"] = json!(ns);
    // The example's issuer is a placeholder: point it at the self-signed one.
    certificate["spec"]["issuerRef"] =
        json!({"name": issuer["metadata"]["name"], "kind": "Issuer"});
    let issuers: Api<DynamicObject> = Api::namespaced_with(client.clone(), ns, &issuer_resource);
    let certificates: Api<DynamicObject> = Api::namespaced_with(client.clone(), ns, &cert_resource);
    let name = certificate["metadata"]["name"]
        .as_str()
        .unwrap()
        .to_string();
    issuers
        .create(
            &PostParams::default(),
            &serde_json::from_value(issuer).unwrap(),
        )
        .await
        .ok();
    certificates
        .create(
            &PostParams::default(),
            &serde_json::from_value(certificate).unwrap(),
        )
        .await
        .ok();
    wait_for(
        "the Certificate to be Ready",
        Duration::from_secs(180),
        || {
            let certificates = certificates.clone();
            let name = name.clone();
            async move {
                certificates.get(&name).await.ok().is_some_and(|c| {
                    let v = serde_json::to_value(c).unwrap();
                    v["status"]["conditions"].as_array().is_some_and(|c| {
                        c.iter()
                            .any(|c| c["type"] == "Ready" && c["status"] == "True")
                    })
                })
            }
        },
    )
    .await;
    namespaces_api
        .delete(ns, &DeleteParams::default())
        .await
        .ok();
}

#[tokio::test]
#[ignore]
async fn uninstall_removes_the_operator_and_its_crds() {
    let client = client().await;
    let ns = "kubyl-live-uninstall";
    let packages = hub::fetch(client.clone()).await.unwrap();
    let package = packages
        .iter()
        .find(|p| p.name == "ack-recyclebin-controller")
        .unwrap();
    let head = package.head().unwrap().clone();
    let crd_name = head.owned[0].name.clone();
    let olm_state = olm(&client).await;
    let choice = InstallChoice {
        package: package.name.clone(),
        catalog: package.catalog.clone(),
        catalog_namespace: package.catalog_namespace.clone(),
        channel: head.name.clone(),
        starting_csv: None,
        approval: Approval::Automatic,
        target: Target::Namespace(ns.into()),
    };
    if !olm_state.subscriptions.iter().any(|s| s.namespace == ns) {
        let steps = ops::plan_install(
            &choice,
            &head.install_modes,
            &olm_state.groups,
            &namespaces(&client).await,
            &olm_state.subscriptions,
        )
        .unwrap();
        assert!(steps.create_namespace);
        ops::install(client.clone(), choice, steps).await.unwrap();
    }
    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());
    wait_for(
        "the operator's CSV and CRD",
        Duration::from_secs(600),
        || {
            let client = client.clone();
            let crds = crds.clone();
            let crd_name = crd_name.clone();
            async move {
                let olm = olm(&client).await;
                olm.csvs.iter().any(|c| c.namespace == ns)
                    && crds.get_opt(&crd_name).await.ok().flatten().is_some()
            }
        },
    )
    .await;
    let olm_state = olm(&client).await;
    let sub = olm_state
        .subscriptions
        .iter()
        .find(|s| s.namespace == ns)
        .unwrap();
    let csv = olm_state.csvs.iter().find(|c| c.namespace == ns).unwrap();
    let crd = crds.get(&crd_name).await.unwrap();
    let version = crd
        .spec
        .versions
        .iter()
        .find(|v| v.storage)
        .unwrap()
        .name
        .clone();
    let owned = &csv.owned[0];
    let api_resource = resource(owned.group(), &version, &owned.kind, owned.plural());
    let (tx, rx) = std::sync::mpsc::channel();
    ops::uninstall(
        client.clone(),
        UninstallSpec {
            subscription: Some((sub.namespace.clone(), sub.name.clone())),
            csv: Some((csv.namespace.clone(), csv.name.clone())),
            crds: vec![(crd_name.clone(), api_resource, true)],
        },
        move |line| {
            tx.send(line).ok();
        },
    )
    .await
    .unwrap();
    println!("progress: {:?}", rx.try_iter().collect::<Vec<_>>());
    wait_for("the CRD to go", Duration::from_secs(120), || {
        let crds = crds.clone();
        let crd_name = crd_name.clone();
        async move { crds.get_opt(&crd_name).await.ok().flatten().is_none() }
    })
    .await;
    let olm_state = olm(&client).await;
    assert!(!olm_state.subscriptions.iter().any(|s| s.namespace == ns));
    Api::<Namespace>::all(client)
        .delete(ns, &DeleteParams::default())
        .await
        .ok();
}

#[tokio::test]
#[ignore]
async fn helm_releases_decode_with_values_and_manifest() {
    let client = client().await;
    let secrets: Api<Secret> = Api::all(client.clone());
    let metadata = secrets
        .list_metadata(&ListParams::default().labels("owner=helm"))
        .await
        .unwrap();
    let objects: Vec<(decode::Driver, Arc<Value>)> = metadata
        .items
        .iter()
        .map(|m| {
            (
                decode::Driver::Secret,
                Arc::new(serde_json::to_value(m).unwrap()),
            )
        })
        .collect();
    let releases = group(&objects);
    println!(
        "releases: {:?}",
        releases
            .iter()
            .map(|r| format!("{}/{} r{}", r.0, r.1, r.3[0].revision))
            .collect::<Vec<_>>()
    );
    let kps = releases
        .iter()
        .find(|r| r.1 == "kube-prometheus-stack")
        .expect("script/prometheus-dev.sh's release");
    let secret = Api::<Secret>::namespaced(client, &kps.0)
        .get(&kps.3[0].object)
        .await
        .unwrap();
    let encoded = secret.data.unwrap().remove("release").unwrap().0;
    let start = Instant::now();
    let release = decode::decode(&encoded).unwrap();
    println!("decoded {:?} in {:?}", release, start.elapsed());
    assert_eq!(release.summary.chart.name, "kube-prometheus-stack");
    assert_eq!(release.summary.revision, kps.3[0].revision);
    assert!(release.values.is_object());
    assert!(release.manifest.contains("kind: Deployment"));
    // Masked values show no string of the values.
    let masked: String = present::value_lines(&release.values, true)
        .iter()
        .flatten()
        .map(|s| s.text.clone())
        .collect();
    let mut strings = Vec::new();
    collect_strings(&release.values, &mut strings);
    for s in strings.iter().filter(|s| s.len() > 3) {
        assert!(!masked.contains(s.as_str()), "{s} is visible");
    }
    let objects = present::manifest_objects(&release.manifest);
    println!("{} objects in the manifest", objects.len());
    assert!(objects.len() > 20);
    let masked_manifest = present::mask_secrets(&release.manifest);
    if release.manifest.contains("kind: Secret") {
        assert!(masked_manifest.contains(present::MASK));
    }
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}
