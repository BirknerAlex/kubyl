//! CPU and memory usage for list columns and the details dock.
//!
//! `kubyl_metrics` (Prometheus, metrics-server as fallback) implements [`MetricsProvider`] and
//! installs it with [`Metrics::set_provider`]. Without a provider usage cells stay empty.
//!
//! The provider answers from a cache and fetches in the background; asking marks the data as
//! wanted. When new data arrives it calls [`Metrics::changed`], so views that show usage observe
//! the global (`cx.observe_global::<Metrics>`) to re-render or re-sort.

use std::rc::Rc;

use gpui::{App, BorrowAppContext as _, Global, SharedString};
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
    revision: u64,
}

impl Global for Metrics {}

impl Metrics {
    pub fn set_provider(cx: &mut App, provider: impl MetricsProvider) {
        cx.default_global::<Self>().provider = Some(Rc::new(provider));
    }

    pub fn provider(cx: &App) -> Option<Rc<dyn MetricsProvider>> {
        cx.try_global::<Self>()?.provider.clone()
    }

    /// Called by the provider when new usage data arrived. Notifies observers of the global.
    pub fn changed(cx: &mut App) {
        cx.update_default_global::<Self, _>(|metrics, _| metrics.revision += 1);
    }

    /// Bumped by every [`Self::changed`]; cheap change detection.
    pub fn revision(cx: &App) -> u64 {
        cx.try_global::<Self>().map_or(0, |m| m.revision)
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::TestAppContext;
    use serde_json::json;

    use super::*;

    struct Fixed;

    impl MetricsProvider for Fixed {
        fn pod_usage(&self, _: &ClusterId, _: &str, name: &str, _: &App) -> Option<Usage> {
            (name == "web-0").then_some(Usage {
                cpu: 0.125,
                memory: 64.0 * 1024.0 * 1024.0,
            })
        }

        fn node_usage(&self, _: &ClusterId, _: &str, _: &App) -> Option<Usage> {
            Some(Usage {
                cpu: 1.0,
                memory: 0.0,
            })
        }
    }

    fn pod(name: &str) -> Value {
        json!({
            "metadata": {"name": name, "namespace": "default"},
            "spec": {"containers": [
                {"name": "a", "resources": {"limits": {"cpu": "250m", "memory": "128Mi"}}},
                {"name": "b", "resources": {"requests": {"cpu": "100m"}}}
            ]}
        })
    }

    #[gpui::test]
    fn usage_cells_come_from_the_provider(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let cluster = ClusterId::new("kind");
            assert_eq!(
                usage_cell(cx, &cluster, "Pod", &pod("web-0"), "cpu"),
                CellValue::Empty
            );
            Metrics::set_provider(cx, Fixed);
            assert_eq!(
                usage_cell(cx, &cluster, "Pod", &pod("web-0"), "cpu"),
                CellValue::Usage {
                    label: "125m".into(),
                    percent: 50.0,
                }
            );
            assert_eq!(
                usage_cell(cx, &cluster, "Pod", &pod("web-0"), "memory"),
                CellValue::Usage {
                    label: "64Mi".into(),
                    percent: 50.0,
                }
            );
            assert_eq!(
                usage_cell(cx, &cluster, "Pod", &pod("web-1"), "cpu"),
                CellValue::Empty
            );
            let node = json!({"metadata": {"name": "n1"}, "status": {"allocatable": {"cpu": "4"}}});
            assert_eq!(
                usage_cell(cx, &cluster, "Node", &node, "cpu"),
                CellValue::Usage {
                    label: "1".into(),
                    percent: 25.0,
                }
            );
        });
    }

    #[gpui::test]
    fn changed_notifies_observers(cx: &mut TestAppContext) {
        let seen = std::rc::Rc::new(Cell::new(0));
        let _subscription = cx.update(|cx| {
            let seen = seen.clone();
            cx.observe_global::<Metrics>(move |_| seen.set(seen.get() + 1))
        });
        cx.update(Metrics::changed);
        cx.update(Metrics::changed);
        assert_eq!(seen.get(), 2);
        assert_eq!(cx.update(|cx| Metrics::revision(cx)), 2);
    }
}
