//! Azure Resource Manager (`https://management.azure.com`) for AKS: response shapes and
//! requests with a bearer token. Reads may be repeated by the caller; writes never are.

use http::Method;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use crate::provider::ProviderError;
use crate::providers::cloud::{self, Action, ApiError, BearerToken, Http, Request, Response};

/// The AKS API version (GA) for managed clusters, agent pools and upgrade profiles.
pub const API_VERSION: &str = "2024-09-01";
/// The Resource Manager API version for listing subscriptions.
pub const SUBSCRIPTIONS_API_VERSION: &str = "2022-12-01";
/// Pages of a list Kubyl follows at most.
const MAX_PAGES: usize = 10;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ManagedCluster {
    pub id: String,
    pub name: String,
    pub location: Option<String>,
    pub sku: Option<Sku>,
    pub properties: ClusterProperties,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sku {
    pub name: Option<String>,
    /// `Free`, `Standard`, `Premium`.
    pub tier: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClusterProperties {
    /// `Succeeded`, `Upgrading`, `Updating`, `Failed`, `Canceled`…
    pub provisioning_state: Option<String>,
    pub power_state: Option<PowerState>,
    /// The version asked for (the target while upgrading).
    pub kubernetes_version: Option<String>,
    /// The version the control plane runs.
    pub current_kubernetes_version: Option<String>,
    pub fqdn: Option<String>,
    #[serde(rename = "privateFQDN")]
    pub private_fqdn: Option<String>,
    #[serde(rename = "azurePortalFQDN")]
    pub azure_portal_fqdn: Option<String>,
    pub agent_pool_profiles: Vec<AgentPool>,
    pub auto_upgrade_profile: Option<AutoUpgradeProfile>,
    /// `KubernetesOfficial` or `AKSLongTermSupport`.
    pub support_plan: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PowerState {
    /// `Running` or `Stopped`.
    pub code: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoUpgradeProfile {
    /// `none`, `patch`, `stable`, `rapid`, `node-image`.
    pub upgrade_channel: Option<String>,
    pub node_os_upgrade_channel: Option<String>,
}

/// An agent pool, as `agentPoolProfiles[]` of the cluster (or the `properties` of an
/// `agentPools/{name}` resource).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentPool {
    pub name: String,
    pub count: Option<u64>,
    pub vm_size: Option<String>,
    pub os_type: Option<String>,
    /// `System` or `User`.
    pub mode: Option<String>,
    pub orchestrator_version: Option<String>,
    pub current_orchestrator_version: Option<String>,
    pub provisioning_state: Option<String>,
    pub power_state: Option<PowerState>,
    pub upgrade_settings: Option<AgentUpgradeSettings>,
    pub enable_auto_scaling: Option<bool>,
    pub min_count: Option<u64>,
    pub max_count: Option<u64>,
    /// `VirtualMachineScaleSets`, `AvailabilitySet`, `VirtualMachines`.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub node_image_version: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentUpgradeSettings {
    /// `33%` or `1`.
    pub max_surge: Option<String>,
    pub max_unavailable: Option<String>,
    pub drain_timeout_in_minutes: Option<u64>,
}

/// `upgradeProfiles/default`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradeProfile {
    pub properties: UpgradeProfileProperties,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradeProfileProperties {
    pub control_plane_profile: ControlPlaneProfile,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ControlPlaneProfile {
    pub kubernetes_version: Option<String>,
    pub upgrades: Vec<Upgrade>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Upgrade {
    pub kubernetes_version: String,
    pub is_preview: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Subscription {
    pub subscription_id: String,
    pub display_name: Option<String>,
    /// `Enabled`, `Disabled`, `Warned`, `PastDue`, `Deleted`.
    pub state: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
    next_link: Option<String>,
}

/// Reads a Resource Manager error (`{"error": {"code", "message"}}`): the action it names
/// (`… to perform action 'Microsoft.ContainerService/managedClusters/write' …`), and whether
/// the token was rejected.
pub fn api_error(response: &Response) -> ApiError {
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    let error = &body["error"];
    let code = error["code"].as_str().map(str::to_string);
    let message = error["message"].as_str().unwrap_or_default().to_string();
    let permission = message
        .split("perform action '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .map(str::to_string);
    let credentials = matches!(
        code.as_deref(),
        Some(
            "InvalidAuthenticationToken"
                | "ExpiredAuthenticationToken"
                | "AuthenticationFailed"
                | "InvalidAuthenticationTokenTenant"
        )
    );
    ApiError {
        code,
        message,
        permission,
        credentials,
    }
}

/// Resource Manager with a token.
pub struct Api {
    pub http: Http,
    pub base: Url,
    pub token: BearerToken,
    /// `az login` (with the tenant when there is one).
    pub login: String,
}

impl Api {
    /// A `nextLink` Kubyl follows: only to the API it talks to (the token goes along).
    fn next_url(&self, link: &str) -> Option<Url> {
        let url = Url::parse(link).ok()?;
        (url.scheme() == self.base.scheme()
            && url.host_str() == self.base.host_str()
            && url.port_or_known_default() == self.base.port_or_known_default())
        .then_some(url)
    }

    pub async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
        if_match: Option<&str>,
        action: &Action,
    ) -> Result<Response, ProviderError> {
        let mut request = Request::new(method, url).bearer(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(etag) = if_match {
            request = request.header("if-match", etag);
        }
        let response = self.http.send(request).await?;
        if response.ok() {
            return Ok(response);
        }
        Err(cloud::error_for(
            response.status,
            api_error(&response),
            action,
            "Azure",
            Some(self.login.clone()),
        ))
    }

    fn url(&self, path: &str, api_version: &str) -> Result<Url, ProviderError> {
        cloud::url(&self.base, path, &[("api-version", api_version)])
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        api_version: &str,
        action: &Action,
        what: &str,
    ) -> Result<T, ProviderError> {
        self.send(
            Method::GET,
            self.url(path, api_version)?,
            None,
            None,
            action,
        )
        .await?
        .json(what)
    }

    /// Every page of a list (`value` and `nextLink`).
    pub async fn list<T: DeserializeOwned>(
        &self,
        path: &str,
        api_version: &str,
        action: &Action,
        what: &str,
    ) -> Result<Vec<T>, ProviderError> {
        let mut items = Vec::new();
        let mut url = Some(self.url(path, api_version)?);
        for _ in 0..MAX_PAGES {
            let Some(current) = url.take() else {
                break;
            };
            let page: Page<T> = self
                .send(Method::GET, current, None, None, action)
                .await?
                .json(what)?;
            items.extend(page.value);
            url = page.next_link.as_deref().and_then(|l| self.next_url(l));
        }
        Ok(items)
    }

    pub async fn subscriptions(&self) -> Result<Vec<Subscription>, ProviderError> {
        self.list(
            "/subscriptions",
            SUBSCRIPTIONS_API_VERSION,
            &Action::new("Microsoft.Resources/subscriptions/read", "subscriptions"),
            "the Azure subscriptions",
        )
        .await
    }

    pub async fn clusters_in(
        &self,
        subscription: &str,
    ) -> Result<Vec<ManagedCluster>, ProviderError> {
        self.list(
            &format!(
                "/subscriptions/{}/providers/Microsoft.ContainerService/managedClusters",
                cloud::encode(subscription)
            ),
            API_VERSION,
            &Action::new(
                "Microsoft.ContainerService/managedClusters/read",
                format!("subscription {subscription}"),
            ),
            "the AKS clusters",
        )
        .await
    }

    /// A write: `PUT` of a whole resource, sent once (`If-Match` its eTag when there is one).
    pub async fn put(
        &self,
        path: &str,
        body: &Value,
        if_match: Option<&str>,
        action: &Action,
    ) -> Result<Value, ProviderError> {
        self.send(
            Method::PUT,
            self.url(path, API_VERSION)?,
            Some(body),
            if_match,
            action,
        )
        .await?
        .json("the AKS answer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::cloud::mock::fixture;

    #[test]
    fn parses_recorded_responses() {
        let cluster: ManagedCluster =
            serde_json::from_str(&fixture("aks", "managed-cluster.json")).unwrap();
        assert_eq!(
            cluster.properties.current_kubernetes_version.as_deref(),
            Some("1.30.3")
        );
        assert_eq!(
            cluster.properties.fqdn.as_deref(),
            Some("prod-k8s-abc123de.hcp.westeurope.azmk8s.io")
        );
        assert_eq!(cluster.properties.agent_pool_profiles.len(), 2);
        assert_eq!(
            cluster.properties.agent_pool_profiles[1]
                .upgrade_settings
                .as_ref()
                .unwrap()
                .max_surge
                .as_deref(),
            Some("1")
        );
        let profile: UpgradeProfile =
            serde_json::from_str(&fixture("aks", "upgrade-profile.json")).unwrap();
        assert_eq!(profile.properties.control_plane_profile.upgrades.len(), 5);
        let page: Page<Subscription> =
            serde_json::from_str(&fixture("aks", "subscriptions.json")).unwrap();
        assert_eq!(page.value.len(), 3);
    }

    #[test]
    fn reads_arm_errors() {
        let response = |status: u16, name: &str| Response {
            status,
            headers: http::HeaderMap::new(),
            body: fixture("aks", name).into_bytes(),
        };
        let denied = api_error(&response(403, "error-authorization-failed.json"));
        assert_eq!(denied.code.as_deref(), Some("AuthorizationFailed"));
        assert_eq!(
            denied.permission.as_deref(),
            Some("Microsoft.ContainerService/managedClusters/write")
        );
        assert!(!denied.credentials);
        let expired = api_error(&response(401, "error-expired-token.json"));
        assert!(expired.credentials);
    }

    #[test]
    fn follows_next_links_only_to_the_same_api() {
        let api = Api {
            http: Http::new().unwrap(),
            base: Url::parse("https://management.azure.com").unwrap(),
            token: BearerToken("t".to_string().into()),
            login: "az login".into(),
        };
        assert!(
            api.next_url(
                "https://management.azure.com/subscriptions?api-version=2022-12-01&%24skiptoken=abc"
            )
            .is_some()
        );
        assert!(
            api.next_url("https://evil.example.com/subscriptions")
                .is_none()
        );
        assert!(
            api.next_url("http://management.azure.com/subscriptions")
                .is_none()
        );
    }
}
