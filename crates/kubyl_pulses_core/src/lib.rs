//! Pulses (k9s `:pulses`): for the main kinds of a cluster, how many objects are healthy and how
//! many are faulty, sampled over time while the view is open.

use std::collections::VecDeque;

use serde_json::Value;

/// What a tile counts, in k9s's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Nodes,
    Namespaces,
    Services,
    Events,
    Pods,
    Deployments,
    StatefulSets,
    DaemonSets,
    Jobs,
    CronJobs,
    PersistentVolumes,
    PersistentVolumeClaims,
    HorizontalPodAutoscalers,
    Ingresses,
    NetworkPolicies,
    ServiceAccounts,
}

impl Kind {
    pub const ALL: [Kind; 16] = [
        Kind::Nodes,
        Kind::Namespaces,
        Kind::Services,
        Kind::Events,
        Kind::Pods,
        Kind::Deployments,
        Kind::StatefulSets,
        Kind::DaemonSets,
        Kind::Jobs,
        Kind::CronJobs,
        Kind::PersistentVolumes,
        Kind::PersistentVolumeClaims,
        Kind::HorizontalPodAutoscalers,
        Kind::Ingresses,
        Kind::NetworkPolicies,
        Kind::ServiceAccounts,
    ];

    /// The kinds k9s shows when a single namespace is active: the cluster-wide ones (the first
    /// four) are left out.
    pub fn shown(namespaced_view: bool) -> impl Iterator<Item = Kind> {
        Kind::ALL
            .into_iter()
            .skip(if namespaced_view { 4 } else { 0 })
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Nodes => "Nodes",
            Kind::Namespaces => "Namespaces",
            Kind::Services => "Services",
            Kind::Events => "Events",
            Kind::Pods => "Pods",
            Kind::Deployments => "Deployments",
            Kind::StatefulSets => "StatefulSets",
            Kind::DaemonSets => "DaemonSets",
            Kind::Jobs => "Jobs",
            Kind::CronJobs => "CronJobs",
            Kind::PersistentVolumes => "PersistentVolumes",
            Kind::PersistentVolumeClaims => "PersistentVolumeClaims",
            Kind::HorizontalPodAutoscalers => "HorizontalPodAutoscalers",
            Kind::Ingresses => "Ingresses",
            Kind::NetworkPolicies => "NetworkPolicies",
            Kind::ServiceAccounts => "ServiceAccounts",
        }
    }

    /// API group and plural.
    pub fn api(self) -> (&'static str, &'static str) {
        match self {
            Kind::Nodes => ("", "nodes"),
            Kind::Namespaces => ("", "namespaces"),
            Kind::Services => ("", "services"),
            Kind::Events => ("", "events"),
            Kind::Pods => ("", "pods"),
            Kind::Deployments => ("apps", "deployments"),
            Kind::StatefulSets => ("apps", "statefulsets"),
            Kind::DaemonSets => ("apps", "daemonsets"),
            Kind::Jobs => ("batch", "jobs"),
            Kind::CronJobs => ("batch", "cronjobs"),
            Kind::PersistentVolumes => ("", "persistentvolumes"),
            Kind::PersistentVolumeClaims => ("", "persistentvolumeclaims"),
            Kind::HorizontalPodAutoscalers => ("autoscaling", "horizontalpodautoscalers"),
            Kind::Ingresses => ("networking.k8s.io", "ingresses"),
            Kind::NetworkPolicies => ("networking.k8s.io", "networkpolicies"),
            Kind::ServiceAccounts => ("", "serviceaccounts"),
        }
    }

    /// Words for the two counts of a tile.
    pub fn labels(self) -> (&'static str, &'static str) {
        match self {
            Kind::Events => ("normal", "warning"),
            _ => ("ok", "faulty"),
        }
    }

    pub fn namespaced(self) -> bool {
        !matches!(
            self,
            Kind::Nodes | Kind::Namespaces | Kind::PersistentVolumes
        )
    }

    /// Whether `object` is faulty. Follows k9s's health checks where it has one (pods, nodes,
    /// events, workloads, claims); other kinds are only counted.
    pub fn is_fault(self, object: &Value) -> bool {
        let int = |p: &str| object.pointer(p).and_then(Value::as_i64).unwrap_or(0);
        let text = |p: &str| {
            object
                .pointer(p)
                .and_then(Value::as_str)
                .unwrap_or_default()
        };
        let array = |p: &str| {
            object
                .pointer(p)
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
        };
        let condition = |kind: &str| {
            array("/status/conditions")
                .iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some(kind))
                .and_then(|c| c.get("status").and_then(Value::as_str))
        };
        match self {
            Kind::Pods => {
                // Completed pods are fine; every other pod needs all containers ready (and at
                // least one), the Ready condition and its readiness gates.
                if text("/status/phase") == "Succeeded" {
                    return false;
                }
                let statuses = array("/status/containerStatuses");
                let ready = statuses
                    .iter()
                    .filter(|c| c.get("ready").and_then(Value::as_bool) == Some(true))
                    .count();
                if statuses.is_empty() || ready != statuses.len() {
                    return true;
                }
                let gates_open = array("/spec/readinessGates").iter().all(|g| {
                    g.get("conditionType")
                        .and_then(Value::as_str)
                        .is_some_and(|t| condition(t) == Some("True"))
                });
                !gates_open || condition("Ready") != Some("True")
            }
            Kind::Deployments => {
                int("/spec/replicas") != int("/status/availableReplicas")
                    || int("/status/unavailableReplicas") != 0
            }
            Kind::StatefulSets => {
                let ready = int("/status/readyReplicas");
                int("/status/replicas") != ready || int("/spec/replicas") != ready
            }
            Kind::DaemonSets => {
                int("/status/desiredNumberScheduled") != int("/status/numberAvailable")
            }
            Kind::Jobs => array("/status/conditions").iter().any(|c| {
                c.get("type").and_then(Value::as_str) == Some("Failed")
                    && c.get("status").and_then(Value::as_str) == Some("True")
            }),
            // Not ready, or cordoned.
            Kind::Nodes => {
                condition("Ready") != Some("True")
                    || object.pointer("/spec/unschedulable") == Some(&Value::Bool(true))
            }
            Kind::Events => text("/type") != "Normal",
            Kind::PersistentVolumeClaims => {
                object.pointer("/metadata/deletionTimestamp").is_some()
                    || !matches!(text("/status/phase"), "Bound" | "Available")
            }
            _ => false,
        }
    }
}

/// How many objects of a kind there are, and how many of them are faulty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count {
    pub total: usize,
    pub fault: usize,
}

impl Count {
    pub fn ok(self) -> usize {
        self.total - self.fault
    }
}

/// Counts `objects` of `kind`.
pub fn count<'a>(kind: Kind, objects: impl IntoIterator<Item = &'a Value>) -> Count {
    let mut count = Count::default();
    for object in objects {
        count.total += 1;
        if kind.is_fault(object) {
            count.fault += 1;
        }
    }
    count
}

/// The last counts of a tile, oldest first.
#[derive(Clone, Debug, Default)]
pub struct History {
    samples: VecDeque<Count>,
}

impl History {
    /// Samples kept: 20 minutes at one per ten seconds.
    pub const CAPACITY: usize = 120;

    pub fn push(&mut self, count: Count) {
        if self.samples.len() == Self::CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(count);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn oks(&self) -> Vec<f64> {
        self.samples.iter().map(|c| c.ok() as f64).collect()
    }

    pub fn faults(&self) -> Vec<f64> {
        self.samples.iter().map(|c| c.fault as f64).collect()
    }
}

/// The last values of a gauge (cluster CPU or memory use, in percent), oldest first.
#[derive(Clone, Debug, Default)]
pub struct Samples {
    values: VecDeque<f64>,
}

impl Samples {
    pub fn push(&mut self, value: f64) {
        if self.values.len() == History::CAPACITY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub fn values(&self) -> Vec<f64> {
        self.values.iter().copied().collect()
    }
}

/// Used and allocatable amounts, as a percentage (`None` without allocatable).
pub fn percent(used: f64, allocatable: f64) -> Option<f64> {
    (allocatable > 0.0).then(|| (used / allocatable * 100.0).clamp(0.0, 999.0))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pods() {
        let ready = json!({"status": {"phase": "Running",
            "conditions": [{"type": "Ready", "status": "True"}],
            "containerStatuses": [{"ready": true}]}});
        let not_ready = json!({"status": {"phase": "Running",
            "containerStatuses": [{"ready": false}]}});
        let no_condition = json!({"status": {"phase": "Running",
            "containerStatuses": [{"ready": true}]}});
        let gate = json!({"spec": {"readinessGates": [{"conditionType": "x"}]},
            "status": {"phase": "Running", "conditions": [{"type": "Ready", "status": "True"}],
            "containerStatuses": [{"ready": true}]}});
        let pending = json!({"status": {"phase": "Pending"}});
        let completed = json!({"status": {"phase": "Succeeded"}});
        let c = count(
            Kind::Pods,
            [
                &ready,
                &not_ready,
                &no_condition,
                &gate,
                &pending,
                &completed,
            ],
        );
        assert_eq!(c, Count { total: 6, fault: 4 });
        assert_eq!(c.ok(), 2);
    }

    #[test]
    fn workloads_nodes_events() {
        let d = json!({"spec": {"replicas": 3}, "status": {"availableReplicas": 3,
            "unavailableReplicas": 1}});
        assert!(Kind::Deployments.is_fault(&d));
        assert!(!Kind::Deployments.is_fault(&json!({"spec": {"replicas": 0}})));
        let node = json!({"status": {"conditions": [{"type": "Ready", "status": "True"}]}});
        assert!(!Kind::Nodes.is_fault(&node));
        let cordoned = json!({"spec": {"unschedulable": true},
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}});
        assert!(Kind::Nodes.is_fault(&cordoned));
        assert!(Kind::Events.is_fault(&json!({"type": "Warning"})));
        assert!(!Kind::Events.is_fault(&json!({"type": "Normal"})));
        assert!(Kind::PersistentVolumeClaims.is_fault(&json!({"status": {"phase": "Pending"}})));
    }

    #[test]
    fn single_namespace_hides_cluster_wide_tiles() {
        assert_eq!(Kind::shown(false).count(), 16);
        assert_eq!(Kind::shown(true).next(), Some(Kind::Pods));
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(1.0, 4.0), Some(25.0));
        assert_eq!(percent(1.0, 0.0), None);
        let mut s = Samples::default();
        for i in 0..200 {
            s.push(i as f64);
        }
        assert_eq!(s.values().len(), History::CAPACITY);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = History::default();
        for i in 0..200 {
            h.push(Count { total: i, fault: 0 });
        }
        assert_eq!(h.len(), History::CAPACITY);
        assert_eq!(h.oks()[0], 80.0);
    }
}
