//! VerticalPodAutoscalers (`autoscaling.k8s.io/v1`, a CRD of kubernetes/autoscaler): the
//! target workload, the update mode and the recommendation per container, next to the requests
//! of the target's pod template.

use serde_json::Value;

use crate::dra::{Condition, conditions};
use crate::format::{array_at, str_at};

/// The API group.
pub const GROUP: &str = "autoscaling.k8s.io";

/// The workload a VPA scales.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub api_version: String,
    pub kind: String,
    pub name: String,
}

impl Target {
    /// `(group, version)` of `api_version`.
    pub fn group_version(&self) -> (&str, &str) {
        match self.api_version.split_once('/') {
            Some((group, version)) => (group, version),
            None => ("", self.api_version.as_str()),
        }
    }

    /// The plural of a well-known target kind (others go through discovery).
    pub fn resource(&self) -> Option<&'static str> {
        Some(match self.kind.as_str() {
            "Deployment" => "deployments",
            "StatefulSet" => "statefulsets",
            "DaemonSet" => "daemonsets",
            "ReplicaSet" => "replicasets",
            "Job" => "jobs",
            "CronJob" => "cronjobs",
            "ReplicationController" => "replicationcontrollers",
            _ => return None,
        })
    }
}

/// CPU and memory of one bound.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resources {
    pub cpu: String,
    pub memory: String,
}

impl Resources {
    fn parse(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        Some(Self {
            cpu: str_at(value, "/cpu").to_string(),
            memory: str_at(value, "/memory").to_string(),
        })
    }

    /// `80m / 96Mi`.
    pub fn label(&self) -> String {
        let dash = |s: &str| {
            if s.is_empty() {
                "—".to_string()
            } else {
                s.to_string()
            }
        };
        format!("{} / {}", dash(&self.cpu), dash(&self.memory))
    }
}

/// The recommendation for one container.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recommendation {
    pub container: String,
    pub lower: Option<Resources>,
    pub target: Option<Resources>,
    pub upper: Option<Resources>,
    pub uncapped: Option<Resources>,
}

/// A VerticalPodAutoscaler.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vpa {
    pub target: Option<Target>,
    /// `Off`, `Initial`, `Recreate`, `InPlaceOrRecreate`, `Auto` (the default).
    pub update_mode: String,
    pub recommendations: Vec<Recommendation>,
    /// `(container, mode)` of the container policies (`*` for all containers).
    pub policies: Vec<(String, String)>,
    pub conditions: Vec<Condition>,
}

impl Vpa {
    pub fn parse(vpa: &Value) -> Self {
        Self {
            target: vpa.pointer("/spec/targetRef").map(|t| Target {
                api_version: str_at(t, "/apiVersion").to_string(),
                kind: str_at(t, "/kind").to_string(),
                name: str_at(t, "/name").to_string(),
            }),
            update_mode: match str_at(vpa, "/spec/updatePolicy/updateMode") {
                "" => "Auto".into(),
                mode => mode.to_string(),
            },
            recommendations: array_at(vpa, "/status/recommendation/containerRecommendations")
                .iter()
                .map(|r| Recommendation {
                    container: str_at(r, "/containerName").to_string(),
                    lower: Resources::parse(r.get("lowerBound")),
                    target: Resources::parse(r.get("target")),
                    upper: Resources::parse(r.get("upperBound")),
                    uncapped: Resources::parse(r.get("uncappedTarget")),
                })
                .collect(),
            policies: array_at(vpa, "/spec/resourcePolicy/containerPolicies")
                .iter()
                .map(|p| {
                    (
                        str_at(p, "/containerName").to_string(),
                        match str_at(p, "/mode") {
                            "" => "Auto".to_string(),
                            m => m.to_string(),
                        },
                    )
                })
                .collect(),
            conditions: conditions(vpa, "/status/conditions"),
        }
    }

    /// `Deployment/web`.
    pub fn target_label(&self) -> Option<String> {
        let target = self.target.as_ref()?;
        Some(format!("{}/{}", target.kind, target.name))
    }

    /// `nginx 80m / 96Mi, sidecar 10m / 32Mi`.
    pub fn recommendation_label(&self) -> String {
        self.recommendations
            .iter()
            .filter_map(|r| {
                r.target
                    .as_ref()
                    .map(|t| format!("{} {}", r.container, t.label()))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The mode of a container's policy (`Off` means the VPA leaves it alone).
    pub fn container_mode(&self, container: &str) -> Option<&str> {
        self.policies
            .iter()
            .find(|(c, _)| c == container)
            .or_else(|| self.policies.iter().find(|(c, _)| c == "*"))
            .map(|(_, mode)| mode.as_str())
    }
}

/// The requests of each container of a workload's pod template: `(container, requests)`.
pub fn template_requests(workload: &Value) -> Vec<(String, Resources)> {
    let containers = [
        "/spec/template/spec/containers",
        "/spec/jobTemplate/spec/template/spec/containers",
    ]
    .iter()
    .map(|p| array_at(workload, p))
    .find(|c| !c.is_empty())
    .unwrap_or_default();
    containers
        .iter()
        .map(|c| {
            (
                str_at(c, "/name").to_string(),
                Resources {
                    cpu: str_at(c, "/resources/requests/cpu").to_string(),
                    memory: str_at(c, "/resources/requests/memory").to_string(),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recommendations_and_targets() {
        let vpa = json!({"apiVersion": "autoscaling.k8s.io/v1", "kind": "VerticalPodAutoscaler",
            "metadata": {"name": "web"},
            "spec": {"targetRef": {"apiVersion": "apps/v1", "kind": "Deployment", "name": "web"},
                "updatePolicy": {"updateMode": "Off"},
                "resourcePolicy": {"containerPolicies": [{"containerName": "metrics", "mode": "Off"},
                                                         {"containerName": "*"}]}},
            "status": {"conditions": [{"type": "RecommendationProvided", "status": "True"}],
                "recommendation": {"containerRecommendations": [{"containerName": "nginx",
                    "lowerBound": {"cpu": "25m", "memory": "48Mi"}, "target": {"cpu": "80m", "memory": "96Mi"},
                    "upperBound": {"cpu": "320m", "memory": "256Mi"}}]}}});
        let parsed = Vpa::parse(&vpa);
        assert_eq!(parsed.target_label().as_deref(), Some("Deployment/web"));
        let target = parsed.target.as_ref().unwrap();
        assert_eq!(target.group_version(), ("apps", "v1"));
        assert_eq!(target.resource(), Some("deployments"));
        assert_eq!(parsed.update_mode, "Off");
        assert_eq!(parsed.recommendation_label(), "nginx 80m / 96Mi");
        assert_eq!(parsed.container_mode("metrics"), Some("Off"));
        assert_eq!(parsed.container_mode("nginx"), Some("Auto"));
        assert!(parsed.conditions[0].is_true());

        // No status yet, default mode.
        let fresh = Vpa::parse(
            &json!({"spec": {"targetRef": {"apiVersion": "batch/v1", "kind": "CronJob", "name": "c"}}}),
        );
        assert_eq!(fresh.update_mode, "Auto");
        assert_eq!(fresh.recommendation_label(), "");
        assert_eq!(
            Resources {
                cpu: "1".into(),
                memory: String::new()
            }
            .label(),
            "1 / —"
        );

        let cronjob = json!({"spec": {"jobTemplate": {"spec": {"template": {"spec": {"containers": [
            {"name": "app", "resources": {"requests": {"cpu": "100m"}}}]}}}}}});
        assert_eq!(template_requests(&cronjob)[0].1.cpu, "100m");
    }
}
