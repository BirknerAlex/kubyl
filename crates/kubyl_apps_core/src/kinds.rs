//! The kinds that make up an application.

/// A kind that can be a member of an application. Pods and ReplicaSets are left out: they
/// carry their workload's labels, and counting them would only repeat it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Deployment,
    StatefulSet,
    DaemonSet,
    CronJob,
    Job,
    Service,
    Ingress,
    ConfigMap,
    PersistentVolumeClaim,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Deployment,
        Kind::StatefulSet,
        Kind::DaemonSet,
        Kind::CronJob,
        Kind::Job,
        Kind::Service,
        Kind::Ingress,
        Kind::ConfigMap,
        Kind::PersistentVolumeClaim,
    ];

    pub fn kind(self) -> &'static str {
        match self {
            Kind::Deployment => "Deployment",
            Kind::StatefulSet => "StatefulSet",
            Kind::DaemonSet => "DaemonSet",
            Kind::CronJob => "CronJob",
            Kind::Job => "Job",
            Kind::Service => "Service",
            Kind::Ingress => "Ingress",
            Kind::ConfigMap => "ConfigMap",
            Kind::PersistentVolumeClaim => "PersistentVolumeClaim",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Kind::Deployment | Kind::StatefulSet | Kind::DaemonSet => "apps",
            Kind::CronJob | Kind::Job => "batch",
            Kind::Ingress => "networking.k8s.io",
            Kind::Service | Kind::ConfigMap | Kind::PersistentVolumeClaim => "",
        }
    }

    /// The version these kinds have been served at for years.
    pub fn version(self) -> &'static str {
        "v1"
    }

    pub fn plural(self) -> &'static str {
        match self {
            Kind::Deployment => "deployments",
            Kind::StatefulSet => "statefulsets",
            Kind::DaemonSet => "daemonsets",
            Kind::CronJob => "cronjobs",
            Kind::Job => "jobs",
            Kind::Service => "services",
            Kind::Ingress => "ingresses",
            Kind::ConfigMap => "configmaps",
            Kind::PersistentVolumeClaim => "persistentvolumeclaims",
        }
    }

    /// Workloads decide an application's health; the rest only belong to it.
    pub fn is_workload(self) -> bool {
        matches!(
            self,
            Kind::Deployment | Kind::StatefulSet | Kind::DaemonSet | Kind::CronJob | Kind::Job
        )
    }

    /// Whether Kubyl needs more than the object's metadata: workloads for their status. The
    /// rest is watched as metadata only, which keeps ConfigMap contents out of memory.
    pub fn needs_full_object(self) -> bool {
        self.is_workload()
    }

    /// Kinds the logs view can stream.
    pub fn has_logs(self) -> bool {
        matches!(
            self,
            Kind::Deployment | Kind::StatefulSet | Kind::DaemonSet | Kind::Job
        )
    }

    pub fn from_plural(group: &str, plural: &str) -> Option<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.group() == group && k.plural() == plural)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_plural(kind.group(), kind.plural()), Some(kind));
        }
        assert_eq!(Kind::from_plural("", "pods"), None);
        assert!(Kind::Deployment.needs_full_object() && !Kind::ConfigMap.needs_full_object());
    }
}
