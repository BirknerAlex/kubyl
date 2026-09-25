//! Metrics against a real cluster. Ignored by default; run against the kind dev cluster:
//!
//! ```sh
//! script/dev-cluster.sh && script/prometheus-dev.sh
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig \
//!   cargo test -p kubyl_metrics --test live -- --ignored --nocapture
//! ```
//!
//! Without Prometheus (`script/prometheus-dev.sh --metrics-server-only`), set
//! `KUBYL_TEST_PROMETHEUS=absent`: discovery must then find nothing and metrics-server must
//! still answer.

use std::collections::HashSet;
use std::time::Duration;

use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_charts::TimeRange;
use kubyl_metrics::discover::{self, Found};
use kubyl_metrics::metrics_server;
use kubyl_metrics::panels;
use kubyl_metrics::prometheus::{PromClient, PromError, Target};
use kubyl_metrics::queries::{LIBRARY, Queries};

async fn client() -> Client {
    let path = std::env::var("KUBYL_TEST_KUBECONFIG").expect("set KUBYL_TEST_KUBECONFIG");
    let kubeconfig = Kubeconfig::read_from(path).unwrap();
    let options = KubeConfigOptions {
        context: Some(std::env::var("KUBYL_TEST_CONTEXT").unwrap_or("kind-kubyl-dev".into())),
        ..Default::default()
    };
    let config = Config::from_custom_kubeconfig(kubeconfig, &options)
        .await
        .unwrap();
    Client::try_from(config).unwrap()
}

fn prometheus_expected() -> bool {
    std::env::var("KUBYL_TEST_PROMETHEUS").as_deref() != Ok("absent")
}

async fn prometheus(client: &Client) -> PromClient {
    match discover::discover(client).await {
        Found::Prometheus(prom) => prom,
        Found::Nothing { best } => panic!("no Prometheus found: {best:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn discovery_matches_the_cluster() {
    let client = client().await;
    let found = discover::discover(&client).await;
    match (prometheus_expected(), found) {
        (true, Found::Prometheus(prom)) => {
            println!("found {}", prom.target().label());
            assert_eq!(
                prom.target().label(),
                "monitoring/kube-prometheus-stack-prometheus"
            );
        }
        (false, Found::Nothing { best }) => println!("nothing found, as expected: {best:?}"),
        (expected, found) => panic!("expected Prometheus: {expected}, got {found:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn a_missing_service_reports_why() {
    let client = client().await;
    let target = Target::service("monitoring", "no-such-prometheus", "9090");
    match discover::probe(&client, vec![target]).await {
        Found::Nothing {
            best: Some((_, PromError::Http(code, _))),
        } => assert_eq!(code, 404),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
#[ignore]
async fn every_library_query_runs() {
    if !prometheus_expected() {
        return;
    }
    let client = client().await;
    let prom = prometheus(&client).await;
    let rules: HashSet<String> = prom.metric_names().await.unwrap().into_iter().collect();
    println!("{} metric names", rules.len());
    assert!(
        rules.contains("node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m"),
        "kube-prometheus-stack ships recording rules"
    );
    for queries in [
        Queries::default(),
        Queries::new(Default::default(), rules.clone()),
    ] {
        for def in LIBRARY {
            let promql = queries.render(def.id, &[]).unwrap();
            let samples = prom
                .query(&promql)
                .await
                .unwrap_or_else(|e| panic!("{}: {e}\n{promql}", def.id));
            println!("{:>18}: {} series", def.id, samples.len());
        }
    }
    // Pods of the dev workloads have usage.
    let queries = Queries::new(Default::default(), rules);
    let promql = queries
        .render("pod_cpu", &[("namespace", "payments")])
        .unwrap();
    let pods = prom.query(&promql).await.unwrap();
    assert!(
        pods.iter().any(|s| s
            .labels
            .get("pod")
            .is_some_and(|p| p.starts_with("checkout-api"))),
        "{pods:?}"
    );
    let promql = queries.render("node_memory", &[]).unwrap();
    assert!(prom.query(&promql).await.unwrap().len() >= 2);
}

/// The details panels get data for real objects: a pod, a deployment's pods, a namespace and a
/// node (panels whose metric the Prometheus lacks are skipped, like in the app).
#[tokio::test]
#[ignore]
async fn detail_panels_have_data() {
    if !prometheus_expected() {
        return;
    }
    let client = client().await;
    let prom = prometheus(&client).await;
    let names: HashSet<String> = prom.metric_names().await.unwrap().into_iter().collect();
    let queries = Queries::new(Default::default(), names.clone());
    let pods: kube::Api<k8s_openapi::api::core::v1::Pod> =
        kube::Api::namespaced(client.clone(), "payments");
    let pod = pods
        .list(&kube::api::ListParams::default().labels("app=checkout-api"))
        .await
        .unwrap()
        .items
        .remove(0);
    let pod_name = pod.metadata.name.unwrap();
    let object = |resource: &str, ns: Option<&str>, name: &str| {
        kubyl_core::ResourceRef::object(
            kubyl_core::ClusterId::new("kind"),
            kubyl_core::Gvr::new("", "v1", resource),
            ns.map(str::to_string),
            name.to_string(),
        )
    };
    let cases = [
        (object("pods", Some("payments"), &pod_name), "Pod"),
        (
            object("deployments", Some("payments"), "checkout-api"),
            "Deployment",
        ),
        (object("namespaces", None, "payments"), "Namespace"),
        (object("nodes", None, "kubyl-dev-worker"), "Node"),
    ];
    let (start, end, step) = TimeRange::M15.window(kubyl_metrics::service::now());
    for (target, kind) in cases {
        let (defs, filters) = panels::for_object(&target, kind).unwrap();
        let filters: Vec<(&str, &str)> = filters
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        for def in defs.iter().filter(|d| names.contains(d.needs)) {
            for series in def.series {
                let promql = queries.render(series.query, &filters).unwrap();
                let result = prom
                    .query_range(&promql, start, end, step)
                    .await
                    .unwrap_or_else(|e| panic!("{kind}/{}: {e}\n{promql}", def.id));
                let points: usize = result.iter().map(|s| s.values.len()).sum();
                println!(
                    "{kind:>10} {:>10} {:>22}: {points} points",
                    def.id, series.name
                );
                // Traffic, CPU and memory always exist; drops, OOM kills… may legitimately be
                // absent or zero, but must still be valid queries.
                if matches!(def.id, "network" | "cpu" | "memory") {
                    assert!(points > 0, "{kind}/{}: no data\n{promql}", def.id);
                }
            }
        }
    }
}

#[tokio::test]
#[ignore]
async fn range_queries_cover_the_window() {
    if !prometheus_expected() {
        return;
    }
    let client = client().await;
    let prom = prometheus(&client).await;
    let queries = Queries::default();
    for range in [TimeRange::M15, TimeRange::H1, TimeRange::D7] {
        let (start, end, step) = range.window(kubyl_metrics::service::now());
        let promql = queries.render("namespace_cpu", &[]).unwrap();
        let series = prom.query_range(&promql, start, end, step).await.unwrap();
        assert!(
            series.iter().any(|s| s.label("namespace") == "payments"),
            "{range:?}: {series:?}"
        );
        for s in &series {
            assert!(s.values.iter().all(|(t, _)| *t >= start && *t <= end));
        }
    }
}

#[tokio::test]
#[ignore]
async fn metrics_server_answers() {
    let client = client().await;
    let mut attempts = 0;
    let nodes = loop {
        match metrics_server::nodes(&client).await {
            Ok(nodes) if !nodes.is_empty() => break nodes,
            // A freshly installed metrics-server needs one scrape.
            other if attempts < 20 => {
                attempts += 1;
                println!("waiting for metrics-server: {other:?}");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            other => panic!("metrics-server: {other:?}"),
        }
    };
    assert!(nodes.contains_key("kubyl-dev-worker"), "{nodes:?}");
    let pods = metrics_server::pods(&client, Some("payments"))
        .await
        .unwrap();
    let checkout = pods
        .iter()
        .find(|(key, _)| key.starts_with("payments/checkout-api"))
        .expect("checkout-api pods");
    assert!(checkout.1.memory > 0.0);
    let all = metrics_server::pods(&client, None).await.unwrap();
    assert!(all.len() >= pods.len());
}

/// The pieces of the OpenShift path that work on any cluster: short-lived service-account
/// tokens, and a clean failure where there are no Routes.
#[tokio::test]
#[ignore]
async fn service_account_tokens_and_missing_routes() {
    use kubyl_metrics::openshift::{ServiceAccountToken, route_url, through_route};
    use secrecy::ExposeSecret as _;

    let client = client().await;
    let sa = ServiceAccountToken::new(client.clone(), "kube-system", "default");
    let token = sa.get().await.unwrap();
    assert!(token.expose_secret().split('.').count() == 3, "a JWT");
    // Cached until renewal.
    assert_eq!(
        sa.get().await.unwrap().expose_secret(),
        token.expose_secret()
    );

    assert_eq!(
        route_url(&client, "monitoring", "kube-prometheus-stack-prometheus").await,
        None
    );
    let target = Target::service("monitoring", "kube-prometheus-stack-prometheus", "9090");
    let err = through_route(&client, &target, None, None)
        .await
        .unwrap_err();
    assert!(err.contains("has no Route"), "{err}");
}
