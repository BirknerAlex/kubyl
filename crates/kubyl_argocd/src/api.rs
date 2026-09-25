//! API mode: a client for `argocd-server`'s REST API (the gRPC gateway), for what Kubernetes
//! mode can't do: rendered manifests, desired-vs-live diffs, the full resource tree, and actions
//! checked against Argo CD's own RBAC.
//!
//! Two transports reach the server:
//! - [`Transport::Proxy`]: the API server's service proxy with the cluster's own client. The API
//!   server strips `Authorization` headers from proxied requests (phase 07 found this; checked
//!   again against Argo CD 3.5), but passes cookies, so the Argo CD token goes in the
//!   `argocd.token` cookie. The kube credentials authenticate to the API server only.
//! - [`Transport::Forward`]: a temporary loopback port-forward to the Service
//!   (`kubyl_portforward`'s ephemeral forwards), used when the proxy is forbidden or can't reach
//!   the pod network. Requests carry only the Argo CD token; the server's certificate is not
//!   checked (self-signed by default; the forward already runs over the cluster connection).
//!
//! Tokens are [`SecretString`]s: never logged, formatted or persisted outside the keychain.

use std::time::Duration;

use http::{Method, Request, StatusCode};
use http_body_util::BodyExt as _;
use openidconnect::reqwest;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ops::{AppTarget, Cascade, SyncRequest};

/// Time allowed for one API call.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Why an API call failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// The token is missing, expired or revoked: sign in again.
    #[error("not signed in to Argo CD (the session expired or was revoked)")]
    Unauthorized,
    /// Argo CD's RBAC denied it.
    #[error("Argo CD denied this: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The Kubernetes API server refused or couldn't reach the Service (RBAC for
    /// `services/proxy`, no endpoints, pod network unreachable).
    #[error("the Argo CD server isn't reachable: {0}")]
    Unreachable(String),
    #[error("Argo CD: {0}")]
    Server(String),
}

/// How requests reach `argocd-server`.
#[derive(Clone)]
pub enum Transport {
    Proxy {
        client: kube::Client,
        /// `/api/v1/namespaces/argocd/services/https:argocd-server:443/proxy` plus the root path.
        prefix: String,
    },
    Forward {
        http: reqwest::Client,
        /// `https://127.0.0.1:52871` plus the root path.
        base: String,
    },
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Proxy { prefix, .. } => write!(f, "Proxy({prefix})"),
            Transport::Forward { base, .. } => write!(f, "Forward({base})"),
        }
    }
}

impl Transport {
    /// The API server's service proxy path for a Service port.
    pub fn proxy(
        client: kube::Client,
        namespace: &str,
        service: &str,
        port: u16,
        https: bool,
        root_path: &str,
    ) -> Self {
        let scheme = if https { "https" } else { "http" };
        Transport::Proxy {
            client,
            prefix: format!(
                "/api/v1/namespaces/{namespace}/services/{scheme}:{service}:{port}/proxy{root_path}"
            ),
        }
    }

    /// A loopback forward on `local_port`.
    pub fn forward(local_port: u16, https: bool, root_path: &str) -> Result<Self, ApiError> {
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            // Loopback only, to the forward: the server's certificate is self-signed by default.
            .danger_accept_invalid_certs(true)
            .no_proxy()
            .build()
            .map_err(|e| ApiError::Unreachable(e.to_string()))?;
        let scheme = if https { "https" } else { "http" };
        Ok(Transport::Forward {
            http,
            base: format!("{scheme}://127.0.0.1:{local_port}{root_path}"),
        })
    }

    /// `via the API server's service proxy` / `via a temporary port-forward`.
    pub fn describe(&self) -> &'static str {
        match self {
            Transport::Proxy { .. } => "the API server's service proxy",
            Transport::Forward { .. } => "a temporary port-forward",
        }
    }

    /// Sends a request; returns the status and the body.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        token: Option<&SecretString>,
    ) -> Result<(StatusCode, Vec<u8>), ApiError> {
        let bytes = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
        match self {
            Transport::Proxy { client, prefix } => {
                let mut request = Request::builder()
                    .method(method)
                    .uri(format!("{prefix}{path}"))
                    .header(http::header::ACCEPT, "application/json");
                if bytes.is_some() {
                    request = request.header(http::header::CONTENT_TYPE, "application/json");
                }
                if let Some(token) = token {
                    // The API server drops Authorization from proxied requests; cookies pass.
                    request = request.header(
                        http::header::COOKIE,
                        format!("argocd.token={}", token.expose_secret()),
                    );
                }
                let request = request
                    .body(kube::client::Body::from(bytes.unwrap_or_default()))
                    .map_err(|e| ApiError::Server(e.to_string()))?;
                let response = tokio::time::timeout(TIMEOUT, client.send(request))
                    .await
                    .map_err(|_| ApiError::Unreachable("timed out".into()))?
                    .map_err(|e| ApiError::Unreachable(e.to_string()))?;
                let status = response.status();
                let body = response
                    .into_body()
                    .collect()
                    .await
                    .map_err(|e| ApiError::Unreachable(e.to_string()))?
                    .to_bytes()
                    .to_vec();
                Ok((status, body))
            }
            Transport::Forward { http, base } => {
                let mut request = http
                    .request(method, format!("{base}{path}"))
                    .header(http::header::ACCEPT, "application/json");
                if let Some(bytes) = bytes {
                    request = request
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(bytes);
                }
                if let Some(token) = token {
                    request = request.bearer_auth(token.expose_secret());
                }
                let response = request
                    .send()
                    .await
                    .map_err(|e| ApiError::Unreachable(e.without_url().to_string()))?;
                let status = response.status();
                let body = response
                    .bytes()
                    .await
                    .map_err(|e| ApiError::Unreachable(e.without_url().to_string()))?
                    .to_vec();
                Ok((
                    StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                    body,
                ))
            }
        }
    }
}

/// An error body from the Kubernetes API server (`kind: Status`) or Argo CD (`{error, code,
/// message}`).
fn error_for(status: StatusCode, body: &[u8], proxied: bool) -> ApiError {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let message = value
        .get("message")
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .map(String::from)
        .unwrap_or_else(|| {
            String::from_utf8_lossy(body)
                .chars()
                .take(200)
                .collect::<String>()
                .trim()
                .to_string()
        });
    // The API server itself answered (RBAC for services/proxy, no endpoints, a dial error).
    if proxied && value.get("kind").and_then(Value::as_str) == Some("Status") {
        return ApiError::Unreachable(message);
    }
    match status {
        StatusCode::UNAUTHORIZED => ApiError::Unauthorized,
        StatusCode::FORBIDDEN => ApiError::Forbidden(message),
        StatusCode::NOT_FOUND => ApiError::NotFound(message),
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            ApiError::Unreachable(message)
        }
        _ => ApiError::Server(if message.is_empty() {
            status.to_string()
        } else {
            message
        }),
    }
}

/// Who is signed in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UserInfo {
    pub logged_in: bool,
    pub username: String,
    pub iss: String,
    pub groups: Vec<String>,
}

/// One resource's desired and live state (`managed-resources`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceDiff {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    /// JSON documents as strings ("null" when absent).
    pub target_state: String,
    pub live_state: String,
    pub normalized_live_state: String,
    pub predicted_live_state: String,
    pub hook: bool,
    pub modified: bool,
}

impl ResourceDiff {
    fn parse(text: &str) -> Option<Value> {
        serde_json::from_str::<Value>(text)
            .ok()
            .filter(|v| !v.is_null())
    }

    /// The live object as Argo CD compares it (normalized).
    pub fn live(&self) -> Option<Value> {
        Self::parse(&self.normalized_live_state).or_else(|| Self::parse(&self.live_state))
    }

    /// The desired object (as it would look after a sync), else the target manifest.
    pub fn desired(&self) -> Option<Value> {
        Self::parse(&self.predicted_live_state).or_else(|| Self::parse(&self.target_state))
    }
}

/// A reference in the resource tree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NodeRef {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub uid: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct InfoItem {
    pub name: String,
    pub value: String,
}

/// A node of Argo CD's resource tree (managed resources and their children).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TreeNode {
    #[serde(flatten)]
    pub node: NodeRef,
    pub parent_refs: Vec<NodeRef>,
    pub info: Vec<InfoItem>,
    pub health: Option<crate::model::HealthStatus>,
    pub created_at: Option<String>,
    pub images: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceTree {
    pub nodes: Vec<TreeNode>,
    pub orphaned_nodes: Vec<TreeNode>,
}

/// Rendered manifests at a revision.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Manifests {
    /// JSON documents as strings.
    pub manifests: Vec<String>,
    pub revision: String,
    pub source_type: String,
}

/// A signed-in (or not yet signed-in) client.
#[derive(Clone)]
pub struct ArgoApi {
    transport: Transport,
    token: Option<SecretString>,
}

impl std::fmt::Debug for ArgoApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArgoApi")
            .field("transport", &self.transport)
            .field("token", &self.token.as_ref().map(|_| "…"))
            .finish()
    }
}

fn app_path(app: &AppTarget, rest: &str) -> String {
    format!(
        "/api/v1/applications/{}{rest}{}appNamespace={}",
        encode(&app.name),
        if rest.contains('?') { "&" } else { "?" },
        encode(&app.namespace)
    )
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

impl ArgoApi {
    pub fn new(transport: Transport, token: Option<SecretString>) -> Self {
        Self { transport, token }
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// The session token (for the web UI's cookie).
    pub(crate) fn token(&self) -> Option<&SecretString> {
        self.token.as_ref()
    }

    pub fn with_token(&self, token: SecretString) -> Self {
        Self {
            transport: self.transport.clone(),
            token: Some(token),
        }
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ApiError> {
        let (status, bytes) = self
            .transport
            .send(method, path, body.as_ref(), self.token.as_ref())
            .await?;
        if !status.is_success() {
            let proxied = matches!(self.transport, Transport::Proxy { .. });
            return Err(error_for(status, &bytes, proxied));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::Server(format!("unexpected answer: {e}")))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let value = self.call(Method::GET, path, None).await?;
        serde_json::from_value(value)
            .map_err(|e| ApiError::Server(format!("unexpected answer: {e}")))
    }

    /// The server's version, without credentials (to check the transport).
    pub async fn version(&self) -> Result<String, ApiError> {
        let value: Value = Self::new(self.transport.clone(), None)
            .get("/api/version")
            .await?;
        Ok(value
            .get("Version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// Signs in with a local account; returns the session token.
    pub async fn login(
        &self,
        username: &str,
        password: &SecretString,
    ) -> Result<SecretString, ApiError> {
        let body = json!({"username": username, "password": password.expose_secret()});
        let value = Self::new(self.transport.clone(), None)
            .call(Method::POST, "/api/v1/session", Some(body))
            .await
            .map_err(|err| match err {
                // A wrong password is a 401 too; say it plainly.
                ApiError::Unauthorized => {
                    ApiError::Forbidden("invalid username or password".into())
                }
                other => other,
            })?;
        value
            .get("token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(|t| SecretString::from(t.to_string()))
            .ok_or_else(|| ApiError::Server("no token in the answer".into()))
    }

    /// Who the token belongs to; `Unauthorized` when it isn't valid.
    pub async fn user_info(&self) -> Result<UserInfo, ApiError> {
        let info: UserInfo = self.get("/api/v1/session/userinfo").await?;
        if info.logged_in {
            Ok(info)
        } else {
            Err(ApiError::Unauthorized)
        }
    }

    /// Server settings (SSO configuration, URL); readable without signing in.
    pub async fn settings(&self) -> Result<Value, ApiError> {
        self.get("/api/v1/settings").await
    }

    /// Desired and live state of every managed resource.
    pub async fn managed_resources(&self, app: &AppTarget) -> Result<Vec<ResourceDiff>, ApiError> {
        let value: Value = self.get(&app_path(app, "/managed-resources")).await?;
        let items = value
            .get("items")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        serde_json::from_value(items).map_err(|e| ApiError::Server(e.to_string()))
    }

    /// The full tree: managed resources and their children.
    pub async fn resource_tree(&self, app: &AppTarget) -> Result<ResourceTree, ApiError> {
        self.get(&app_path(app, "/resource-tree")).await
    }

    /// Rendered manifests (at `revision`, else the target).
    pub async fn manifests(
        &self,
        app: &AppTarget,
        revision: Option<&str>,
    ) -> Result<Manifests, ApiError> {
        let rest = match revision {
            Some(revision) => format!("/manifests?revision={}", encode(revision)),
            None => "/manifests".into(),
        };
        self.get(&app_path(app, &rest)).await
    }

    /// Refreshes (the answer is the refreshed app).
    pub async fn refresh(&self, app: &AppTarget, hard: bool) -> Result<(), ApiError> {
        let rest = format!("?refresh={}", if hard { "hard" } else { "normal" });
        self.call(Method::GET, &app_path(app, &rest), None)
            .await
            .map(|_| ())
    }

    pub async fn sync(
        &self,
        app: &AppTarget,
        request: &SyncRequest,
        multi_source: bool,
    ) -> Result<(), ApiError> {
        let body = sync_body(app, request, multi_source);
        self.call(Method::POST, &app_path(app, "/sync"), Some(body))
            .await
            .map(|_| ())
    }

    pub async fn rollback(
        &self,
        app: &AppTarget,
        id: i64,
        prune: bool,
        dry_run: bool,
    ) -> Result<(), ApiError> {
        let body = json!({
            "name": app.name, "appNamespace": app.namespace, "id": id,
            "prune": prune, "dryRun": dry_run,
        });
        self.call(Method::POST, &app_path(app, "/rollback"), Some(body))
            .await
            .map(|_| ())
    }

    pub async fn terminate(&self, app: &AppTarget) -> Result<(), ApiError> {
        self.call(Method::DELETE, &app_path(app, "/operation"), None)
            .await
            .map(|_| ())
    }

    pub async fn delete(&self, app: &AppTarget, cascade: Cascade) -> Result<(), ApiError> {
        let rest = match cascade {
            Cascade::Foreground => "?cascade=true&propagationPolicy=foreground",
            Cascade::Background => "?cascade=true&propagationPolicy=background",
            Cascade::None => "?cascade=false",
        };
        self.call(Method::DELETE, &app_path(app, rest), None)
            .await
            .map(|_| ())
    }
}

/// The body of `POST …/sync` (`ApplicationSyncRequest`).
pub fn sync_body(app: &AppTarget, request: &SyncRequest, multi_source: bool) -> Value {
    let mut body = json!({
        "name": app.name,
        "appNamespace": app.namespace,
        "prune": request.prune,
        "dryRun": request.dry_run,
    });
    // Without options the server uses the app's own.
    if let Some(options) = &request.sync_options {
        body["syncOptions"] = json!({"items": options});
    }
    if multi_source {
        let (positions, revisions): (Vec<i64>, Vec<String>) = request
            .revisions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                r.as_ref()
                    .filter(|r| !r.trim().is_empty())
                    .map(|r| (i as i64 + 1, r.trim().to_string()))
            })
            .unzip();
        if !positions.is_empty() {
            body["sourcePositions"] = json!(positions);
            body["revisions"] = json!(revisions);
        }
    } else if let Some(Some(revision)) = request.revisions.first()
        && !revision.trim().is_empty()
    {
        body["revision"] = json!(revision.trim());
    }
    body["strategy"] = if request.apply_only {
        json!({"apply": {"force": request.force}})
    } else {
        json!({"hook": {"force": request.force}})
    };
    if !request.resources.is_empty() {
        body["resources"] = Value::Array(
            request
                .resources
                .iter()
                .map(|r| json!({"group": r.group, "kind": r.kind, "namespace": r.namespace, "name": r.name}))
                .collect(),
        );
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::SyncResource;

    #[test]
    fn errors_from_argo_and_from_the_api_server() {
        let argo = br#"{"error":"permission denied: applications, sync, demo/guestbook, sub: alice","code":7,"message":"permission denied: applications, sync, demo/guestbook, sub: alice"}"#;
        assert_eq!(
            error_for(StatusCode::FORBIDDEN, argo, true),
            ApiError::Forbidden(
                "permission denied: applications, sync, demo/guestbook, sub: alice".into()
            )
        );
        let kube = br#"{"kind":"Status","apiVersion":"v1","status":"Failure","message":"services \"https:argocd-server:443\" is forbidden: User \"bob\" cannot get resource \"services/proxy\"","code":403}"#;
        assert!(matches!(
            error_for(StatusCode::FORBIDDEN, kube, true),
            ApiError::Unreachable(_)
        ));
        // Through a forward nothing but Argo CD answers.
        assert!(matches!(
            error_for(StatusCode::FORBIDDEN, kube, false),
            ApiError::Forbidden(_)
        ));
        assert_eq!(
            error_for(StatusCode::UNAUTHORIZED, b"{}", true),
            ApiError::Unauthorized
        );
        assert!(matches!(
            error_for(StatusCode::SERVICE_UNAVAILABLE, b"no endpoints", false),
            ApiError::Unreachable(_)
        ));
    }

    #[test]
    fn paths_and_bodies() {
        let app = AppTarget::new("argocd-apps", "guestbook team");
        assert_eq!(
            app_path(&app, "/managed-resources"),
            "/api/v1/applications/guestbook+team/managed-resources?appNamespace=argocd-apps"
        );
        assert_eq!(
            app_path(&app, "?refresh=hard"),
            "/api/v1/applications/guestbook+team?refresh=hard&appNamespace=argocd-apps"
        );
        let request = SyncRequest {
            revisions: vec![Some("v2".into())],
            prune: true,
            apply_only: true,
            sync_options: Some(vec!["ServerSideApply=true".into()]),
            resources: vec![SyncResource {
                group: "apps".into(),
                kind: "Deployment".into(),
                namespace: "web".into(),
                name: "web".into(),
            }],
            ..Default::default()
        };
        let body = sync_body(&app, &request, false);
        assert_eq!(body["revision"], "v2");
        assert_eq!(body["strategy"], json!({"apply": {"force": false}}));
        assert_eq!(
            body["syncOptions"],
            json!({"items": ["ServerSideApply=true"]})
        );
        assert_eq!(body["resources"][0]["name"], "web");
        let multi = SyncRequest {
            revisions: vec![None, Some("abc".into())],
            ..Default::default()
        };
        let body = sync_body(&app, &multi, true);
        assert_eq!(body["sourcePositions"], json!([2]));
        assert_eq!(body["revisions"], json!(["abc"]));
        assert!(body.get("revision").is_none());
    }

    #[test]
    fn diffs_and_trees_parse() {
        let diff: ResourceDiff = serde_json::from_value(json!({
            "kind": "ConfigMap", "name": "cfg", "namespace": "web",
            "targetState": "{\"data\":{\"a\":\"1\"}}", "liveState": "null",
            "normalizedLiveState": "{\"data\":{\"a\":\"2\"}}", "predictedLiveState": ""
        }))
        .unwrap();
        assert_eq!(diff.live().unwrap()["data"]["a"], "2");
        assert_eq!(diff.desired().unwrap()["data"]["a"], "1");
        let tree: ResourceTree = serde_json::from_value(json!({"nodes": [
            {"group": "apps", "version": "v1", "kind": "ReplicaSet", "namespace": "web", "name": "web-1", "uid": "r1",
             "parentRefs": [{"group": "apps", "kind": "Deployment", "namespace": "web", "name": "web", "uid": "d1"}],
             "info": [{"name": "Revision", "value": "Rev:2"}], "health": {"status": "Healthy"}}
        ]}))
        .unwrap();
        assert_eq!(tree.nodes[0].node.kind, "ReplicaSet");
        assert_eq!(tree.nodes[0].parent_refs[0].uid, "d1");
        assert_eq!(
            tree.nodes[0].health.as_ref().unwrap().health(),
            crate::model::Health::Healthy
        );
    }

    #[test]
    fn tokens_never_print() {
        let api = ArgoApi::new(
            Transport::forward(1, true, "").unwrap(),
            Some(SecretString::from("secret-token".to_string())),
        );
        assert!(!format!("{api:?}").contains("secret-token"));
    }
}
