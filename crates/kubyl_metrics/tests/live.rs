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
use kubyl_metrics::prometheus::{PromClient, PromError, Target};
use kubyl_metrics::queries::{LIBRARY, Queries, RULE_PROBE};

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
    let rules: HashSet<String> = prom
        .query(RULE_PROBE)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|s| s.labels.get("__name__").cloned())
        .collect();
    println!("recording rules: {rules:?}");
    assert!(
        !rules.is_empty(),
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
