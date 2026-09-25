//! metrics-server (`metrics.k8s.io/v1beta1`): current CPU and memory of pods and nodes, the
//! numbers `kubectl top` shows. No history.

use std::collections::HashMap;

use kubyl_resources::format::parse_quantity;
use kubyl_resources::metrics::Usage;
use kubyl_resources::{ObjectKey, object_key};
use serde_json::Value;

const BASE: &str = "/apis/metrics.k8s.io/v1beta1";

/// A failed metrics-server request: `forbidden` means RBAC, try per namespace.
#[derive(Clone, Debug, PartialEq)]
pub struct FetchError {
    pub message: String,
    pub forbidden: bool,
}

fn usage(object: &Value) -> Usage {
    let quantity = |key: &str| {
        object
            .pointer(&format!("/usage/{key}"))
            .and_then(Value::as_str)
            .and_then(parse_quantity)
            .unwrap_or_default()
    };
    Usage {
        cpu: quantity("cpu"),
        memory: quantity("memory"),
    }
}

/// `PodMetricsList` → usage per `namespace/name`, summed over containers.
pub fn parse_pods(list: &Value) -> HashMap<ObjectKey, Usage> {
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let key = object_key(
                item.pointer("/metadata/namespace").and_then(Value::as_str),
                item.pointer("/metadata/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            let total = item["containers"]
                .as_array()
                .into_iter()
                .flatten()
                .map(usage)
                .fold(Usage::default(), |a, b| Usage {
                    cpu: a.cpu + b.cpu,
                    memory: a.memory + b.memory,
                });
            (key, total)
        })
        .collect()
}

/// `NodeMetricsList` → usage per node name.
pub fn parse_nodes(list: &Value) -> HashMap<String, Usage> {
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = item.pointer("/metadata/name")?.as_str()?.to_string();
            Some((name, usage(item)))
        })
        .collect()
}

async fn get(client: &kube::Client, path: String) -> Result<Value, FetchError> {
    let request = http::Request::get(path)
        .body(Vec::new())
        .map_err(|e| FetchError {
            message: e.to_string(),
            forbidden: false,
        })?;
    client.request(request).await.map_err(|err| FetchError {
        forbidden: matches!(&err, kube::Error::Api(s) if s.code == 403),
        message: match &err {
            kube::Error::Api(status) => status.message.clone(),
            other => other.to_string(),
        },
    })
}

/// Pod usage in one namespace, or everywhere.
pub async fn pods(
    client: &kube::Client,
    namespace: Option<&str>,
) -> Result<HashMap<ObjectKey, Usage>, FetchError> {
    let path = match namespace {
        Some(ns) => format!("{BASE}/namespaces/{ns}/pods"),
        None => format!("{BASE}/pods"),
    };
    Ok(parse_pods(&get(client, path).await?))
}

/// Node usage.
pub async fn nodes(client: &kube::Client) -> Result<HashMap<String, Usage>, FetchError> {
    Ok(parse_nodes(&get(client, format!("{BASE}/nodes")).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sums_containers_like_kubectl_top() {
        let list = json!({"items": [{
            "metadata": {"name": "web-0", "namespace": "payments"},
            "containers": [
                {"name": "a", "usage": {"cpu": "120000000n", "memory": "10Mi"}},
                {"name": "b", "usage": {"cpu": "5m", "memory": "2048Ki"}}
            ]
        }]});
        let pods = parse_pods(&list);
        let web = pods["payments/web-0"];
        assert!((web.cpu - 0.125).abs() < 1e-9);
        assert_eq!(web.memory, 12.0 * 1024.0 * 1024.0);

        let nodes = parse_nodes(&json!({"items": [
            {"metadata": {"name": "n1"}, "usage": {"cpu": "1500m", "memory": "2Gi"}}
        ]}));
        assert_eq!(nodes["n1"].cpu, 1.5);
        assert!(parse_nodes(&json!({})).is_empty());
    }
}
