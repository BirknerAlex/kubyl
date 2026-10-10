//! The Trivy Operator report kinds the Security Center reads.

use kubyl_base::TrivyCaps;

/// The API group of every report.
pub const GROUP: &str = "aquasecurity.github.io";

/// A report CRD.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReportKind {
    Vulnerability,
    ConfigAudit,
    ExposedSecret,
    RbacAssessment,
    ClusterRbacAssessment,
}

impl ReportKind {
    pub const ALL: [ReportKind; 5] = [
        ReportKind::Vulnerability,
        ReportKind::ConfigAudit,
        ReportKind::ExposedSecret,
        ReportKind::RbacAssessment,
        ReportKind::ClusterRbacAssessment,
    ];

    pub fn kind(self) -> &'static str {
        match self {
            ReportKind::Vulnerability => "VulnerabilityReport",
            ReportKind::ConfigAudit => "ConfigAuditReport",
            ReportKind::ExposedSecret => "ExposedSecretReport",
            ReportKind::RbacAssessment => "RbacAssessmentReport",
            ReportKind::ClusterRbacAssessment => "ClusterRbacAssessmentReport",
        }
    }

    pub fn plural(self) -> &'static str {
        match self {
            ReportKind::Vulnerability => "vulnerabilityreports",
            ReportKind::ConfigAudit => "configauditreports",
            ReportKind::ExposedSecret => "exposedsecretreports",
            ReportKind::RbacAssessment => "rbacassessmentreports",
            ReportKind::ClusterRbacAssessment => "clusterrbacassessmentreports",
        }
    }

    pub fn namespaced(self) -> bool {
        self != ReportKind::ClusterRbacAssessment
    }

    /// Whether the cluster serves this kind.
    pub fn served(self, caps: &TrivyCaps) -> bool {
        match self {
            ReportKind::Vulnerability => caps.vulnerabilities,
            ReportKind::ConfigAudit => caps.config_audit,
            ReportKind::ExposedSecret => caps.exposed_secrets,
            ReportKind::RbacAssessment => caps.rbac,
            ReportKind::ClusterRbacAssessment => caps.cluster_rbac,
        }
    }

    /// Whether the report names an image (`Repository` and `Tag` printer columns).
    pub fn has_image(self) -> bool {
        matches!(self, ReportKind::Vulnerability | ReportKind::ExposedSecret)
    }

    pub fn from_plural(plural: &str) -> Option<ReportKind> {
        ReportKind::ALL.into_iter().find(|k| k.plural() == plural)
    }
}

/// The three views.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Images,
    Resources,
    Roles,
}

impl View {
    pub const ALL: [View; 3] = [View::Images, View::Resources, View::Roles];

    pub fn label(self) -> &'static str {
        match self {
            View::Images => "Images",
            View::Resources => "Resources",
            View::Roles => "Roles",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            View::Images => "images",
            View::Resources => "resources",
            View::Roles => "roles",
        }
    }

    pub fn from_id(id: &str) -> Option<View> {
        View::ALL.into_iter().find(|v| v.id() == id)
    }

    /// The report kinds the view is made of.
    pub fn kinds(self) -> &'static [ReportKind] {
        match self {
            View::Images => &[ReportKind::Vulnerability, ReportKind::ExposedSecret],
            View::Resources => &[ReportKind::ConfigAudit, ReportKind::ExposedSecret],
            View::Roles => &[
                ReportKind::RbacAssessment,
                ReportKind::ClusterRbacAssessment,
            ],
        }
    }

    /// Whether the cluster serves anything the view shows.
    pub fn served(self, caps: &TrivyCaps) -> bool {
        match self {
            View::Images => caps.vulnerabilities,
            View::Resources => caps.config_audit,
            View::Roles => caps.rbac || caps.cluster_rbac,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_and_views() {
        for kind in ReportKind::ALL {
            assert_eq!(ReportKind::from_plural(kind.plural()), Some(kind));
        }
        let caps = TrivyCaps {
            vulnerabilities: true,
            ..Default::default()
        };
        assert!(View::Images.served(&caps) && !View::Resources.served(&caps));
        assert!(!View::Roles.served(&caps));
        assert!(View::Roles.served(&TrivyCaps {
            cluster_rbac: true,
            ..Default::default()
        }));
        assert!(ReportKind::Vulnerability.served(&caps) && !ReportKind::ConfigAudit.served(&caps));
        assert!(!ReportKind::ClusterRbacAssessment.namespaced());
        assert_eq!(View::from_id("roles"), Some(View::Roles));
    }
}
