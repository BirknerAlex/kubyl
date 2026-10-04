//! CPU and memory usage of pods and nodes, as metrics providers report it.

use kubyl_base::SharedString;
use serde_json::Value;

use crate::format::{array_at, parse_quantity, str_at};

/// Current usage of a pod or node.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// CPU in cores (0.184 = 184m).
    pub cpu: f64,
    /// Memory in bytes.
    pub memory: f64,
}

/// A series of samples for sparklines (oldest first, evenly spaced).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageHistory {
    /// CPU in cores.
    pub cpu: Vec<f64>,
    /// Memory in bytes.
    pub memory: Vec<f64>,
    /// Time covered by the samples, e.g. `last 1h`.
    pub window: SharedString,
    /// e.g. `Prometheus` or `metrics-server`.
    pub source: SharedString,
}

/// What a provider knows about a cluster's metrics source, for "why is there no usage" hints.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceStatus {
    /// Still looking (or the cluster isn't connected).
    Detecting,
    /// Usage is available; the label names the source (`Prometheus`, `metrics-server`).
    Ready(SharedString),
    /// No source; the text says why and what to install.
    Unavailable(SharedString),
}

/// Sum of a resource's requests or limits over a pod's containers (`resource` = `cpu`/`memory`).
/// A pod is unlimited (`None`) when any container lacks a limit, since the partial sum would
/// understate what the pod may use.
pub fn pod_resource(pod: &Value, kind: &str, resource: &str) -> Option<f64> {
    let containers = array_at(pod, "/spec/containers");
    let values: Vec<f64> = containers
        .iter()
        .filter_map(|c| parse_quantity(str_at(c, &format!("/resources/{kind}/{resource}"))))
        .collect();
    let complete = kind != "limits" || values.len() == containers.len();
    (!values.is_empty() && complete).then(|| values.iter().sum())
}
