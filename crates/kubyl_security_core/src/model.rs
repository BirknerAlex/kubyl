//! A report as a row: who it is about and how many findings of each severity it has.

use jiff::Timestamp;
use kubyl_resources_core::format;
use serde_json::Value;

use crate::kinds::ReportKind;

/// Trivy's severities, worst first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Unknown,
}

impl Severity {
    pub const ALL: [Severity; 5] = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::High => "High",
            Severity::Medium => "Medium",
            Severity::Low => "Low",
            Severity::Unknown => "Unknown",
        }
    }

    /// `CRITICAL`, `high`… (anything else is `Unknown`).
    pub fn parse(text: &str) -> Severity {
        match text.trim().to_ascii_uppercase().as_str() {
            "CRITICAL" => Severity::Critical,
            "HIGH" => Severity::High,
            "MEDIUM" => Severity::Medium,
            "LOW" => Severity::Low,
            _ => Severity::Unknown,
        }
    }
}

/// Findings per severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub unknown: u32,
}

impl Counts {
    pub fn get(&self, severity: Severity) -> u32 {
        match severity {
            Severity::Critical => self.critical,
            Severity::High => self.high,
            Severity::Medium => self.medium,
            Severity::Low => self.low,
            Severity::Unknown => self.unknown,
        }
    }

    pub fn total(&self) -> u32 {
        self.critical + self.high + self.medium + self.low + self.unknown
    }

    /// The worst severity with findings.
    pub fn worst(&self) -> Option<Severity> {
        Severity::ALL.into_iter().find(|s| self.get(*s) > 0)
    }

    /// Per severity the larger count (the same image scanned by several reports).
    pub fn max(self, other: Counts) -> Counts {
        Counts {
            critical: self.critical.max(other.critical),
            high: self.high.max(other.high),
            medium: self.medium.max(other.medium),
            low: self.low.max(other.low),
            unknown: self.unknown.max(other.unknown),
        }
    }

    /// Per severity the sum.
    pub fn plus(self, other: Counts) -> Counts {
        Counts {
            critical: self.critical + other.critical,
            high: self.high + other.high,
            medium: self.medium + other.medium,
            low: self.low + other.low,
            unknown: self.unknown + other.unknown,
        }
    }
}

/// The labels Trivy Operator puts on every report.
const SUBJECT_KIND: &str = "trivy-operator.resource.kind";
const SUBJECT_NAME: &str = "trivy-operator.resource.name";
const SUBJECT_NAMESPACE: &str = "trivy-operator.resource.namespace";
const CONTAINER: &str = "trivy-operator.container.name";

/// What a report is about: the workload (or Role) the operator scanned.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Subject {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

impl Subject {
    /// `Deployment shop/web`.
    pub fn label(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{} {ns}/{}", self.kind, self.name),
            None => format!("{} {}", self.kind, self.name),
        }
    }
}

/// One report, as a list row.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub kind: ReportKind,
    /// The report object's namespace (`None` for cluster reports) and name.
    pub namespace: Option<String>,
    pub name: String,
    pub subject: Subject,
    pub container: Option<String>,
    /// `repository:tag` of an image report.
    pub image: Option<String>,
    pub scanner: String,
    pub created: Option<Timestamp>,
    pub counts: Counts,
}

impl Report {
    /// A report from its metadata (labels, creation time) and the printer columns of its list
    /// row: `cell("Critical")`… as the server's table gave them.
    pub fn parse<'a>(
        kind: ReportKind,
        metadata_object: &Value,
        cell: impl Fn(&str) -> Option<&'a Value>,
    ) -> Option<Report> {
        let name = format::name(metadata_object);
        if name.is_empty() {
            return None;
        }
        let label = |key: &str| {
            metadata_object
                .pointer("/metadata/labels")
                .and_then(|l| l.get(key))
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        let number = |column: &str| {
            cell(column)
                .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
                .unwrap_or(0) as u32
        };
        let text = |column: &str| {
            cell(column)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty() && *v != "<none>")
                .map(str::to_string)
        };
        let namespace = format::namespace(metadata_object).map(str::to_string);
        let subject = Subject {
            kind: label(SUBJECT_KIND).unwrap_or_default(),
            name: label(SUBJECT_NAME).unwrap_or_else(|| name.to_string()),
            namespace: label(SUBJECT_NAMESPACE).or_else(|| namespace.clone()),
        };
        let image = kind
            .has_image()
            .then(|| {
                let repository = text("Repository")?;
                Some(match text("Tag") {
                    Some(tag) => format!("{repository}:{tag}"),
                    None => repository,
                })
            })
            .flatten();
        Some(Report {
            kind,
            namespace,
            name: name.to_string(),
            subject,
            container: label(CONTAINER),
            image,
            scanner: text("Scanner").unwrap_or_default(),
            created: format::creation(metadata_object),
            counts: Counts {
                critical: number("Critical"),
                high: number("High"),
                medium: number("Medium"),
                low: number("Low"),
                unknown: number("Unknown"),
            },
        })
    }

    /// `namespace/name` of the report object (`name` for cluster reports).
    pub fn key(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn parse(kind: ReportKind, meta: Value, cells: &[(&str, Value)]) -> Option<Report> {
        let cells: HashMap<&str, Value> = cells.iter().cloned().collect();
        Report::parse(kind, &meta, |c| cells.get(c))
    }

    #[test]
    fn a_vulnerability_report_row() {
        let meta = json!({"metadata": {"name": "replicaset-old-nginx-9d4-nginx", "namespace": "kubyl-trivy",
            "creationTimestamp": "2026-10-10T02:30:00Z",
            "labels": {"trivy-operator.resource.kind": "ReplicaSet", "trivy-operator.resource.name": "old-nginx-9d4",
                "trivy-operator.resource.namespace": "kubyl-trivy", "trivy-operator.container.name": "nginx"}}});
        let report = parse(
            ReportKind::Vulnerability,
            meta,
            &[
                ("Repository", json!("library/nginx")),
                ("Tag", json!("1.19")),
                ("Scanner", json!("Trivy")),
                ("Critical", json!(12)),
                ("High", json!(48)),
                ("Medium", json!("30")),
                ("Low", json!(0)),
                ("Unknown", json!(2)),
            ],
        )
        .unwrap();
        assert_eq!(report.image.as_deref(), Some("library/nginx:1.19"));
        assert_eq!(
            report.subject.label(),
            "ReplicaSet kubyl-trivy/old-nginx-9d4"
        );
        assert_eq!(report.container.as_deref(), Some("nginx"));
        assert_eq!(report.counts.total(), 92);
        assert_eq!(report.counts.worst(), Some(Severity::Critical));
        assert_eq!(report.key(), "kubyl-trivy/replicaset-old-nginx-9d4-nginx");
        assert_eq!(report.scanner, "Trivy");
    }

    #[test]
    fn missing_cells_count_as_zero_and_cluster_reports_have_no_namespace() {
        let meta = json!({"metadata": {"name": "clusterrole-admin",
            "labels": {"trivy-operator.resource.kind": "ClusterRole", "trivy-operator.resource.name": "admin"}}});
        let report = parse(
            ReportKind::ClusterRbacAssessment,
            meta,
            &[("High", json!(1))],
        )
        .unwrap();
        assert_eq!(
            report.counts,
            Counts {
                high: 1,
                ..Default::default()
            }
        );
        assert_eq!(report.namespace, None);
        assert_eq!(report.subject.label(), "ClusterRole admin");
        assert_eq!(report.image, None);
        assert!(parse(ReportKind::ConfigAudit, json!({"metadata": {}}), &[]).is_none());
    }

    #[test]
    fn config_audits_ignore_image_columns() {
        let meta = json!({"metadata": {"name": "pod-x", "namespace": "n"}});
        let report = parse(
            ReportKind::ConfigAudit,
            meta,
            &[("Repository", json!("nginx"))],
        )
        .unwrap();
        assert_eq!(report.image, None);
        // Without the operator's labels the report's own name stands in.
        assert_eq!(report.subject.name, "pod-x");
        assert_eq!(report.subject.namespace.as_deref(), Some("n"));
    }

    #[test]
    fn severities() {
        assert_eq!(Severity::parse("CRITICAL"), Severity::Critical);
        assert_eq!(Severity::parse(" low "), Severity::Low);
        assert_eq!(Severity::parse("whatever"), Severity::Unknown);
        let a = Counts {
            critical: 1,
            high: 5,
            ..Default::default()
        };
        let b = Counts {
            critical: 3,
            high: 2,
            ..Default::default()
        };
        assert_eq!(
            a.max(b),
            Counts {
                critical: 3,
                high: 5,
                ..Default::default()
            }
        );
        assert_eq!(a.plus(b).critical, 4);
        assert_eq!(Counts::default().worst(), None);
        let mut all = Severity::ALL;
        all.sort();
        assert_eq!(all[0], Severity::Critical);
    }
}
