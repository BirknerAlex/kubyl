//! The rows of the three views, built from the reports' list rows.
//!
//! - **Images**: vulnerability reports by image (`repository:tag`). The same image in several
//!   workloads is one row that lists them; its counts are the largest of its reports (they are
//!   the same findings, scanned at different times). Exposed-secret findings of the image are a
//!   count next to them.
//! - **Resources**: config audits by workload, with the exposed secrets found in its images.
//! - **Roles**: RBAC assessments of Roles and ClusterRoles.
//!
//! Rows sort by their worst findings first.

use std::collections::BTreeMap;

use jiff::Timestamp;

use crate::kinds::ReportKind;
use crate::model::{Counts, Report, Severity, Subject};

/// Where a row's full report lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReportRef {
    pub kind: ReportKind,
    pub namespace: Option<String>,
    pub name: String,
}

impl ReportRef {
    fn of(report: &Report) -> Self {
        Self {
            kind: report.kind,
            namespace: report.namespace.clone(),
            name: report.name.clone(),
        }
    }
}

/// A workload that runs an image.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Workload {
    pub subject: Subject,
    pub container: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageRow {
    pub image: String,
    pub counts: Counts,
    /// Exposed-secret findings in the image (never their text).
    pub secrets: u32,
    pub workloads: Vec<Workload>,
    pub scanner: String,
    /// The newest of its reports.
    pub created: Option<Timestamp>,
    pub vulnerability_reports: Vec<ReportRef>,
    pub secret_reports: Vec<ReportRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceRow {
    pub subject: Subject,
    pub counts: Counts,
    pub secrets: u32,
    pub created: Option<Timestamp>,
    pub config_report: Option<ReportRef>,
    pub secret_reports: Vec<ReportRef>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RoleRow {
    pub subject: Subject,
    pub counts: Counts,
    pub created: Option<Timestamp>,
    pub report: ReportRef,
}

fn newest(a: Option<Timestamp>, b: Option<Timestamp>) -> Option<Timestamp> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Worst first: more criticals, then highs, mediums, lows.
fn severity_key(counts: &Counts) -> (u32, u32, u32, u32) {
    (counts.critical, counts.high, counts.medium, counts.low)
}

pub fn images(reports: &[Report]) -> Vec<ImageRow> {
    let mut rows: BTreeMap<String, ImageRow> = BTreeMap::new();
    for report in reports {
        let Some(image) = &report.image else {
            continue;
        };
        let row = rows.entry(image.clone()).or_insert_with(|| ImageRow {
            image: image.clone(),
            counts: Counts::default(),
            secrets: 0,
            workloads: Vec::new(),
            scanner: String::new(),
            created: None,
            vulnerability_reports: Vec::new(),
            secret_reports: Vec::new(),
        });
        match report.kind {
            ReportKind::Vulnerability => {
                row.counts = row.counts.max(report.counts);
                row.vulnerability_reports.push(ReportRef::of(report));
                if row.scanner.is_empty() {
                    row.scanner = report.scanner.clone();
                }
            }
            ReportKind::ExposedSecret => {
                row.secrets = row.secrets.max(report.counts.total());
                row.secret_reports.push(ReportRef::of(report));
            }
            _ => continue,
        }
        let workload = Workload {
            subject: report.subject.clone(),
            container: report.container.clone(),
        };
        if !row.workloads.contains(&workload) {
            row.workloads.push(workload);
        }
        row.created = newest(row.created, report.created);
    }
    let mut rows: Vec<ImageRow> = rows.into_values().collect();
    for row in &mut rows {
        row.workloads.sort();
    }
    rows.sort_by(|a, b| {
        severity_key(&b.counts)
            .cmp(&severity_key(&a.counts))
            .then(b.secrets.cmp(&a.secrets))
            .then(a.image.cmp(&b.image))
    });
    rows
}

pub fn resources(reports: &[Report]) -> Vec<ResourceRow> {
    let mut rows: BTreeMap<Subject, ResourceRow> = BTreeMap::new();
    let entry = |rows: &mut BTreeMap<Subject, ResourceRow>, report: &Report| {
        rows.entry(report.subject.clone())
            .or_insert_with(|| ResourceRow {
                subject: report.subject.clone(),
                counts: Counts::default(),
                secrets: 0,
                created: None,
                config_report: None,
                secret_reports: Vec::new(),
            })
            .clone()
    };
    for report in reports {
        match report.kind {
            ReportKind::ConfigAudit | ReportKind::ExposedSecret => {}
            _ => continue,
        }
        let mut row = entry(&mut rows, report);
        if report.kind == ReportKind::ConfigAudit {
            row.counts = report.counts;
            row.config_report = Some(ReportRef::of(report));
        } else {
            row.secrets += report.counts.total();
            row.secret_reports.push(ReportRef::of(report));
        }
        row.created = newest(row.created, report.created);
        rows.insert(report.subject.clone(), row);
    }
    // Resources that only have exposed secrets (no config audit yet) stay listed.
    let mut rows: Vec<ResourceRow> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        severity_key(&b.counts)
            .cmp(&severity_key(&a.counts))
            .then(b.secrets.cmp(&a.secrets))
            .then(a.subject.cmp(&b.subject))
    });
    rows
}

pub fn roles(reports: &[Report]) -> Vec<RoleRow> {
    let mut rows: Vec<RoleRow> = reports
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                ReportKind::RbacAssessment | ReportKind::ClusterRbacAssessment
            )
        })
        .map(|r| RoleRow {
            subject: r.subject.clone(),
            counts: r.counts,
            created: r.created,
            report: ReportRef::of(r),
        })
        .collect();
    rows.sort_by(|a, b| {
        severity_key(&b.counts)
            .cmp(&severity_key(&a.counts))
            .then(a.subject.cmp(&b.subject))
    });
    rows
}

/// The sum of the rows' counts: the severity summary above a view.
pub fn totals(counts: impl IntoIterator<Item = Counts>) -> Counts {
    counts
        .into_iter()
        .fold(Counts::default(), |sum, c| sum.plus(c))
}

/// The findings of each view, summed: what the overview card and the sidebar badge show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Vulnerabilities, each image once.
    pub images: Counts,
    /// Config audit findings of each resource.
    pub resources: Counts,
    /// RBAC findings of each role.
    pub roles: Counts,
}

impl Summary {
    /// All three views together.
    pub fn total(&self) -> Counts {
        self.images.plus(self.resources).plus(self.roles)
    }

    /// The critical findings across the views (the badge).
    pub fn critical(&self) -> u32 {
        self.total().critical
    }
}

/// The summary of `reports`, of one namespace when given (cluster-scoped reports, of
/// ClusterRoles, belong to no namespace and stay out of a namespace's summary).
pub fn summary(reports: &[Report], namespace: Option<&str>) -> Summary {
    let scoped: Vec<Report> = reports
        .iter()
        .filter(|r| namespace.is_none_or(|ns| r.subject.namespace.as_deref() == Some(ns)))
        .cloned()
        .collect();
    Summary {
        images: totals(images(&scoped).iter().map(|r| r.counts)),
        resources: totals(resources(&scoped).iter().map(|r| r.counts)),
        roles: totals(roles(&scoped).iter().map(|r| r.counts)),
    }
}

/// The critical findings across the Security Center's views (what the sidebar badge shows).
pub fn critical_total(reports: &[Report]) -> u32 {
    summary(reports, None).critical()
}

/// What a view's filter box and severity chips ask for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    /// Only rows with findings of this severity.
    pub severity: Option<Severity>,
    pub namespace: Option<String>,
}

impl Filter {
    /// `haystack` is the row's text (image, workloads, kind…), `namespaces` where it lives.
    pub fn matches<'a>(
        &self,
        counts: &Counts,
        haystack: &str,
        mut namespaces: impl Iterator<Item = Option<&'a str>>,
    ) -> bool {
        if self.severity.is_some_and(|s| counts.get(s) == 0) {
            return false;
        }
        if let Some(ns) = &self.namespace
            && !namespaces.any(|n| n == Some(ns.as_str()))
        {
            return false;
        }
        let haystack = haystack.to_lowercase();
        self.text
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

impl ImageRow {
    pub fn matches(&self, filter: &Filter) -> bool {
        let haystack = format!(
            "{} {}",
            self.image,
            self.workloads
                .iter()
                .map(|w| w.subject.label())
                .collect::<Vec<_>>()
                .join(" ")
        );
        filter.matches(
            &self.counts,
            &haystack,
            self.workloads
                .iter()
                .map(|w| w.subject.namespace.as_deref()),
        )
    }
}

impl ResourceRow {
    pub fn matches(&self, filter: &Filter) -> bool {
        filter.matches(
            &self.counts,
            &self.subject.label(),
            std::iter::once(self.subject.namespace.as_deref()),
        )
    }
}

impl RoleRow {
    pub fn matches(&self, filter: &Filter) -> bool {
        filter.matches(
            &self.counts,
            &self.subject.label(),
            std::iter::once(self.subject.namespace.as_deref()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
        kind: ReportKind,
        name: &str,
        subject: (&str, &str, &str),
        image: Option<&str>,
        counts: Counts,
    ) -> Report {
        Report {
            kind,
            namespace: Some(subject.2.to_string()),
            name: name.into(),
            subject: Subject {
                kind: subject.0.into(),
                name: subject.1.into(),
                namespace: Some(subject.2.into()),
            },
            container: Some("app".into()),
            image: image.map(str::to_string),
            scanner: "Trivy".into(),
            created: "2026-10-10T02:30:00Z".parse().ok(),
            counts,
        }
    }

    fn c(critical: u32, high: u32) -> Counts {
        Counts {
            critical,
            high,
            ..Default::default()
        }
    }

    #[test]
    fn images_aggregate_their_workloads() {
        let reports = [
            report(
                ReportKind::Vulnerability,
                "a",
                ("ReplicaSet", "web-1", "shop"),
                Some("nginx:1.19"),
                c(10, 40),
            ),
            // The same image in another namespace, scanned later and found a bit less.
            report(
                ReportKind::Vulnerability,
                "b",
                ("ReplicaSet", "edge-1", "edge"),
                Some("nginx:1.19"),
                c(9, 41),
            ),
            report(
                ReportKind::Vulnerability,
                "c",
                ("ReplicaSet", "db-1", "shop"),
                Some("postgres:16"),
                c(0, 3),
            ),
            report(
                ReportKind::ExposedSecret,
                "d",
                ("ReplicaSet", "web-1", "shop"),
                Some("nginx:1.19"),
                c(1, 0),
            ),
            // Only secrets: still an image.
            report(
                ReportKind::ExposedSecret,
                "e",
                ("ReplicaSet", "leaky-1", "shop"),
                Some("leaky:1"),
                c(0, 2),
            ),
            report(
                ReportKind::ConfigAudit,
                "f",
                ("ReplicaSet", "web-1", "shop"),
                None,
                c(1, 1),
            ),
        ];
        let rows = images(&reports);
        let names: Vec<_> = rows.iter().map(|r| r.image.as_str()).collect();
        assert_eq!(names, ["nginx:1.19", "postgres:16", "leaky:1"]);
        let nginx = &rows[0];
        assert_eq!(nginx.counts, c(10, 41));
        assert_eq!(nginx.secrets, 1);
        assert_eq!(nginx.workloads.len(), 2);
        assert_eq!(nginx.vulnerability_reports.len(), 2);
        assert_eq!(rows[2].counts, Counts::default());
        assert_eq!(rows[2].secrets, 2);
    }

    #[test]
    fn resources_join_config_audits_and_exposed_secrets() {
        let reports = [
            report(
                ReportKind::ConfigAudit,
                "a",
                ("ReplicaSet", "web-1", "shop"),
                None,
                c(1, 4),
            ),
            report(
                ReportKind::ConfigAudit,
                "b",
                ("Pod", "privileged", "shop"),
                None,
                c(3, 2),
            ),
            report(
                ReportKind::ExposedSecret,
                "c",
                ("ReplicaSet", "web-1", "shop"),
                Some("x:1"),
                c(0, 2),
            ),
            report(
                ReportKind::ExposedSecret,
                "d",
                ("ReplicaSet", "web-1", "shop"),
                Some("y:1"),
                c(1, 0),
            ),
            report(
                ReportKind::Vulnerability,
                "e",
                ("ReplicaSet", "web-1", "shop"),
                Some("x:1"),
                c(99, 99),
            ),
        ];
        let rows = resources(&reports);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject.name, "privileged");
        let web = &rows[1];
        assert_eq!(
            web.counts,
            c(1, 4),
            "vulnerabilities don't count as config findings"
        );
        assert_eq!(web.secrets, 3);
        assert_eq!(web.secret_reports.len(), 2);
        assert!(web.config_report.is_some());
    }

    #[test]
    fn roles_list_both_scopes_worst_first() {
        let mut cluster = report(
            ReportKind::ClusterRbacAssessment,
            "cr",
            ("ClusterRole", "admin", "x"),
            None,
            c(0, 5),
        );
        cluster.namespace = None;
        cluster.subject.namespace = None;
        let reports = [
            report(
                ReportKind::RbacAssessment,
                "r",
                ("Role", "everything", "shop"),
                None,
                c(2, 0),
            ),
            cluster,
            report(
                ReportKind::ConfigAudit,
                "f",
                ("Pod", "x", "shop"),
                None,
                c(9, 9),
            ),
        ];
        let rows = roles(&reports);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].subject.label(), "Role shop/everything");
        assert_eq!(rows[1].subject.label(), "ClusterRole admin");
        assert_eq!(rows[1].report.namespace, None);
        assert_eq!(totals(rows.iter().map(|r| r.counts)), c(2, 5));
    }

    #[test]
    fn the_badge_counts_each_image_once() {
        let reports = [
            report(
                ReportKind::Vulnerability,
                "a",
                ("ReplicaSet", "web-1", "shop"),
                Some("nginx:1.19"),
                c(10, 0),
            ),
            report(
                ReportKind::Vulnerability,
                "b",
                ("ReplicaSet", "edge-1", "edge"),
                Some("nginx:1.19"),
                c(10, 0),
            ),
            report(
                ReportKind::Vulnerability,
                "c",
                ("ReplicaSet", "db-1", "shop"),
                Some("postgres:16"),
                c(2, 9),
            ),
            report(
                ReportKind::ConfigAudit,
                "d",
                ("Pod", "p", "shop"),
                None,
                c(1, 0),
            ),
            report(
                ReportKind::RbacAssessment,
                "e",
                ("Role", "r", "shop"),
                None,
                c(3, 0),
            ),
            // Exposed secrets aren't vulnerabilities: they have their own column.
            report(
                ReportKind::ExposedSecret,
                "f",
                ("Pod", "p", "shop"),
                Some("x:1"),
                c(5, 0),
            ),
        ];
        assert_eq!(critical_total(&reports), 10 + 2 + 1 + 3);
        assert_eq!(critical_total(&[]), 0);
    }

    #[test]
    fn summaries_split_by_view_and_namespace() {
        let mut cluster_role = report(
            ReportKind::ClusterRbacAssessment,
            "cr",
            ("ClusterRole", "admin", "x"),
            None,
            c(4, 0),
        );
        cluster_role.subject.namespace = None;
        let reports = [
            report(
                ReportKind::Vulnerability,
                "a",
                ("ReplicaSet", "web-1", "shop"),
                Some("nginx:1.19"),
                c(10, 3),
            ),
            report(
                ReportKind::Vulnerability,
                "b",
                ("ReplicaSet", "db-1", "bank"),
                Some("pg:16"),
                c(1, 1),
            ),
            report(
                ReportKind::ConfigAudit,
                "d",
                ("Pod", "p", "shop"),
                None,
                c(2, 0),
            ),
            report(
                ReportKind::RbacAssessment,
                "e",
                ("Role", "r", "shop"),
                None,
                c(3, 0),
            ),
            cluster_role,
        ];
        let all = summary(&reports, None);
        assert_eq!(
            (
                all.images.critical,
                all.resources.critical,
                all.roles.critical
            ),
            (11, 2, 7)
        );
        assert_eq!(all.critical(), 20);
        assert_eq!(all.total().high, 4);
        let shop = summary(&reports, Some("shop"));
        assert_eq!(
            (
                shop.images.critical,
                shop.resources.critical,
                shop.roles.critical
            ),
            (10, 2, 3)
        );
        assert_eq!(summary(&reports, Some("nowhere")), Summary::default());
    }

    #[test]
    fn filters_by_text_severity_and_namespace() {
        let reports = [
            report(
                ReportKind::Vulnerability,
                "a",
                ("ReplicaSet", "web-1", "shop"),
                Some("nginx:1.19"),
                c(1, 0),
            ),
            report(
                ReportKind::Vulnerability,
                "b",
                ("ReplicaSet", "db-1", "bank"),
                Some("postgres:16"),
                c(0, 3),
            ),
        ];
        let rows = images(&reports);
        let all = Filter::default();
        assert!(rows.iter().all(|r| r.matches(&all)));
        let critical = Filter {
            severity: Some(Severity::Critical),
            ..Filter::default()
        };
        assert_eq!(rows.iter().filter(|r| r.matches(&critical)).count(), 1);
        let text = Filter {
            text: "POSTGRES bank".into(),
            ..Filter::default()
        };
        assert_eq!(rows.iter().filter(|r| r.matches(&text)).count(), 1);
        let ns = Filter {
            namespace: Some("shop".into()),
            ..Filter::default()
        };
        let hit: Vec<_> = rows
            .iter()
            .filter(|r| r.matches(&ns))
            .map(|r| r.image.as_str())
            .collect();
        assert_eq!(hit, ["nginx:1.19"]);
    }
}
