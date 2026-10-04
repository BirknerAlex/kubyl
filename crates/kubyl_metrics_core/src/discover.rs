//! Finding a Prometheus in a cluster.
//!
//! Services are ranked by name and labels (kube-prometheus-stack, kube-prometheus,
//! prometheus-operated, the community chart's `prometheus-server`, OpenShift's thanos-querier,
//! Thanos, VictoriaMetrics), then probed in order through the service proxy. The first one that
//! answers a query wins.

use std::time::Duration;

use serde_json::Value;

use crate::prometheus::{PromClient, PromError, Target};

/// Namespaces searched when listing Services cluster-wide is forbidden.
pub const WELL_KNOWN_NAMESPACES: &[&str] = &[
    "monitoring",
    "prometheus",
    "openshift-monitoring",
    "observability",
    "kube-prometheus-stack",
    "victoria-metrics",
    "vm",
    "metrics",
    "kube-system",
    "default",
];

/// How many candidates are probed at most.
const MAX_PROBES: usize = 5;
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);

/// A Service that looks like a Prometheus API.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub target: Target,
    pub score: i32,
}

/// Name fragments of Services that are part of a monitoring stack but aren't a query API.
const NOT_AN_API: &[&str] = &[
    "alertmanager",
    "node-exporter",
    "operator",
    "kube-state-metrics",
    "pushgateway",
    "adapter",
    "grafana",
    "blackbox",
    "exporter",
    "kubelet",
    "coredns",
    "reloader",
    "thanos-sidecar",
    "vmagent",
    "vminsert",
    "vmstorage",
    "vmalert",
];

/// Ranks Services (JSON objects of a Service list) that may serve the Prometheus API.
pub fn candidates(services: &[Value]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = services.iter().filter_map(candidate).collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.target.label().cmp(&b.target.label()))
    });
    out
}

fn candidate(service: &Value) -> Option<Candidate> {
    let meta = &service["metadata"];
    let name = meta["name"].as_str()?;
    let namespace = meta["namespace"].as_str().unwrap_or("default");
    let label = |key: &str| meta["labels"][key].as_str().unwrap_or_default();
    let lower = name.to_lowercase();

    // OpenShift's querier sits in front of the platform Prometheus (HTTPS on 9091).
    let openshift_querier = namespace == "openshift-monitoring" && lower == "thanos-querier";
    if NOT_AN_API.iter().any(|part| lower.contains(part)) && !openshift_querier {
        return None;
    }
    let kube_prometheus_stack = lower.ends_with("kube-prometheus-stack-prometheus")
        || lower.ends_with("-kube-prometheus-prometheus")
        || lower.ends_with("-kube-prom-prometheus");
    let (score, path) = if openshift_querier || kube_prometheus_stack {
        (100, "")
    } else if lower == "prometheus-k8s" {
        (95, "")
    } else if lower == "prometheus-operated" {
        (90, "")
    } else if lower == "prometheus-server"
        || lower.ends_with("-prometheus-server")
        || lower.contains("thanos-query")
    {
        (85, "")
    } else if lower.starts_with("vmselect") {
        (80, "/select/0/prometheus")
    } else if lower.starts_with("vmsingle") || lower.starts_with("victoria-metrics-single") {
        (80, "")
    } else if lower == "prometheus"
        || label("app.kubernetes.io/name") == "prometheus"
        || label("app") == "prometheus"
    {
        (70, "")
    } else if lower.contains("prometheus") {
        (40, "")
    } else {
        return None;
    };
    let (port, https) = pick_port(service, openshift_querier)?;
    let bonus = if namespace == "monitoring" { 1 } else { 0 };
    Some(Candidate {
        target: Target::Service {
            namespace: namespace.to_string(),
            service: name.to_string(),
            port,
            scheme: if https { "https" } else { "http" }.to_string(),
            path: path.to_string(),
        },
        score: score + bonus,
    })
}

/// The API port: named `web`/`http-web`/`http`, or a well-known number, else the first one.
fn pick_port(service: &Value, openshift_querier: bool) -> Option<(String, bool)> {
    let ports = service["spec"]["ports"].as_array()?;
    let number = |p: &Value| p["port"].as_i64();
    let name = |p: &Value| p["name"].as_str().unwrap_or_default().to_string();
    let chosen = if openshift_querier {
        ports
            .iter()
            .find(|p| name(p) == "web" || number(p) == Some(9091))
    } else {
        ports
            .iter()
            .find(|p| matches!(name(p).as_str(), "web" | "http-web" | "http" | "http-query"))
            .or_else(|| {
                ports.iter().find(|p| {
                    matches!(
                        number(p),
                        Some(9090 | 8481 | 8428 | 8429 | 10902 | 80 | 443)
                    )
                })
            })
            .or_else(|| ports.first())
    }?;
    let https = openshift_querier || name(chosen).contains("https") || number(chosen) == Some(443);
    Some((number(chosen)?.to_string(), https))
}

/// Lists Services cluster-wide, or in [`WELL_KNOWN_NAMESPACES`] when that's forbidden.
pub async fn list_services(client: &kube::Client) -> Vec<Value> {
    match list(client, "/api/v1/services").await {
        Ok(items) => items,
        Err(err) => {
            tracing::debug!(%err, "listing services cluster-wide failed; trying known namespaces");
            let mut items = Vec::new();
            for ns in WELL_KNOWN_NAMESPACES {
                if let Ok(found) = list(client, &format!("/api/v1/namespaces/{ns}/services")).await
                {
                    items.extend(found);
                }
            }
            items
        }
    }
}

async fn list(client: &kube::Client, path: &str) -> Result<Vec<Value>, kube::Error> {
    let request = http::Request::get(path)
        .body(Vec::new())
        .map_err(kube::Error::HttpError)?;
    let list: Value = client.request(request).await?;
    Ok(list["items"].as_array().cloned().unwrap_or_default())
}

/// The outcome of looking for Prometheus.
#[derive(Clone, Debug)]
pub enum Found {
    Prometheus(PromClient),
    /// Nothing answered. `best` is the most likely candidate and why it failed, for the UI.
    Nothing {
        best: Option<(Target, PromError)>,
    },
}

/// Probes `targets` in order and returns the first that answers.
pub async fn probe(client: &kube::Client, targets: Vec<Target>) -> Found {
    let mut best = None;
    for target in targets.into_iter().take(MAX_PROBES) {
        let prom = PromClient::new(client.clone(), target.clone());
        match prom.probe(PROBE_TIMEOUT).await {
            Ok(()) => {
                tracing::info!(target = %target.label(), "found Prometheus");
                return Found::Prometheus(prom);
            }
            Err(err) => {
                tracing::debug!(target = %target.label(), %err, "Prometheus candidate failed");
                best.get_or_insert((target, err));
            }
        }
    }
    Found::Nothing { best }
}

/// Lists Services and probes the ranked candidates.
pub async fn discover(client: &kube::Client) -> Found {
    let services = list_services(client).await;
    let targets = candidates(&services)
        .into_iter()
        .map(|c| c.target)
        .collect();
    probe(client, targets).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc(namespace: &str, name: &str, ports: Value) -> Value {
        json!({"metadata": {"name": name, "namespace": namespace}, "spec": {"ports": ports}})
    }

    #[test]
    fn ranks_known_setups() {
        let services = vec![
            svc(
                "monitoring",
                "kube-prometheus-stack-alertmanager",
                json!([{"name": "http-web", "port": 9093}]),
            ),
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus-node-exporter",
                json!([{"port": 9100}]),
            ),
            svc(
                "monitoring",
                "prometheus-operated",
                json!([{"name": "web", "port": 9090}]),
            ),
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                json!([
                    {"name": "reloader-web", "port": 8080},
                    {"name": "http-web", "port": 9090}
                ]),
            ),
            svc(
                "monitoring",
                "kube-prometheus-stack-operator",
                json!([{"name": "https", "port": 443}]),
            ),
            svc(
                "default",
                "kubernetes",
                json!([{"name": "https", "port": 443}]),
            ),
        ];
        let found = candidates(&services);
        let labels: Vec<String> = found.iter().map(|c| c.target.label()).collect();
        assert_eq!(
            labels,
            [
                "monitoring/kube-prometheus-stack-prometheus",
                "monitoring/prometheus-operated"
            ]
        );
        assert_eq!(
            found[0].target,
            Target::service("monitoring", "kube-prometheus-stack-prometheus", "9090")
        );
    }

    #[test]
    fn openshift_victoria_metrics_and_labels() {
        let services = vec![
            svc(
                "openshift-monitoring",
                "thanos-querier",
                json!([
                    {"name": "web", "port": 9091}, {"name": "tenancy", "port": 9092}
                ]),
            ),
            svc(
                "vm",
                "vmselect-cluster",
                json!([{"name": "http", "port": 8481}]),
            ),
            json!({"metadata": {"name": "metrics", "namespace": "obs", "labels": {"app.kubernetes.io/name": "prometheus"}},
                   "spec": {"ports": [{"port": 80}]}}),
        ];
        let found = candidates(&services);
        match &found[0].target {
            Target::Service { scheme, port, .. } => {
                assert_eq!(scheme, "https");
                assert_eq!(port, "9091");
            }
            other => panic!("unexpected {other:?}"),
        }
        match &found[1].target {
            Target::Service { path, port, .. } => {
                assert_eq!(path, "/select/0/prometheus");
                assert_eq!(port, "8481");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(found[2].target.label(), "obs/metrics");
        assert_eq!(found[2].score, 70);
    }

    #[test]
    fn ignores_services_without_ports() {
        assert!(candidates(&[svc("monitoring", "prometheus", json!([]))]).is_empty());
    }
}
