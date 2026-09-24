//! CPU and memory usage for list columns and the details dock.
//!
//! Phase 07 (metrics-server / Prometheus) implements [`MetricsProvider`] and installs it with
//! [`Metrics::set_provider`]. Until then no provider is installed and usage cells stay empty.

use std::rc::Rc;

use gpui::{App, Global, SharedString};
use kubyl_core::{CellValue, ClusterId};
use serde_json::Value;

use crate::format::{array_at, format_bytes, format_cpu, parse_quantity, str_at};

/// Current usage of a pod or node.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// CPU in cores (0.184 = 184m).
    pub cpu: f64,
    /// Memory in bytes.
    pub memory: f64,
}

/// A series of samples for sparklines (oldest first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageHistory {
    pub cpu: Vec<f64>,
    pub memory: Vec<f64>,
    /// e.g. `Prometheus` or `metrics-server`.
    pub source: SharedString,
}

/// Supplies usage data. Implemented by phase 07.
pub trait MetricsProvider: 'static {
    fn pod_usage(
        &self,
        cluster: &ClusterId,
        namespace: &str,
        name: &str,
        cx: &App,
    ) -> Option<Usage>;
    fn node_usage(&self, cluster: &ClusterId, name: &str, cx: &App) -> Option<Usage>;
    /// Recent samples for the details dock sparklines.
    fn pod_history(
        &self,
        _cluster: &ClusterId,
        _namespace: &str,
        _name: &str,
        _cx: &App,
    ) -> Option<UsageHistory> {
        None
    }
}

/// The installed provider, if any.
#[derive(Default)]
pub struct Metrics {
    provider: Option<Rc<dyn MetricsProvider>>,
}

impl Global for Metrics {}

impl Metrics {
    pub fn set_provider(cx: &mut App, provider: impl MetricsProvider) {
        cx.default_global::<Self>().provider = Some(Rc::new(provider));
    }

    pub fn provider(cx: &App) -> Option<Rc<dyn MetricsProvider>> {
        cx.try_global::<Self>()?.provider.clone()
    }
}

/// Sum of a resource's requests or limits over a pod's containers (`resource` = `cpu`/`memory`).
pub fn pod_resource(pod: &Value, kind: &str, resource: &str) -> Option<f64> {
    let values: Vec<f64> = array_at(pod, "/spec/containers")
        .iter()
        .filter_map(|c| parse_quantity(str_at(c, &format!("/resources/{kind}/{resource}"))))
        .collect();
    (!values.is_empty()).then(|| values.iter().sum())
}

/// The CPU or memory cell (`column` = `cpu` or `memory`) of a pod or node, from the provider.
/// The bar shows usage against the limit (pods; the request when there's no limit) or the
/// allocatable capacity (nodes).
pub fn usage_cell(
    cx: &App,
    cluster: &ClusterId,
    kind: &str,
    object: &Value,
    column: &str,
) -> CellValue {
    let Some(provider) = Metrics::provider(cx) else {
        return CellValue::Empty;
    };
    let name = str_at(object, "/metadata/name");
    let (usage, capacity) = match kind {
        "Pod" => {
            let namespace = str_at(object, "/metadata/namespace");
            let usage = provider.pod_usage(cluster, namespace, name, cx);
            let capacity = pod_resource(object, "limits", column)
                .or_else(|| pod_resource(object, "requests", column));
            (usage, capacity)
        }
        "Node" => (
            provider.node_usage(cluster, name, cx),
            parse_quantity(str_at(object, &format!("/status/allocatable/{column}"))),
        ),
        _ => return CellValue::Empty,
    };
    let Some(usage) = usage else {
        return CellValue::Empty;
    };
    let (value, label) = match column {
        "cpu" => (usage.cpu, format_cpu(usage.cpu)),
        "memory" => (usage.memory, format_bytes(usage.memory)),
        _ => return CellValue::Empty,
    };
    let percent = capacity
        .filter(|c| *c > 0.0)
        .map(|c| (value / c * 100.0) as f32)
        .unwrap_or(0.0);
    CellValue::Usage {
        label: label.into(),
        percent,
    }
}
