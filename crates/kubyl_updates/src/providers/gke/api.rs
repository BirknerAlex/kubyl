//! The GKE API (`https://container.googleapis.com/v1`): response shapes and requests with a
//! bearer token. Reads may be repeated by the caller; writes never are.

use http::Method;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use crate::provider::ProviderError;
use crate::providers::cloud::{self, Action, ApiError, BearerToken, Http, Request, Response};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cluster {
    pub name: String,
    pub location: Option<String>,
    /// `PROVISIONING`, `RUNNING`, `RECONCILING`, `STOPPING`, `ERROR`, `DEGRADED`.
    pub status: Option<String>,
    pub status_message: Option<String>,
    /// `1.30.5-gke.1014001`.
    pub current_master_version: Option<String>,
    pub release_channel: Option<ReleaseChannel>,
    pub node_pools: Vec<NodePool>,
    pub autopilot: Option<Autopilot>,
    pub locations: Vec<String>,
}

impl Cluster {
    pub fn autopilot(&self) -> bool {
        self.autopilot.as_ref().is_some_and(|a| a.enabled)
    }

    /// The release channel, `None` when not enrolled.
    pub fn channel(&self) -> Option<&str> {
        self.release_channel
            .as_ref()
            .and_then(|c| c.channel.as_deref())
            .filter(|c| !c.is_empty() && *c != "UNSPECIFIED")
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReleaseChannel {
    /// `RAPID`, `REGULAR`, `STABLE`, `EXTENDED`, `UNSPECIFIED`.
    pub channel: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Autopilot {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodePool {
    pub name: String,
    pub version: Option<String>,
    /// Per zone.
    pub initial_node_count: Option<u64>,
    pub autoscaling: Option<Autoscaling>,
    pub upgrade_settings: Option<UpgradeSettings>,
    /// `PROVISIONING`, `RUNNING`, `RUNNING_WITH_ERROR`, `RECONCILING`, `STOPPING`, `ERROR`.
    pub status: Option<String>,
    pub status_message: Option<String>,
    pub config: Option<NodeConfig>,
    pub locations: Vec<String>,
    pub management: Option<Management>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Autoscaling {
    pub enabled: bool,
    /// Per zone.
    pub min_node_count: Option<u64>,
    pub max_node_count: Option<u64>,
    /// For the whole pool.
    pub total_min_node_count: Option<u64>,
    pub total_max_node_count: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradeSettings {
    pub max_surge: Option<u64>,
    pub max_unavailable: Option<u64>,
    /// `SURGE` or `BLUE_GREEN`.
    pub strategy: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeConfig {
    pub machine_type: Option<String>,
    /// `COS_CONTAINERD`, `UBUNTU_CONTAINERD`.
    pub image_type: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Management {
    pub auto_upgrade: bool,
    pub auto_repair: bool,
}

/// `serverConfig`: the versions a location offers.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServerConfig {
    pub default_cluster_version: Option<String>,
    pub valid_master_versions: Vec<String>,
    pub valid_node_versions: Vec<String>,
    pub channels: Vec<ChannelConfig>,
}

impl ServerConfig {
    pub fn channel(&self, name: &str) -> Option<&ChannelConfig> {
        self.channels.iter().find(|c| c.channel == name)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChannelConfig {
    pub channel: String,
    pub default_version: Option<String>,
    pub valid_versions: Vec<String>,
    /// The version auto-upgrade moves clusters on this channel to.
    pub upgrade_target_version: Option<String>,
}

/// An operation (`UPGRADE_MASTER`, `UPGRADE_NODES`…).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Operation {
    pub name: String,
    pub operation_type: Option<String>,
    /// `PENDING`, `RUNNING`, `DONE`, `ABORTING`.
    pub status: Option<String>,
    pub detail: Option<String>,
    pub status_message: Option<String>,
    /// The cluster or node pool (`…/clusters/prod/nodePools/default-pool`).
    pub target_link: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub progress: Option<OperationProgress>,
    pub error: Option<OperationError>,
}

impl Operation {
    pub fn running(&self) -> bool {
        matches!(self.status.as_deref(), Some("PENDING" | "RUNNING"))
    }

    /// The node pool it works on (from `targetLink`).
    pub fn node_pool(&self) -> Option<&str> {
        let link = self.target_link.as_deref()?;
        let (_, pool) = link.split_once("/nodePools/")?;
        Some(pool.split('/').next().unwrap_or(pool))
    }

    /// Whether it works on `cluster` (or one of its node pools).
    pub fn is_for(&self, cluster: &str) -> bool {
        self.target_link.as_deref().is_some_and(|link| {
            let suffix = format!("/clusters/{cluster}");
            link.ends_with(&suffix) || link.contains(&format!("{suffix}/"))
        })
    }

    /// Done/total from the progress metrics (`NODES_DONE`/`NODES_TOTAL`, `nodes done`…), as a
    /// percentage.
    pub fn percent(&self) -> Option<f32> {
        let metrics = &self.progress.as_ref()?.metrics;
        let find = |words: &[&str]| {
            metrics.iter().find_map(|m| {
                let name = m.name.to_lowercase().replace('_', " ");
                words
                    .iter()
                    .any(|w| name.split_whitespace().any(|part| part == *w))
                    .then(|| m.value())
                    .flatten()
            })
        };
        let total = find(&["total"])?;
        let done = find(&["done", "complete", "completed"])?;
        (total > 0.0).then(|| (done / total * 100.0).clamp(0.0, 100.0) as f32)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OperationProgress {
    pub status: Option<String>,
    pub metrics: Vec<Metric>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Metric {
    pub name: String,
    /// int64: a JSON string.
    pub int_value: Option<Value>,
    pub double_value: Option<f64>,
}

impl Metric {
    fn value(&self) -> Option<f64> {
        match &self.int_value {
            Some(Value::String(s)) => s.parse().ok(),
            Some(Value::Number(n)) => n.as_f64(),
            _ => self.double_value,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OperationError {
    pub message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Operations {
    #[serde(default)]
    operations: Vec<Operation>,
}

/// Reads a Google API error (`{"error": {"code", "message", "status"}}`): the permission it
/// names (`Required "container.clusters.update" permission(s)`), and whether the token was
/// rejected.
pub fn api_error(response: &Response) -> ApiError {
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    let error = &body["error"];
    let code = error["status"].as_str().map(str::to_string);
    let message = error["message"].as_str().unwrap_or_default().to_string();
    let permission = message
        .split("Required \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string);
    ApiError {
        credentials: code.as_deref() == Some("UNAUTHENTICATED"),
        code,
        message,
        permission,
    }
}

/// The GKE API with a token.
pub struct Api {
    pub http: Http,
    pub base: Url,
    pub token: BearerToken,
    /// `gcloud auth login` (or the ADC variant).
    pub login: String,
}

impl Api {
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        action: &Action,
    ) -> Result<Response, ProviderError> {
        let url = cloud::url(&self.base, path, &[])?;
        let mut request = Request::new(method, url).bearer(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = self.http.send(request).await?;
        if response.ok() {
            return Ok(response);
        }
        let error = api_error(&response);
        // A disabled API is a 403 too, but no permission fixes it.
        if error.message.contains("has not been used") || error.message.contains("is disabled") {
            return Err(ProviderError::Other(format!(
                "Google Cloud API: {}",
                error.message
            )));
        }
        Err(cloud::error_for(
            response.status,
            error,
            action,
            "Google Cloud",
            Some(self.login.clone()),
        ))
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        action: &Action,
        what: &str,
    ) -> Result<T, ProviderError> {
        self.send(Method::GET, path, None, action).await?.json(what)
    }

    pub async fn cluster(&self, path: &str, action: &Action) -> Result<Cluster, ProviderError> {
        self.get(path, action, "the GKE cluster").await
    }

    pub async fn server_config(
        &self,
        location_path: &str,
        action: &Action,
    ) -> Result<ServerConfig, ProviderError> {
        self.get(
            &format!("{location_path}/serverConfig"),
            action,
            "the GKE server config",
        )
        .await
    }

    pub async fn operations(
        &self,
        location_path: &str,
        action: &Action,
    ) -> Result<Vec<Operation>, ProviderError> {
        let operations: Operations = self
            .get(
                &format!("{location_path}/operations"),
                action,
                "the GKE operations",
            )
            .await?;
        Ok(operations.operations)
    }

    pub async fn node_pool(&self, path: &str, action: &Action) -> Result<NodePool, ProviderError> {
        self.get(path, action, "the GKE node pool").await
    }

    /// A write: sent once; GKE answers with the operation it started.
    pub async fn write(
        &self,
        method: Method,
        path: &str,
        body: &Value,
        action: &Action,
    ) -> Result<Operation, ProviderError> {
        self.send(method, path, Some(body), action)
            .await?
            .json("the GKE operation")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::cloud::mock::fixture;

    #[test]
    fn parses_recorded_responses() {
        let cluster: Cluster = serde_json::from_str(&fixture("gke", "cluster.json")).unwrap();
        assert_eq!(
            cluster.current_master_version.as_deref(),
            Some("1.30.5-gke.1014001")
        );
        assert_eq!(cluster.channel(), Some("REGULAR"));
        assert_eq!(cluster.node_pools.len(), 2);
        let config: ServerConfig =
            serde_json::from_str(&fixture("gke", "server-config.json")).unwrap();
        assert_eq!(
            config
                .channel("REGULAR")
                .unwrap()
                .upgrade_target_version
                .as_deref(),
            Some("1.31.1-gke.1678000")
        );
        let operations: Operations =
            serde_json::from_str(&fixture("gke", "operations.json")).unwrap();
        let running: Vec<_> = operations
            .operations
            .iter()
            .filter(|o| o.running() && o.is_for("prod"))
            .collect();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].node_pool(), Some("batch"));
        assert_eq!(running[0].percent(), Some(50.0));
        // Another cluster's operation isn't ours.
        assert!(!operations.operations[2].is_for("prod"));
    }

    #[test]
    fn reads_google_errors() {
        let response = |status: u16, name: &str| Response {
            status,
            headers: http::HeaderMap::new(),
            body: fixture("gke", name).into_bytes(),
        };
        let denied = api_error(&response(403, "error-permission-denied.json"));
        assert_eq!(denied.code.as_deref(), Some("PERMISSION_DENIED"));
        assert_eq!(
            denied.permission.as_deref(),
            Some("container.clusters.update")
        );
        assert!(!denied.credentials);
        let expired = api_error(&response(401, "error-unauthenticated.json"));
        assert!(expired.credentials);
    }
}
