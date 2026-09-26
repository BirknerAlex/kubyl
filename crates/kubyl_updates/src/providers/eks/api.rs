//! The Amazon EKS REST API (restJson1, service `eks`, `https://eks.<region>.amazonaws.com`):
//! response shapes and signed requests. Reads may be repeated by the caller; writes never are.

use http::Method;
use jiff::Timestamp;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use super::credentials::AwsCredentials;
use super::sigv4;
use crate::provider::ProviderError;
use crate::providers::cloud::{self, Action, ApiError, Http, Request, Response, encode};

/// Pages of a list call Kubyl reads at most.
const MAX_PAGES: usize = 10;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Cluster {
    pub name: String,
    pub arn: Option<String>,
    /// `1.30`.
    pub version: Option<String>,
    /// `eks.12`.
    pub platform_version: Option<String>,
    /// `CREATING`, `ACTIVE`, `DELETING`, `FAILED`, `UPDATING`, `PENDING`.
    pub status: Option<String>,
    pub endpoint: Option<String>,
    pub upgrade_policy: Option<UpgradePolicy>,
    pub health: Option<Health>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpgradePolicy {
    /// `STANDARD` or `EXTENDED`.
    pub support_type: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Health {
    pub issues: Vec<Issue>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Issue {
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Nodegroup {
    pub nodegroup_name: String,
    pub version: Option<String>,
    /// The AMI release (`1.30.4-20240917`).
    pub release_version: Option<String>,
    /// `CREATING`, `ACTIVE`, `UPDATING`, `DELETING`, `CREATE_FAILED`, `DELETE_FAILED`,
    /// `DEGRADED`.
    pub status: Option<String>,
    pub capacity_type: Option<String>,
    pub instance_types: Vec<String>,
    /// `AL2023_x86_64_STANDARD`, `BOTTLEROCKET_ARM_64`, `CUSTOM` (an AMI from a launch
    /// template).
    pub ami_type: Option<String>,
    pub scaling_config: Option<ScalingConfig>,
    pub update_config: Option<UpdateConfig>,
    pub launch_template: Option<LaunchTemplate>,
    pub health: Option<Health>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScalingConfig {
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub desired_size: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateConfig {
    pub max_unavailable: Option<u64>,
    pub max_unavailable_percentage: Option<u64>,
    /// `DEFAULT` or `MINIMAL`.
    pub update_strategy: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchTemplate {
    pub name: Option<String>,
    pub version: Option<String>,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Addon {
    pub addon_name: String,
    pub addon_version: Option<String>,
    /// `CREATING`, `ACTIVE`, `CREATE_FAILED`, `UPDATING`, `DELETING`, `DELETE_FAILED`,
    /// `DEGRADED`, `UPDATE_FAILED`.
    pub status: Option<String>,
    pub health: Option<Health>,
}

/// One version of an add-on (`DescribeAddonVersions`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct AddonVersion {
    pub addon_version: String,
    pub compatibilities: Vec<Compatibility>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Compatibility {
    pub cluster_version: String,
    pub default_version: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddonInfo {
    pub addon_name: String,
    pub addon_versions: Vec<AddonVersion>,
}

/// An update (`DescribeUpdate`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Update {
    pub id: String,
    /// `InProgress`, `Failed`, `Cancelled`, `Successful`.
    pub status: String,
    /// `VersionUpdate`, `EndpointAccessUpdate`, `LoggingUpdate`, `ConfigUpdate`,
    /// `AddonUpdate`…
    #[serde(rename = "type")]
    pub kind: String,
    pub params: Vec<UpdateParam>,
    pub created_at: Value,
    pub errors: Vec<UpdateError>,
}

impl Update {
    /// A parameter (`Version`, `PlatformVersion`, `ReleaseVersion`, `AddonVersion`).
    pub fn param(&self, kind: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.kind == kind)
            .and_then(|p| p.value.as_deref())
    }

    pub fn created(&self) -> Option<Timestamp> {
        cloud::timestamp(&self.created_at)
    }

    pub fn running(&self) -> bool {
        self.status == "InProgress"
    }

    /// The first error, as `code: message`.
    pub fn error(&self) -> Option<String> {
        self.errors.first().map(
            |e| match (e.error_code.as_deref(), e.error_message.as_deref()) {
                (Some(code), Some(message)) => format!("{code}: {message}"),
                (code, message) => code.or(message).unwrap_or("failed").to_string(),
            },
        )
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateParam {
    #[serde(rename = "type")]
    pub kind: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateError {
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// A Kubernetes version EKS offers (`DescribeClusterVersions`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClusterVersion {
    /// `1.31`.
    pub cluster_version: String,
    pub default_platform_version: Option<String>,
    pub default_version: bool,
    pub release_date: Value,
    pub end_of_standard_support_date: Value,
    pub end_of_extended_support_date: Value,
    /// `UNSUPPORTED`, `STANDARD_SUPPORT`, `EXTENDED_SUPPORT` (deprecated for `versionStatus`).
    pub status: Option<String>,
    pub version_status: Option<String>,
    pub kubernetes_patch_version: Option<String>,
}

impl ClusterVersion {
    pub fn support_status(&self) -> Option<&str> {
        self.version_status.as_deref().or(self.status.as_deref())
    }
}

/// An EKS cluster insight (`ListInsights`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Insight {
    pub id: String,
    pub name: String,
    /// `UPGRADE_READINESS`, `MISCONFIGURATION`.
    pub category: Option<String>,
    pub kubernetes_version: Option<String>,
    pub description: Option<String>,
    pub insight_status: Option<InsightStatus>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InsightStatus {
    /// `PASSING`, `WARNING`, `ERROR`, `UNKNOWN`.
    pub status: Option<String>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
struct ClusterEnvelope {
    cluster: Cluster,
}

#[derive(Deserialize)]
struct NodegroupEnvelope {
    nodegroup: Nodegroup,
}

#[derive(Deserialize)]
struct AddonEnvelope {
    addon: Addon,
}

#[derive(Deserialize)]
pub(crate) struct UpdateEnvelope {
    pub update: Update,
}

/// A page of a list call: its items and the next token.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    #[serde(
        alias = "nodegroups",
        alias = "addons",
        alias = "updateIds",
        alias = "clusterVersions",
        alias = "insights"
    )]
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    next_token: Option<String>,
}

/// Reads an EKS error: the `x-amzn-ErrorType` header (or `__type`) and the body's message.
/// Expired or unrecognized credentials are credential errors, not missing permissions.
pub fn api_error(response: &Response) -> ApiError {
    let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    let code = response
        .header("x-amzn-errortype")
        .map(str::to_string)
        .or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|c| {
            // `AccessDeniedException:http://internal.amazon.com/…`, `…#ExpiredTokenException`.
            let c = c.split(':').next().unwrap_or(&c);
            c.rsplit('#').next().unwrap_or(c).to_string()
        });
    let message = body
        .get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `… is not authorized to perform: eks:DescribeCluster on resource: …`.
    let permission = message
        .split("perform: ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .map(|p| p.trim_end_matches([',', '.']).to_string());
    let credentials = matches!(
        code.as_deref(),
        Some(
            "UnrecognizedClientException"
                | "ExpiredTokenException"
                | "ExpiredToken"
                | "InvalidClientTokenId"
                | "InvalidSignatureException"
                | "IncompleteSignature"
                | "MissingAuthenticationTokenException"
        )
    );
    ApiError {
        code,
        message,
        permission,
        credentials,
    }
}

/// The EKS API of one cluster, with credentials for the requests.
pub struct Api {
    pub http: Http,
    pub base: Url,
    pub region: String,
    pub cluster: String,
    pub credentials: AwsCredentials,
    /// The command that renews the credentials (`aws sso login --profile prod`).
    pub login: Option<String>,
}

impl Api {
    fn cluster_path(&self) -> String {
        format!("/clusters/{}", encode(&self.cluster))
    }

    fn resource(&self) -> String {
        format!("cluster {}", self.cluster)
    }

    /// Signs and sends one request; non-2xx answers become provider errors.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
        action: &Action,
    ) -> Result<Response, ProviderError> {
        let url = cloud::url(&self.base, path, query)?;
        let mut request = Request::new(method.clone(), url.clone());
        let bytes = body
            .map(|b| serde_json::to_vec(b).unwrap_or_default())
            .unwrap_or_default();
        let mut signed_extra = Vec::new();
        if let Some(body) = body {
            request = request.json(body);
            signed_extra.push(("content-type".to_string(), "application/json".to_string()));
        }
        let headers = sigv4::sign(
            &sigv4::Signable {
                method: method.as_str(),
                url: &url,
                headers: &signed_extra,
                body: &bytes,
            },
            &self.credentials.keys(),
            &self.region,
            "eks",
            Timestamp::now(),
        );
        for (name, value) in headers {
            if let Ok(mut value) = http::HeaderValue::from_str(&value) {
                value.set_sensitive(name != "x-amz-date");
                request.headers.insert(name, value);
            }
        }
        // `Request::json` serialized the same value: the signed hash matches the body sent.
        let response = self.http.send(request).await?;
        if response.ok() {
            return Ok(response);
        }
        Err(cloud::error_for(
            response.status,
            api_error(&response),
            action,
            "AWS",
            self.login.clone(),
        ))
    }

    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        action: &Action,
        what: &str,
    ) -> Result<T, ProviderError> {
        self.send(Method::GET, path, query, None, action)
            .await?
            .json(what)
    }

    /// Every page of a list call.
    async fn list<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
        action: &Action,
        what: &str,
    ) -> Result<Vec<T>, ProviderError> {
        let mut items = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = query.to_vec();
            let mut body = body.cloned();
            if let Some(token) = &token {
                match &mut body {
                    Some(Value::Object(map)) => {
                        map.insert("nextToken".into(), Value::String(token.clone()));
                    }
                    _ => query.push(("nextToken", token)),
                }
            }
            let page: Page<T> = self
                .send(method.clone(), path, &query, body.as_ref(), action)
                .await?
                .json(what)?;
            items.extend(page.items);
            token = page.next_token.filter(|t| !t.is_empty());
            if token.is_none() {
                break;
            }
        }
        Ok(items)
    }

    pub async fn describe_cluster(&self) -> Result<Cluster, ProviderError> {
        let envelope: ClusterEnvelope = self
            .get(
                &self.cluster_path(),
                &[],
                &Action::new("eks:DescribeCluster", self.resource()),
                "the EKS cluster",
            )
            .await?;
        Ok(envelope.cluster)
    }

    pub async fn nodegroup_names(&self) -> Result<Vec<String>, ProviderError> {
        self.list(
            Method::GET,
            &format!("{}/node-groups", self.cluster_path()),
            &[],
            None,
            &Action::new("eks:ListNodegroups", self.resource()),
            "the EKS node groups",
        )
        .await
    }

    pub async fn describe_nodegroup(&self, name: &str) -> Result<Nodegroup, ProviderError> {
        let envelope: NodegroupEnvelope = self
            .get(
                &format!("{}/node-groups/{}", self.cluster_path(), encode(name)),
                &[],
                &Action::new(
                    "eks:DescribeNodegroup",
                    format!("node group {name} of {}", self.resource()),
                ),
                "an EKS node group",
            )
            .await?;
        Ok(envelope.nodegroup)
    }

    pub async fn addon_names(&self) -> Result<Vec<String>, ProviderError> {
        self.list(
            Method::GET,
            &format!("{}/addons", self.cluster_path()),
            &[],
            None,
            &Action::new("eks:ListAddons", self.resource()),
            "the EKS add-ons",
        )
        .await
    }

    pub async fn describe_addon(&self, name: &str) -> Result<Addon, ProviderError> {
        let envelope: AddonEnvelope = self
            .get(
                &format!("{}/addons/{}", self.cluster_path(), encode(name)),
                &[],
                &Action::new(
                    "eks:DescribeAddon",
                    format!("add-on {name} of {}", self.resource()),
                ),
                "an EKS add-on",
            )
            .await?;
        Ok(envelope.addon)
    }

    /// Every version of an add-on with the cluster versions it supports.
    pub async fn addon_versions(&self, name: &str) -> Result<Vec<AddonVersion>, ProviderError> {
        let infos: Vec<AddonInfo> = self
            .list(
                Method::GET,
                "/addons/supported-versions",
                &[("addonName", name)],
                None,
                &Action::new("eks:DescribeAddonVersions", format!("add-on {name}")),
                "the EKS add-on versions",
            )
            .await?;
        Ok(infos
            .into_iter()
            .filter(|i| i.addon_name == name)
            .flat_map(|i| i.addon_versions)
            .collect())
    }

    pub async fn cluster_versions(&self) -> Result<Vec<ClusterVersion>, ProviderError> {
        self.list(
            Method::GET,
            "/cluster-versions",
            &[],
            None,
            &Action::new("eks:DescribeClusterVersions", "EKS cluster versions"),
            "the EKS versions",
        )
        .await
    }

    /// Update ids of the cluster (or of a node group).
    pub async fn update_ids(&self, nodegroup: Option<&str>) -> Result<Vec<String>, ProviderError> {
        let query: Vec<(&str, &str)> = nodegroup
            .map(|n| ("nodegroupName", n))
            .into_iter()
            .collect();
        self.list(
            Method::GET,
            &format!("{}/updates", self.cluster_path()),
            &query,
            None,
            &Action::new("eks:ListUpdates", self.resource()),
            "the EKS updates",
        )
        .await
    }

    pub async fn describe_update(
        &self,
        id: &str,
        nodegroup: Option<&str>,
    ) -> Result<Update, ProviderError> {
        let query: Vec<(&str, &str)> = nodegroup
            .map(|n| ("nodegroupName", n))
            .into_iter()
            .collect();
        let envelope: UpdateEnvelope = self
            .get(
                &format!("{}/updates/{}", self.cluster_path(), encode(id)),
                &query,
                &Action::new("eks:DescribeUpdate", self.resource()),
                "an EKS update",
            )
            .await?;
        Ok(envelope.update)
    }

    /// Upgrade-readiness insights for a Kubernetes version.
    pub async fn insights(&self, version: &str) -> Result<Vec<Insight>, ProviderError> {
        let body = serde_json::json!({
            "filter": {
                "categories": ["UPGRADE_READINESS"],
                "kubernetesVersions": [version],
            }
        });
        self.list(
            Method::POST,
            &format!("{}/insights", self.cluster_path()),
            &[],
            Some(&body),
            &Action::new("eks:ListInsights", self.resource()),
            "the EKS insights",
        )
        .await
    }

    /// A write (`UpdateClusterVersion`, `UpdateNodegroupVersion`, `UpdateAddon`): sent once.
    pub async fn write(
        &self,
        path: &str,
        body: &Value,
        action: &Action,
    ) -> Result<Update, ProviderError> {
        let envelope: UpdateEnvelope = self
            .send(Method::POST, path, &[], Some(body), action)
            .await?
            .json("the EKS update")?;
        Ok(envelope.update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::cloud::mock::fixture;

    fn response(status: u16, error_type: Option<&str>, body: &str) -> Response {
        let mut headers = http::HeaderMap::new();
        if let Some(t) = error_type {
            headers.insert("x-amzn-errortype", t.parse().unwrap());
        }
        Response {
            status,
            headers,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_recorded_responses() {
        let cluster: ClusterEnvelope =
            serde_json::from_str(&fixture("eks", "describe-cluster.json")).unwrap();
        assert_eq!(cluster.cluster.version.as_deref(), Some("1.30"));
        assert_eq!(cluster.cluster.platform_version.as_deref(), Some("eks.12"));
        assert_eq!(
            cluster
                .cluster
                .upgrade_policy
                .unwrap()
                .support_type
                .as_deref(),
            Some("EXTENDED")
        );
        let ng: NodegroupEnvelope =
            serde_json::from_str(&fixture("eks", "describe-nodegroup-general.json")).unwrap();
        assert_eq!(ng.nodegroup.scaling_config.unwrap().desired_size, Some(3));
        assert_eq!(ng.nodegroup.update_config.unwrap().max_unavailable, Some(1));
        let page: Page<String> =
            serde_json::from_str(&fixture("eks", "list-nodegroups.json")).unwrap();
        assert_eq!(page.items, ["general", "gpu"]);
        let versions: Page<ClusterVersion> =
            serde_json::from_str(&fixture("eks", "describe-cluster-versions.json")).unwrap();
        assert!(versions.items.iter().any(|v| v.cluster_version == "1.31"));
        let addons: Page<AddonInfo> =
            serde_json::from_str(&fixture("eks", "describe-addon-versions-vpc-cni.json")).unwrap();
        assert!(!addons.items[0].addon_versions.is_empty());
        let update: UpdateEnvelope =
            serde_json::from_str(&fixture("eks", "describe-update-running.json")).unwrap();
        assert!(update.update.running());
        assert_eq!(update.update.param("Version"), Some("1.31"));
        let insights: Page<Insight> =
            serde_json::from_str(&fixture("eks", "list-insights.json")).unwrap();
        assert_eq!(insights.items.len(), 3);
    }

    #[test]
    fn reads_eks_errors() {
        let denied = api_error(&response(
            403,
            Some(
                "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/",
            ),
            r#"{"message":"User: arn:aws:sts::111122223333:assumed-role/Dev/alice is not authorized to perform: eks:UpdateClusterVersion on resource: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1"}"#,
        ));
        assert_eq!(denied.code.as_deref(), Some("AccessDeniedException"));
        assert_eq!(
            denied.permission.as_deref(),
            Some("eks:UpdateClusterVersion")
        );
        assert!(!denied.credentials);

        let expired = api_error(&response(
            403,
            None,
            r#"{"__type":"com.amazon.coral.service#ExpiredTokenException","Message":"The security token included in the request is expired"}"#,
        ));
        assert_eq!(expired.code.as_deref(), Some("ExpiredTokenException"));
        assert!(expired.credentials);
        assert!(expired.message.contains("expired"));

        let error = cloud::error_for(
            403,
            expired,
            &Action::new("eks:DescribeCluster", "cluster prod"),
            "AWS",
            Some("aws sso login --profile prod".into()),
        );
        assert!(matches!(
            error,
            ProviderError::Credentials { command: Some(c), .. } if c == "aws sso login --profile prod"
        ));

        let missing = api_error(&response(
            404,
            Some("ResourceNotFoundException"),
            r#"{"message":"No cluster found for name: prod."}"#,
        ));
        assert_eq!(missing.message, "No cluster found for name: prod.");
    }
}
