//! A full report's findings, parsed from the object fetched when a row is opened.
//!
//! Reports can be large (an old image has thousands of vulnerabilities), so the parser keeps
//! only the fields the panel and the CSV show, and [`Details::shown`] caps what the panel lists.
//!
//! **Exposed secrets are masked by construction.** A `secrets[]` entry carries the matched text
//! in `match`; [`SecretFinding`] has no field for it and the parser never reads it, so the text
//! can't reach the panel, the CSV, a log or a `Debug` print.

use kubyl_resources_core::format::{array_at, str_at};
use serde_json::Value;

use crate::kinds::ReportKind;
use crate::model::Severity;

/// What the panel lists at most; the CSV has everything.
pub const SHOWN: usize = 300;

#[derive(Clone, Debug, PartialEq)]
pub struct Vulnerability {
    pub id: String,
    /// The package.
    pub package: String,
    pub installed: String,
    pub fixed: String,
    pub severity: Severity,
    pub title: String,
    /// A `http(s)` link to the advisory.
    pub link: Option<String>,
    pub score: Option<f64>,
    pub target: String,
}

/// A failed check of a config audit or RBAC assessment.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub id: String,
    pub title: String,
    pub description: String,
    pub severity: Severity,
    pub category: String,
    pub messages: Vec<String>,
    pub remediation: String,
}

/// An exposed secret: which rule matched where. There is no field for the secret's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretFinding {
    pub rule: String,
    pub category: String,
    pub severity: Severity,
    pub title: String,
    /// The file the match is in.
    pub target: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Details {
    pub vulnerabilities: Vec<Vulnerability>,
    pub checks: Vec<Check>,
    pub secrets: Vec<SecretFinding>,
    /// What the report says it scanned, e.g. `nginx:1.19 (debian 10.13)`.
    pub artifact: String,
    pub os: String,
}

impl Details {
    /// How many findings the report holds.
    pub fn len(&self) -> usize {
        self.vulnerabilities.len() + self.checks.len() + self.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the panel cuts the list.
    pub fn truncated(&self) -> bool {
        self.vulnerabilities.len() > SHOWN
            || self.checks.len() > SHOWN
            || self.secrets.len() > SHOWN
    }
}

/// Only a web link counts as one.
fn web_link(link: &str) -> Option<String> {
    (link.starts_with("https://") || link.starts_with("http://")).then(|| link.to_string())
}

fn text(value: &Value, pointer: &str) -> String {
    str_at(value, pointer).to_string()
}

/// The findings of a report object of `kind`: worst first.
pub fn parse(kind: ReportKind, report: &Value) -> Details {
    let body = report.get("report").unwrap_or(&Value::Null);
    let mut details = Details::default();
    let artifact = body.get("artifact");
    if let Some(artifact) = artifact {
        let repository = str_at(artifact, "/repository");
        let tag = str_at(artifact, "/tag");
        details.artifact = match (repository.is_empty(), tag.is_empty()) {
            (true, _) => String::new(),
            (false, true) => repository.to_string(),
            (false, false) => format!("{repository}:{tag}"),
        };
    }
    let os = body.get("os");
    if let Some(os) = os {
        details.os = format!("{} {}", str_at(os, "/family"), str_at(os, "/name"))
            .trim()
            .to_string();
    }
    match kind {
        ReportKind::Vulnerability => {
            details.vulnerabilities = array_at(body, "/vulnerabilities")
                .iter()
                .map(|v| Vulnerability {
                    id: text(v, "/vulnerabilityID"),
                    package: text(v, "/resource"),
                    installed: text(v, "/installedVersion"),
                    fixed: text(v, "/fixedVersion"),
                    severity: Severity::parse(str_at(v, "/severity")),
                    title: text(v, "/title"),
                    link: web_link(str_at(v, "/primaryLink")),
                    score: v.get("score").and_then(Value::as_f64),
                    target: text(v, "/target"),
                })
                .collect();
            details.vulnerabilities.sort_by(|a, b| {
                a.severity
                    .cmp(&b.severity)
                    .then(
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
                    .then_with(|| a.id.cmp(&b.id))
                    .then_with(|| a.package.cmp(&b.package))
            });
        }
        ReportKind::ConfigAudit
        | ReportKind::RbacAssessment
        | ReportKind::ClusterRbacAssessment => {
            details.checks = array_at(body, "/checks")
                .iter()
                // Passing checks aren't findings.
                .filter(|c| c.get("success").and_then(Value::as_bool) != Some(true))
                .map(|c| Check {
                    id: text(c, "/checkID"),
                    title: text(c, "/title"),
                    description: text(c, "/description"),
                    severity: Severity::parse(str_at(c, "/severity")),
                    category: text(c, "/category"),
                    messages: array_at(c, "/messages")
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    remediation: text(c, "/remediation"),
                })
                .collect();
            details
                .checks
                .sort_by(|a, b| a.severity.cmp(&b.severity).then_with(|| a.id.cmp(&b.id)));
        }
        ReportKind::ExposedSecret => {
            details.secrets = array_at(body, "/secrets")
                .iter()
                // `match` (the secret's text) is deliberately not read.
                .map(|s| SecretFinding {
                    rule: text(s, "/ruleID"),
                    category: text(s, "/category"),
                    severity: Severity::parse(str_at(s, "/severity")),
                    title: text(s, "/title"),
                    target: text(s, "/target"),
                })
                .collect();
            details.secrets.sort_by(|a, b| {
                a.severity
                    .cmp(&b.severity)
                    .then_with(|| a.rule.cmp(&b.rule))
            });
        }
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FAKE_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
    const FAKE_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn vulnerability(id: &str, severity: &str, score: f64) -> Value {
        json!({"vulnerabilityID": id, "resource": "libssl1.1", "installedVersion": "1.1.1n", "fixedVersion": "1.1.1w",
            "severity": severity, "title": "openssl: something", "primaryLink": "https://avd.aquasec.com/nvd/cve",
            "score": score, "target": "nginx:1.19 (debian 10.13)"})
    }

    #[test]
    fn vulnerabilities_sort_worst_first_and_links_must_be_web_links() {
        let mut bad_link = vulnerability("CVE-4", "HIGH", 7.0);
        bad_link["primaryLink"] = json!("javascript:alert(1)");
        let report = json!({"report": {
            "artifact": {"repository": "library/nginx", "tag": "1.19"},
            "os": {"family": "debian", "name": "10.13"},
            "vulnerabilities": [vulnerability("CVE-1", "LOW", 3.0), vulnerability("CVE-2", "CRITICAL", 9.8),
                vulnerability("CVE-3", "HIGH", 8.1), bad_link, json!({"vulnerabilityID": "CVE-5", "severity": "WEIRD"})]}});
        let details = parse(ReportKind::Vulnerability, &report);
        let ids: Vec<_> = details
            .vulnerabilities
            .iter()
            .map(|v| v.id.as_str())
            .collect();
        assert_eq!(ids, ["CVE-2", "CVE-3", "CVE-4", "CVE-1", "CVE-5"]);
        assert_eq!(details.artifact, "library/nginx:1.19");
        assert_eq!(details.os, "debian 10.13");
        assert_eq!(details.vulnerabilities[0].fixed, "1.1.1w");
        assert_eq!(details.vulnerabilities[2].link, None);
        assert!(details.vulnerabilities[0].link.is_some());
        assert_eq!(details.vulnerabilities[4].severity, Severity::Unknown);
        assert_eq!(details.len(), 5);
    }

    #[test]
    fn config_audits_list_only_failed_checks() {
        let report = json!({"report": {"checks": [
            {"checkID": "KSV017", "title": "Privileged", "severity": "HIGH", "success": false, "messages": ["Container 'shell' of Pod 'privileged' should set privileged to false"],
             "remediation": "Change 'containers[].securityContext.privileged' to 'false'", "category": "Kubernetes Security Check", "description": "Privileged containers share namespaces"},
            {"checkID": "KSV001", "title": "Process can elevate its own privileges", "severity": "MEDIUM", "success": false, "messages": []},
            {"checkID": "KSV003", "title": "Default capabilities", "severity": "LOW", "success": true},
            {"checkID": "KSV999", "title": "No success field", "severity": "CRITICAL"}]}});
        let details = parse(ReportKind::ConfigAudit, &report);
        let ids: Vec<_> = details.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["KSV999", "KSV017", "KSV001"]);
        assert_eq!(details.checks[1].messages.len(), 1);
        assert!(details.checks[1].remediation.contains("privileged"));
        let rbac = parse(ReportKind::RbacAssessment, &report);
        assert_eq!(rbac.checks.len(), 3);
    }

    #[test]
    fn exposed_secrets_never_carry_the_secret() {
        let report = json!({"report": {"secrets": [
            {"ruleID": "aws-access-key-id", "category": "AWS", "severity": "CRITICAL", "title": "AWS Access Key ID",
             "target": "/root/.aws-credentials", "match": format!("aws_access_key_id = {FAKE_KEY}")},
            {"ruleID": "aws-secret-access-key", "category": "AWS", "severity": "CRITICAL", "title": "AWS Secret Access Key",
             "target": "/root/.aws-credentials", "match": format!("aws_secret_access_key = {FAKE_SECRET}")}]}});
        let details = parse(ReportKind::ExposedSecret, &report);
        assert_eq!(details.secrets.len(), 2);
        assert_eq!(details.secrets[0].rule, "aws-access-key-id");
        assert_eq!(details.secrets[0].target, "/root/.aws-credentials");
        // Not in the parsed value, however it's printed or exported.
        let printed = format!("{details:?} {details:#?}");
        assert!(
            !printed.contains(FAKE_KEY) && !printed.contains(FAKE_SECRET),
            "{printed}"
        );
        let csv = crate::table::details_csv(&details);
        assert!(
            !csv.contains(FAKE_KEY) && !csv.contains(FAKE_SECRET),
            "{csv}"
        );
    }

    #[test]
    fn large_reports_are_cut_in_the_panel_only() {
        let many: Vec<Value> = (0..SHOWN + 50)
            .map(|i| vulnerability(&format!("CVE-{i}"), "LOW", 1.0))
            .collect();
        let details = parse(
            ReportKind::Vulnerability,
            &json!({"report": {"vulnerabilities": many}}),
        );
        assert!(details.truncated());
        assert_eq!(
            details.vulnerabilities.len(),
            SHOWN + 50,
            "the CSV keeps all"
        );
        assert!(!parse(ReportKind::Vulnerability, &json!({})).truncated());
        assert!(parse(ReportKind::Vulnerability, &json!({})).is_empty());
    }
}
