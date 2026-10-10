//! The parsers against reports recorded from Trivy Operator (chart 0.35 era, Trivy 0.75) on the
//! kind dev cluster by `script/trivy-dev.sh`: `vulnerabilityreport-nginx.json` (nginx:1.19; the
//! real summary, a few findings of each severity kept), `configauditreport-privileged.json`,
//! `rbacassessmentreport-everything.json`, `exposedsecretreport-empty.json` (busybox) and
//! `table-*.json` (the API server's printer-column tables). `exposedsecretreport-leaky.json`
//! is hand-made in the schema Trivy writes (the operator can't scan an image loaded into kind).

use kubyl_security_core::aggregate::{self, Filter};
use kubyl_security_core::details::{self, Details};
use kubyl_security_core::kinds::ReportKind;
use kubyl_security_core::model::{Counts, Report, Severity};
use kubyl_security_core::table;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap()
}

/// The list rows of a recorded table, as the Security Center builds them.
fn reports(kind: ReportKind, table: &str, objects: &[&str]) -> Vec<Report> {
    let table = fixture(table);
    let columns: Vec<&str> = table["columnDefinitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let objects: Vec<Value> = objects.iter().map(|o| fixture(o)).collect();
    table["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| {
            let name = row["object"]["metadata"]["name"].as_str().unwrap();
            let cells = row["cells"].as_array().unwrap();
            // The recorded objects carry the labels; the table's rows only their metadata.
            let meta = objects
                .iter()
                .find(|o| o["metadata"]["name"] == name)
                .unwrap_or(&row["object"]);
            Report::parse(kind, meta, |column| {
                columns
                    .iter()
                    .position(|c| *c == column)
                    .and_then(|ix| cells.get(ix))
            })
        })
        .collect()
}

#[test]
fn the_vulnerability_table_gives_the_real_counts() {
    let rows = reports(
        ReportKind::Vulnerability,
        "table-vulnerabilityreports.json",
        &["vulnerabilityreport-nginx.json"],
    );
    let nginx = rows.iter().find(|r| r.name.contains("old-nginx")).unwrap();
    assert_eq!(nginx.image.as_deref(), Some("library/nginx:1.19"));
    assert_eq!(
        nginx.counts,
        Counts {
            critical: 42,
            high: 143,
            medium: 175,
            low: 28,
            unknown: 36
        }
    );
    assert_eq!(nginx.subject.kind, "ReplicaSet");
    assert_eq!(nginx.container.as_deref(), Some("nginx"));
    let images = aggregate::images(&rows);
    assert_eq!(
        images[0].image, "library/nginx:1.19",
        "the worst image first"
    );
    assert_eq!(images.last().unwrap().counts, Counts::default());
    assert!(images[0].matches(&Filter {
        severity: Some(Severity::Critical),
        ..Filter::default()
    }));
    assert!(!images[1].matches(&Filter {
        severity: Some(Severity::Critical),
        ..Filter::default()
    }));
}

#[test]
fn the_nginx_report_lists_worst_first_and_exports_everything() {
    let details = details::parse(
        ReportKind::Vulnerability,
        &fixture("vulnerabilityreport-nginx.json"),
    );
    assert_eq!(details.artifact, "library/nginx:1.19");
    assert!(details.os.contains("debian"), "{}", details.os);
    assert_eq!(details.vulnerabilities[0].severity, Severity::Critical);
    let severities: Vec<Severity> = details.vulnerabilities.iter().map(|v| v.severity).collect();
    assert!(
        severities.windows(2).all(|w| w[0] <= w[1]),
        "sorted worst first"
    );
    assert!(
        details
            .vulnerabilities
            .iter()
            .all(|v| !v.id.is_empty() && !v.package.is_empty())
    );
    assert!(
        details
            .vulnerabilities
            .iter()
            .filter_map(|v| v.link.as_ref())
            .all(|l| l.starts_with("https://"))
    );
    let csv = table::details_csv(&details);
    assert_eq!(csv.lines().count(), details.vulnerabilities.len() + 1);
}

#[test]
fn the_privileged_pod_fails_checks() {
    let details = details::parse(
        ReportKind::ConfigAudit,
        &fixture("configauditreport-privileged.json"),
    );
    assert!(
        details
            .checks
            .iter()
            .any(|c| c.title.to_lowercase().contains("privileged"))
    );
    assert_eq!(
        details.checks.len(),
        19,
        "every check the operator recorded (it keeps the failed ones)"
    );
    assert!(details.checks[0].severity <= details.checks.last().unwrap().severity);
    let rows = reports(
        ReportKind::ConfigAudit,
        "table-configauditreports.json",
        &["configauditreport-privileged.json"],
    );
    let resources = aggregate::resources(&rows);
    assert_eq!(resources.len(), 3);
    assert!(
        resources
            .iter()
            .any(|r| r.subject.name == "privileged" && r.counts.high == 4)
    );
}

#[test]
fn rbac_assessments_and_exposed_secrets() {
    let rbac = details::parse(
        ReportKind::RbacAssessment,
        &fixture("rbacassessmentreport-everything.json"),
    );
    assert!(!rbac.checks.is_empty());
    assert_eq!(rbac.checks[0].severity, Severity::Critical);

    assert!(
        details::parse(
            ReportKind::ExposedSecret,
            &fixture("exposedsecretreport-empty.json")
        )
        .is_empty()
    );
    let leaky_json = fixture("exposedsecretreport-leaky.json");
    let leaky = details::parse(ReportKind::ExposedSecret, &leaky_json);
    assert_eq!(leaky.secrets.len(), 2);
    assert_eq!(leaky.secrets[0].rule, "aws-access-key-id");
    // The matched text (even Trivy's censored form) is never read.
    let text = format!("{leaky:?}") + &table::details_csv(&leaky);
    assert!(
        !text.contains("********") && !text.contains("aws_access_key_id ="),
        "{text}"
    );
    assert!(
        leaky_json["report"]["secrets"][0]["match"].is_string(),
        "the fixture does carry a match"
    );
    let _: Details = leaky;
}
