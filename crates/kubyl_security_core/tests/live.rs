//! The Security Center's data against Trivy Operator on the kind dev cluster. Ignored by default:
//!
//! ```sh
//! script/dev-cluster.sh
//! script/trivy-dev.sh          # or --fixtures when the vulnerability database can't be pulled
//! kind get kubeconfig --name kubyl-dev > /tmp/kubyl-dev.yaml
//! KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev.yaml \
//!   cargo test -p kubyl_security_core --test live -- --ignored --nocapture
//! ```
//!
//! Read-only. Reads the way Kubyl does: the list as the API server's printer-column table, then
//! one report object by name for its findings. Waits up to three minutes for the operator's
//! first scans.

use std::time::Duration;

use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use kubyl_security_core::kinds::{GROUP, ReportKind};
use kubyl_security_core::model::{Report, Severity};
use kubyl_security_core::{aggregate, details, table};
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

/// The list rows of a kind in all namespaces, from its table (with the objects' labels).
async fn reports(client: &Client, kind: ReportKind) -> Vec<Report> {
    let request = http::Request::get(format!(
        "/apis/{GROUP}/v1alpha1/{}?includeObject=Metadata",
        kind.plural()
    ))
    .header(
        http::header::ACCEPT,
        "application/json;as=Table;v=v1;g=meta.k8s.io",
    )
    .body(Vec::new())
    .unwrap();
    let table: Value = client.request(request).await.unwrap();
    let columns: Vec<String> = table["columnDefinitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    table["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            let cells = row["cells"].as_array().unwrap();
            Report::parse(kind, &row["object"], |column| {
                columns
                    .iter()
                    .position(|c| c == column)
                    .and_then(|ix| cells.get(ix))
            })
        })
        .collect()
}

async fn all(client: &Client) -> Vec<Report> {
    let mut all = Vec::new();
    for kind in ReportKind::ALL {
        all.extend(reports(client, kind).await);
    }
    all
}

async fn full_report(client: &Client, report: &Report) -> Value {
    let base = match &report.namespace {
        Some(ns) => format!(
            "/apis/{GROUP}/v1alpha1/namespaces/{ns}/{}",
            report.kind.plural()
        ),
        None => format!("/apis/{GROUP}/v1alpha1/{}", report.kind.plural()),
    };
    let request = http::Request::get(format!("{base}/{}", report.name))
        .body(Vec::new())
        .unwrap();
    client.request(request).await.unwrap()
}

#[tokio::test]
#[ignore = "needs a cluster with script/trivy-dev.sh"]
async fn the_sample_workloads_show_up_in_every_view() {
    let client = client().await;
    // The first scans take a while (the vulnerability database is downloaded first).
    let mut reports = Vec::new();
    for _ in 0..36 {
        reports = all(&client).await;
        let scanned = aggregate::images(&reports)
            .iter()
            .any(|i| i.image.ends_with("nginx:1.19") && i.counts.total() > 0);
        if scanned
            && aggregate::roles(&reports)
                .iter()
                .any(|r| r.subject.name == "everything")
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    let images = aggregate::images(&reports);
    for image in &images {
        println!(
            "{:<24} crit {:>3} high {:>3} med {:>3} low {:>3} secrets {} workloads {}",
            image.image,
            image.counts.critical,
            image.counts.high,
            image.counts.medium,
            image.counts.low,
            image.secrets,
            image.workloads.len()
        );
    }
    let nginx = images
        .iter()
        .find(|i| i.image.ends_with("nginx:1.19"))
        .expect("nginx:1.19 scanned: run script/trivy-dev.sh and wait for the scan jobs");
    assert!(nginx.counts.critical > 0 && nginx.counts.high > 0);
    assert!(
        nginx
            .workloads
            .iter()
            .any(|w| w.subject.namespace.as_deref() == Some("kubyl-trivy"))
    );
    assert!(aggregate::critical_total(&reports) >= nginx.counts.critical);

    // Findings of one report: fetched by name, worst first, one per counted finding.
    let report = reports
        .iter()
        .find(|r| r.kind == ReportKind::Vulnerability && r.image.as_deref() == Some(&nginx.image))
        .unwrap();
    let found = details::parse(
        ReportKind::Vulnerability,
        &full_report(&client, report).await,
    );
    println!(
        "{} findings, first {:?}",
        found.len(),
        found.vulnerabilities.first().map(|v| &v.id)
    );
    assert_eq!(found.vulnerabilities.len() as u32, report.counts.total());
    assert_eq!(found.vulnerabilities[0].severity, Severity::Critical);
    assert_eq!(
        table::details_csv(&found).lines().count(),
        found.vulnerabilities.len() + 1
    );

    let resources = aggregate::resources(&reports);
    let privileged = resources
        .iter()
        .find(|r| r.subject.name == "privileged")
        .expect("the privileged pod has a config audit");
    assert!(privileged.counts.high > 0);
    let audit = reports
        .iter()
        .find(|r| r.kind == ReportKind::ConfigAudit && r.subject.name == "privileged")
        .unwrap();
    let checks = details::parse(ReportKind::ConfigAudit, &full_report(&client, audit).await);
    assert!(!checks.checks.is_empty());

    let roles = aggregate::roles(&reports);
    let everything = roles
        .iter()
        .find(|r| r.subject.name == "everything")
        .expect("the wildcard Role is assessed");
    assert!(everything.counts.critical > 0);
}

#[tokio::test]
#[ignore = "needs a cluster with script/trivy-dev.sh"]
async fn exposed_secret_reports_never_show_text() {
    let client = client().await;
    let reports = reports(&client, ReportKind::ExposedSecret).await;
    for report in &reports {
        let object = full_report(&client, report).await;
        let found = details::parse(ReportKind::ExposedSecret, &object);
        let text = format!("{found:?}") + &table::details_csv(&found);
        // Whatever the report matched, its text isn't in what Kubyl keeps.
        for secret in object["report"]["secrets"].as_array().into_iter().flatten() {
            if let Some(matched) = secret["match"].as_str().filter(|m| m.len() > 6) {
                assert!(!text.contains(matched), "the secret's text leaked: {text}");
            }
        }
    }
    println!("{} exposed-secret reports checked", reports.len());
}
