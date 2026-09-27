//! Pre-flight check results: pass, warn or fail, an explanation, details and a fix.

use crate::model::ObjectLink;

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckStatus {
    Fail,
    Warn,
    /// Couldn't check (403, no data source): the summary says what's missing.
    Unknown,
    Pass,
    /// Nothing to check here (e.g. no add-ons on this provider).
    Info,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            CheckStatus::Fail => "failed",
            CheckStatus::Warn => "warning",
            CheckStatus::Unknown => "not checked",
            CheckStatus::Pass => "passed",
            CheckStatus::Info => "info",
        }
    }
}

/// What fixes a finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Opens the object's details.
    Open { label: String, link: ObjectLink },
    /// Opens a Helm release (its latest storage object).
    OpenRelease {
        namespace: String,
        name: String,
        /// `sh.helm.release.v1.<name>.v<revision>`.
        object: String,
        secret: bool,
    },
    /// Copies a command (`oc patch …`, `helm mapkubeapis …`).
    Copy { label: String, text: String },
    /// Opens a page in the browser.
    Url { label: String, url: String },
    /// Opens Kubyl's Installed Operators.
    Operators,
}

impl Fix {
    pub fn label(&self) -> &str {
        match self {
            Fix::Open { label, .. } | Fix::Copy { label, .. } | Fix::Url { label, .. } => label,
            Fix::OpenRelease { .. } => "Open release",
            Fix::Operators => "Open operators",
        }
    }
}

/// One finding of a check (a PDB, a deprecated API, a release).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    pub status: CheckStatus,
    pub text: String,
    pub sub: Option<String>,
    pub fix: Option<Fix>,
}

impl Detail {
    pub fn new(status: CheckStatus, text: impl Into<String>) -> Self {
        Self {
            status,
            text: text.into(),
            sub: None,
            fix: None,
        }
    }

    pub fn with_sub(mut self, sub: impl Into<String>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    pub fn fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// One check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Stable id (`pdb`, `deprecated-apis`…), for keys and tests.
    pub id: &'static str,
    pub title: String,
    pub status: CheckStatus,
    /// One line: why it passed, warns or failed.
    pub summary: String,
    pub details: Vec<Detail>,
    /// The main fix (the row's button).
    pub fix: Option<Fix>,
}

impl Check {
    pub fn new(
        id: &'static str,
        title: impl Into<String>,
        status: CheckStatus,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id,
            title: title.into(),
            status,
            summary: summary.into(),
            details: Vec::new(),
            fix: None,
        }
    }

    pub fn details(mut self, details: Vec<Detail>) -> Self {
        self.details = details;
        self
    }

    pub fn fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// Counts for the summary line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub failed: usize,
    pub warnings: usize,
    pub unknown: usize,
    pub passed: usize,
}

impl Counts {
    pub fn of(checks: &[Check]) -> Self {
        let mut counts = Self::default();
        for check in checks {
            match check.status {
                CheckStatus::Fail => counts.failed += 1,
                CheckStatus::Warn => counts.warnings += 1,
                CheckStatus::Unknown => counts.unknown += 1,
                CheckStatus::Pass => counts.passed += 1,
                CheckStatus::Info => {}
            }
        }
        counts
    }

    /// `1 failed · 2 warnings · 5 passed`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        match self.warnings {
            0 => {}
            1 => parts.push("1 warning".into()),
            n => parts.push(format!("{n} warnings")),
        }
        if self.unknown > 0 {
            parts.push(format!("{} not checked", self.unknown));
        }
        if self.passed > 0 {
            parts.push(format!("{} passed", self.passed));
        }
        parts.join(" · ")
    }
}

/// Sorts checks for display: failures first, then warnings, unknown, passed, info.
pub fn sort(checks: &mut [Check]) {
    checks.sort_by_key(|c| c.status);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_order() {
        let mut checks = vec![
            Check::new("a", "A", CheckStatus::Pass, ""),
            Check::new("b", "B", CheckStatus::Warn, ""),
            Check::new("c", "C", CheckStatus::Fail, ""),
            Check::new("d", "D", CheckStatus::Warn, ""),
            Check::new("e", "E", CheckStatus::Info, ""),
        ];
        sort(&mut checks);
        assert_eq!(checks[0].id, "c");
        assert_eq!(checks.last().unwrap().id, "e");
        assert_eq!(
            Counts::of(&checks).summary(),
            "1 failed · 2 warnings · 1 passed"
        );
    }
}
