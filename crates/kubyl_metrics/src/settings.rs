//! The `"metrics"` section of settings.json.

use std::collections::BTreeMap;

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where usage numbers and charts come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourcePreference {
    /// Prometheus when one is found, metrics-server otherwise.
    #[default]
    Auto,
    /// Only Prometheus (no fallback).
    Prometheus,
    /// Only metrics-server: current values, no history.
    MetricsServer,
    /// No metrics at all.
    Off,
}

/// Where Kubyl finds Prometheus in one cluster. Either a Service (reached through the API
/// server's service proxy, so no extra network access is needed) or an external `url`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PrometheusOverride {
    /// Namespace of the Prometheus Service, e.g. `monitoring`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Service name, e.g. `prometheus-k8s` or `vmselect-vm`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Port number or name. Default: the Service's `web`/`http` port, else its first port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// `http` (default) or `https`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// Path prefix of the Prometheus API, e.g. `/select/0/prometheus` for VictoriaMetrics
    /// vmselect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// An external Prometheus-compatible URL instead of a Service, e.g.
    /// `https://prometheus.example.com`. Requests go directly from this machine. An
    /// Authorization header for it is kept in the OS keychain (command palette: "Metrics: Set
    /// Prometheus Authorization Header…"), never in this file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `namespace/name` of a service account whose short-lived token (TokenRequest) is used when
    /// the Prometheus sits behind an auth proxy and your own token can't be used (you sign in
    /// with a client certificate, or you may not query). Default on OpenShift:
    /// `openshift-monitoring/prometheus-k8s`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account: Option<String>,
    /// Accept a self-signed certificate from `url`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub insecure_skip_tls_verify: bool,
    /// Don't use Prometheus for this cluster.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

/// Metrics sources and the PromQL query library.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MetricsSettings {
    /// `auto`, `prometheus`, `metrics_server` or `off`.
    pub source: SourcePreference,
    /// Look for Prometheus Services automatically (kube-prometheus-stack, prometheus-operated,
    /// OpenShift thanos-querier, VictoriaMetrics…).
    pub discover: bool,
    /// Seconds between refreshes of current usage (list columns, node bars).
    pub refresh_interval: u64,
    /// Prometheus per cluster, keyed by cluster id (`<context>@<kubeconfig path>`) or context
    /// name.
    pub prometheus: BTreeMap<String, PrometheusOverride>,
    /// PromQL overrides by query id (`cluster_cpu`, `namespace_memory`, `pod_cpu`…; see the
    /// ids in `kubyl_metrics::queries`). `$sel` is replaced with extra label matchers
    /// (`,namespace="payments"`), so keep it inside a selector's braces after another matcher.
    pub queries: BTreeMap<String, String>,
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            source: SourcePreference::Auto,
            discover: true,
            refresh_interval: 15,
            prometheus: BTreeMap::new(),
            queries: BTreeMap::new(),
        }
    }
}

impl SettingsSection for MetricsSettings {
    const KEY: Option<&'static str> = Some("metrics");
}

impl MetricsSettings {
    /// The override for a cluster: by cluster id, else by context name.
    pub fn prometheus_for(&self, cluster_id: &str, context: &str) -> Option<&PrometheusOverride> {
        self.prometheus
            .get(cluster_id)
            .or_else(|| self.prometheus.get(context))
    }

    /// The override for a cluster entry by the first of its keys that has one
    /// (`ConnectionManager::settings_keys`: the entry's id, its members' ids, their context
    /// names), so overrides set for a context keep working once it's grouped.
    pub fn prometheus_for_keys(&self, keys: &[String]) -> Option<&PrometheusOverride> {
        keys.iter().find_map(|key| self.prometheus.get(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_match_id_then_context() {
        let mut settings = MetricsSettings::default();
        settings.prometheus.insert(
            "kind-dev".into(),
            PrometheusOverride {
                namespace: Some("monitoring".into()),
                ..Default::default()
            },
        );
        assert!(
            settings
                .prometheus_for("kind-dev@/tmp/k", "kind-dev")
                .is_some()
        );
        assert!(settings.prometheus_for("other@/tmp/k", "other").is_none());
        // A group finds what was set for one of its contexts.
        let keys = [
            "group:c,u@/tmp/k/".to_string(),
            "shop/c/u@/tmp/k".to_string(),
            "kind-dev".to_string(),
        ];
        assert!(settings.prometheus_for_keys(&keys).is_some());
        assert!(settings.prometheus_for_keys(&keys[..2]).is_none());
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["source"], "auto");
        assert_eq!(
            json["prometheus"]["kind-dev"],
            serde_json::json!({"namespace": "monitoring"})
        );
    }
}
