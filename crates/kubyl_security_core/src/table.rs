//! The views as text: columns and CSV records, shared by the tables and the export.

use jiff::Timestamp;
use kubyl_resources_core::format;

use crate::aggregate::{ImageRow, ResourceRow, RoleRow};
use crate::details::Details;
use crate::kinds::View;
use crate::model::Counts;

/// The columns of a view, as `(id, title)`, in table and CSV order.
pub fn columns(view: View) -> Vec<(&'static str, &'static str)> {
    let mut columns = match view {
        View::Images => vec![("image", "Image"), ("workloads", "Workloads")],
        View::Resources => vec![("resource", "Resource"), ("namespace", "Namespace")],
        View::Roles => vec![("role", "Role"), ("namespace", "Namespace")],
    };
    columns.extend([
        ("critical", "Critical"),
        ("high", "High"),
        ("medium", "Medium"),
        ("low", "Low"),
    ]);
    match view {
        View::Images => columns.extend([("secrets", "Secrets"), ("scanner", "Scanner")]),
        View::Resources => columns.push(("secrets", "Secrets")),
        View::Roles => {}
    }
    columns.push(("age", "Age"));
    columns
}

pub fn header(view: View) -> Vec<String> {
    columns(view)
        .into_iter()
        .map(|(_, title)| title.to_string())
        .collect()
}

fn age(created: Option<Timestamp>, now: Timestamp) -> String {
    created
        .map(|t| format::human_duration(format::seconds_since(t, now)))
        .unwrap_or_default()
}

/// One cell of an image row.
pub fn image_cell(row: &ImageRow, column: &str, now: Timestamp) -> String {
    match column {
        "image" => row.image.clone(),
        "workloads" => row.workloads.len().to_string(),
        "secrets" => row.secrets.to_string(),
        "scanner" => row.scanner.clone(),
        "age" => age(row.created, now),
        severity => severity_cell(&row.counts, severity),
    }
}

pub fn resource_cell(row: &ResourceRow, column: &str, now: Timestamp) -> String {
    match column {
        "resource" => format!("{} {}", row.subject.kind, row.subject.name),
        "namespace" => row.subject.namespace.clone().unwrap_or_default(),
        "secrets" => row.secrets.to_string(),
        "age" => age(row.created, now),
        severity => severity_cell(&row.counts, severity),
    }
}

pub fn role_cell(row: &RoleRow, column: &str, now: Timestamp) -> String {
    match column {
        "role" => format!("{} {}", row.subject.kind, row.subject.name),
        "namespace" => row.subject.namespace.clone().unwrap_or_default(),
        "age" => age(row.created, now),
        severity => severity_cell(&row.counts, severity),
    }
}

fn severity_cell(counts: &Counts, column: &str) -> String {
    match column {
        "critical" => counts.critical,
        "high" => counts.high,
        "medium" => counts.medium,
        "low" => counts.low,
        _ => return String::new(),
    }
    .to_string()
}

pub fn image_records<'a>(
    rows: impl IntoIterator<Item = &'a ImageRow>,
    now: Timestamp,
) -> Vec<Vec<String>> {
    let columns = columns(View::Images);
    rows.into_iter()
        .map(|r| {
            columns
                .iter()
                .map(|(id, _)| image_cell(r, id, now))
                .collect()
        })
        .collect()
}

pub fn resource_records<'a>(
    rows: impl IntoIterator<Item = &'a ResourceRow>,
    now: Timestamp,
) -> Vec<Vec<String>> {
    let columns = columns(View::Resources);
    rows.into_iter()
        .map(|r| {
            columns
                .iter()
                .map(|(id, _)| resource_cell(r, id, now))
                .collect()
        })
        .collect()
}

pub fn role_records<'a>(
    rows: impl IntoIterator<Item = &'a RoleRow>,
    now: Timestamp,
) -> Vec<Vec<String>> {
    let columns = columns(View::Roles);
    rows.into_iter()
        .map(|r| {
            columns
                .iter()
                .map(|(id, _)| role_cell(r, id, now))
                .collect()
        })
        .collect()
}

/// The findings of one report as CSV (all of them, not the panel's cut). Exposed secrets have
/// rule, file and severity only.
pub fn details_csv(details: &Details) -> String {
    let mut records: Vec<Vec<String>> = Vec::new();
    let header: Vec<&str>;
    if !details.vulnerabilities.is_empty()
        || (details.checks.is_empty() && details.secrets.is_empty())
    {
        header = vec![
            "Severity",
            "Vulnerability",
            "Package",
            "Installed",
            "Fixed",
            "Score",
            "Title",
            "Link",
            "Target",
        ];
        for v in &details.vulnerabilities {
            records.push(vec![
                v.severity.label().into(),
                v.id.clone(),
                v.package.clone(),
                v.installed.clone(),
                v.fixed.clone(),
                v.score.map(|s| s.to_string()).unwrap_or_default(),
                v.title.clone(),
                v.link.clone().unwrap_or_default(),
                v.target.clone(),
            ]);
        }
    } else if !details.checks.is_empty() {
        header = vec![
            "Severity",
            "Check",
            "Title",
            "Category",
            "Messages",
            "Remediation",
        ];
        for c in &details.checks {
            records.push(vec![
                c.severity.label().into(),
                c.id.clone(),
                c.title.clone(),
                c.category.clone(),
                c.messages.join("; "),
                c.remediation.clone(),
            ]);
        }
    } else {
        header = vec!["Severity", "Rule", "Category", "Title", "File"];
        for s in &details.secrets {
            records.push(vec![
                s.severity.label().into(),
                s.rule.clone(),
                s.category.clone(),
                s.title.clone(),
                s.target.clone(),
            ]);
        }
    }
    kubyl_base::csv::write(&header, &records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate;
    use crate::details::{Check, Vulnerability};
    use crate::kinds::ReportKind;
    use crate::model::{Report, Severity, Subject};

    fn image_row() -> ImageRow {
        let report = Report {
            kind: ReportKind::Vulnerability,
            namespace: Some("shop".into()),
            name: "r".into(),
            subject: Subject {
                kind: "ReplicaSet".into(),
                name: "web-1".into(),
                namespace: Some("shop".into()),
            },
            container: None,
            image: Some("=cmd|' /C calc'!A0".into()),
            scanner: "Trivy".into(),
            created: "2026-10-01T10:00:00Z".parse().ok(),
            counts: Counts {
                critical: 2,
                high: 7,
                medium: 1,
                low: 0,
                unknown: 0,
            },
        };
        aggregate::images(&[report]).remove(0)
    }

    #[test]
    fn views_have_matching_columns_and_records() {
        let now: Timestamp = "2026-10-04T10:00:00Z".parse().unwrap();
        let rows = [image_row()];
        let records = image_records(&rows, now);
        assert_eq!(
            header(View::Images),
            [
                "Image",
                "Workloads",
                "Critical",
                "High",
                "Medium",
                "Low",
                "Secrets",
                "Scanner",
                "Age"
            ]
        );
        assert_eq!(
            records[0],
            [
                "=cmd|' /C calc'!A0",
                "1",
                "2",
                "7",
                "1",
                "0",
                "0",
                "Trivy",
                "3d"
            ]
        );
        // The writer guards the image name (it comes from the cluster).
        let csv = kubyl_base::csv::write(&header(View::Images), &records);
        assert!(csv.contains("\r\n'=cmd|"), "{csv}");
        assert_eq!(columns(View::Roles).len(), header(View::Roles).len());
        assert_eq!(columns(View::Resources).last().unwrap().0, "age");
    }

    #[test]
    fn details_export_as_csv_with_every_finding() {
        let details = Details {
            vulnerabilities: vec![Vulnerability {
                id: "CVE-1".into(),
                package: "libssl".into(),
                installed: "1".into(),
                fixed: "2".into(),
                severity: Severity::Critical,
                title: "a, b".into(),
                link: Some("https://avd.aquasec.com/nvd/cve-1".into()),
                score: Some(9.8),
                target: "t".into(),
            }],
            ..Default::default()
        };
        let csv = details_csv(&details);
        assert_eq!(
            csv,
            "Severity,Vulnerability,Package,Installed,Fixed,Score,Title,Link,Target\r\nCritical,CVE-1,libssl,1,2,9.8,\"a, b\",https://avd.aquasec.com/nvd/cve-1,t\r\n"
        );
        let checks = Details {
            checks: vec![Check {
                id: "KSV017".into(),
                title: "Privileged".into(),
                description: String::new(),
                severity: Severity::High,
                category: "Kubernetes Security Check".into(),
                messages: vec!["one".into(), "two".into()],
                remediation: "=do not".into(),
            }],
            ..Default::default()
        };
        let csv = details_csv(&checks);
        assert!(csv.starts_with("Severity,Check,Title,Category,Messages,Remediation\r\n"));
        assert!(csv.contains("one; two,'=do not"), "{csv}");
    }
}
