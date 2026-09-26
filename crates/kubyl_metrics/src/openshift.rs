//! OpenShift's monitoring APIs (thanos-querier, prometheus-k8s) sit behind an auth proxy that
//! wants a bearer token allowed to `get namespaces` (the `cluster-monitoring-view` role). The API
//! server strips credentials from requests it proxies, so the service proxy always gets a 401.
//!
//! Kubyl then calls the Service's Route directly, like `oc` and the console do: with the user's
//! own token, or, when the user has none (client certificates) or theirs is refused, with a
//! short-lived token of a service account that may query (`openshift-monitoring/prometheus-k8s`,
//! minted through the TokenRequest API; nothing long-lived is read or stored). TLS is verified
//! against the OS trust store, then against the ingress CA the cluster publishes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use kubyl_kube::auth::BearerToken;
use secrecy::SecretString;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::prometheus::PromError;
use crate::transport::Transport;

/// OpenShift's monitoring stack. Only cluster admins can create `openshift-*` namespaces, so its
/// Routes may receive the user's token without being named in settings.
pub const MONITORING_NAMESPACE: &str = "openshift-monitoring";

/// Service account used when the user's own token can't be used, per namespace of the target.
pub fn default_service_account(namespace: &str) -> Option<(&'static str, &'static str)> {
    (namespace == MONITORING_NAMESPACE).then_some((MONITORING_NAMESPACE, "prometheus-k8s"))
}

/// Lifetime requested for service-account tokens; renewed a while before it ends.
const TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
const RENEW_AFTER: Duration = Duration::from_secs(3000);
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Where the Authorization header of a direct request comes from.
#[derive(Clone)]
pub enum Bearer {
    /// The user's own token (same identity and RBAC as the cluster connection).
    User(BearerToken),
    /// A TokenRequest token of a service account.
    ServiceAccount(Arc<ServiceAccountToken>),
}

impl Bearer {
    pub async fn get(&self) -> Result<SecretString, String> {
        match self {
            Bearer::User(token) => token.get().await.map_err(|e| e.to_string()),
            Bearer::ServiceAccount(token) => token.get().await,
        }
    }

    /// For messages: `your token`, `service account openshift-monitoring/prometheus-k8s`.
    pub fn describe(&self) -> String {
        match self {
            Bearer::User(_) => "your token".into(),
            Bearer::ServiceAccount(sa) => {
                format!("service account {}/{}", sa.namespace, sa.name)
            }
        }
    }
}

/// Short-lived tokens of one service account, requested on demand and renewed before expiry.
pub struct ServiceAccountToken {
    client: kube::Client,
    namespace: String,
    name: String,
    cached: Mutex<Option<(SecretString, Instant)>>,
}

impl ServiceAccountToken {
    pub fn new(client: kube::Client, namespace: &str, name: &str) -> Self {
        Self {
            client,
            namespace: namespace.into(),
            name: name.into(),
            cached: Mutex::new(None),
        }
    }

    pub async fn get(&self) -> Result<SecretString, String> {
        let mut cached = self.cached.lock().await;
        if let Some((token, at)) = cached.as_ref()
            && at.elapsed() < RENEW_AFTER
        {
            return Ok(token.clone());
        }
        let body = serde_json::json!({
            "apiVersion": "authentication.k8s.io/v1",
            "kind": "TokenRequest",
            "spec": {"expirationSeconds": TOKEN_LIFETIME.as_secs()}
        });
        let request = http::Request::post(format!(
            "/api/v1/namespaces/{}/serviceaccounts/{}/token",
            self.namespace, self.name
        ))
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&body).unwrap_or_default())
        .map_err(|e| e.to_string())?;
        let response: Value = self
            .client
            .request(request)
            .await
            .map_err(|err| match err {
                kube::Error::Api(status) if status.code == 403 => format!(
                    "you may not create tokens for {}/{}",
                    self.namespace, self.name
                ),
                other => other.to_string(),
            })?;
        let token = parse_token_request(&response)
            .ok_or_else(|| "the TokenRequest returned no token".to_string())?;
        *cached = Some((token.clone(), Instant::now()));
        Ok(token)
    }
}

/// The token of a TokenRequest response.
pub fn parse_token_request(response: &Value) -> Option<SecretString> {
    response
        .pointer("/status/token")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(|t| SecretString::from(t.to_string()))
}

/// `https://<host>` of the Route that exposes `service`, from a RouteList.
///
/// Only a host a router admitted counts: `spec.host` alone is whatever the Route's author wrote
/// and may point anywhere. The Route's `path` is only what it matches (thanos-querier's is
/// `/api`), not a prefix to add: the router passes request paths through unchanged, so
/// `/api/v1/query` goes to the host root.
pub fn parse_route_url(routes: &Value, service: &str) -> Option<String> {
    routes["items"].as_array()?.iter().find_map(|route| {
        let spec = &route["spec"];
        if spec["to"]["kind"].as_str().unwrap_or("Service") != "Service"
            || spec["to"]["name"].as_str()? != service
        {
            return None;
        }
        let host = route["status"]["ingress"]
            .as_array()?
            .iter()
            .find(|ingress| {
                ingress["conditions"].as_array().is_some_and(|conditions| {
                    conditions
                        .iter()
                        .any(|c| c["type"] == "Admitted" && c["status"] == "True")
                })
            })?["host"]
            .as_str()
            .filter(|h| !h.is_empty())?;
        let scheme = if spec.get("tls").is_some_and(|t| !t.is_null()) {
            "https"
        } else {
            "http"
        };
        Some(format!("{scheme}://{host}"))
    })
}

/// The URL of the Route in front of `namespace/service`, if the cluster has Routes and one.
pub async fn route_url(client: &kube::Client, namespace: &str, service: &str) -> Option<String> {
    let request = http::Request::get(format!(
        "/apis/route.openshift.io/v1/namespaces/{namespace}/routes"
    ))
    .body(Vec::new())
    .ok()?;
    let routes: Value = client.request(request).await.ok()?;
    parse_route_url(&routes, service)
}

/// The CA of the cluster's default ingress certificate (DER), which OpenShift publishes for
/// clients of its Routes.
pub async fn ingress_ca(client: &kube::Client) -> Option<Vec<Vec<u8>>> {
    let request = http::Request::get(
        "/api/v1/namespaces/openshift-config-managed/configmaps/default-ingress-cert",
    )
    .body(Vec::new())
    .ok()?;
    let map: Value = client.request(request).await.ok()?;
    let certs = pem_certificates(map.pointer("/data/ca-bundle.crt")?.as_str()?);
    (!certs.is_empty()).then_some(certs)
}

/// DER bodies of the `CERTIFICATE` blocks of a PEM bundle.
pub fn pem_certificates(pem: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut body: Option<String> = None;
    for line in pem.lines().map(str::trim) {
        match line {
            "-----BEGIN CERTIFICATE-----" => body = Some(String::new()),
            "-----END CERTIFICATE-----" => {
                if let Some(b64) = body.take()
                    && let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b64)
                {
                    out.push(der);
                }
            }
            line => {
                if let Some(body) = body.as_mut() {
                    body.push_str(line);
                }
            }
        }
    }
    out
}

/// A Service reached through its admitted Route.
#[derive(Clone, Debug)]
pub struct Routed {
    pub transport: Transport,
    /// `https://<admitted host><path prefix>`.
    pub url: String,
}

/// Reaches `namespace/service` through its Route with a bearer token, after the service proxy
/// was refused (401/403): the user's token first, then a service account's. TLS against the OS
/// trust store, then the cluster's ingress CA. `probe` is a path that must answer (e.g.
/// `/api/v1/query?query=vector%281%29`, `/api/v2/status`).
///
/// Callers decide whether the Service may receive a token at all (see the token rule in
/// phase 07 and 14: only `openshift-*` monitoring namespaces or Services named in settings).
pub async fn through_route(
    client: &kube::Client,
    namespace: &str,
    service: &str,
    path: &str,
    user: Option<BearerToken>,
    service_account: Option<(String, String)>,
    probe: &str,
) -> Result<Routed, String> {
    let Some(url) = route_url(client, namespace, service).await else {
        return Err(format!(
            "the API server strips credentials on proxied requests and {namespace}/{service} has no Route to call directly"
        ));
    };
    let url = format!("{url}{}", path.trim_end_matches('/'));
    let mut bearers = Vec::new();
    if let Some(user) = user {
        bearers.push(Bearer::User(user));
    }
    let service_account = service_account.or_else(|| {
        default_service_account(namespace).map(|(ns, name)| (ns.to_string(), name.to_string()))
    });
    if let Some((ns, name)) = service_account {
        bearers.push(Bearer::ServiceAccount(Arc::new(ServiceAccountToken::new(
            client.clone(),
            &ns,
            &name,
        ))));
    }
    if bearers.is_empty() {
        return Err("no token to call its Route with".into());
    }
    let answers = |transport: &Transport| {
        let transport = transport.clone();
        let probe = probe.to_string();
        async move {
            match tokio::time::timeout(PROBE_TIMEOUT, transport.get_text(&probe, &[])).await {
                Ok(result) => result.map(|_| ()),
                Err(_) => Err(PromError::Timeout),
            }
        }
    };
    let mut ingress: Option<Option<Vec<Vec<u8>>>> = None;
    let mut errors = Vec::new();
    for bearer in bearers {
        // OS trust first (public or locally trusted certificates), then the published ingress CA.
        let mut attempt = Transport::direct(&url, None, None, bearer.clone());
        let mut result = match &attempt {
            Ok(transport) => answers(transport).await,
            Err(err) => Err(err.clone()),
        };
        if matches!(result, Err(PromError::Transport(_))) {
            if ingress.is_none() {
                ingress = Some(ingress_ca(client).await);
            }
            if let Some(Some(ca)) = &ingress {
                attempt = Transport::direct(&url, Some(ca.clone()), None, bearer.clone());
                result = match &attempt {
                    Ok(transport) => answers(transport).await,
                    Err(err) => Err(err.clone()),
                };
            }
        }
        match (attempt, result) {
            (Ok(transport), Ok(())) => {
                tracing::info!(route = %url, with = %bearer.describe(), "{namespace}/{service} through its Route");
                return Ok(Routed { transport, url });
            }
            (_, Err(err)) => errors.push(format!("with {}: {err}", bearer.describe())),
            (Err(err), _) => errors.push(format!("with {}: {err}", bearer.describe())),
        }
    }
    Err(format!("its Route answered {}", errors.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;
    use serde_json::json;

    #[test]
    fn finds_the_route_of_a_service() {
        let admitted = |host: &str| json!({"ingress": [{"host": host, "conditions": [{"type": "Admitted", "status": "True"}]}]});
        let routes = json!({"items": [
            // Not admitted by a router: its host is only what its author wrote.
            {"spec": {"host": "evil.example.com", "to": {"kind": "Service", "name": "alertmanager-main"}, "tls": {"termination": "reencrypt"}}},
            {"spec": {"host": "rejected.example.com", "to": {"kind": "Service", "name": "grafana"}},
             "status": {"ingress": [{"host": "rejected.example.com", "conditions": [{"type": "Admitted", "status": "False"}]}]}},
            {"spec": {"host": "thanos-querier-openshift-monitoring.apps.example", "path": "/api",
                      "to": {"kind": "Service", "name": "thanos-querier"}, "tls": {"termination": "reencrypt"}},
             "status": admitted("thanos-querier-openshift-monitoring.apps.lab.example")}
        ]});
        assert_eq!(
            parse_route_url(&routes, "thanos-querier").as_deref(),
            Some("https://thanos-querier-openshift-monitoring.apps.lab.example")
        );
        assert_eq!(parse_route_url(&routes, "alertmanager-main"), None);
        assert_eq!(parse_route_url(&routes, "grafana"), None);
        assert_eq!(parse_route_url(&routes, "prometheus"), None);
        let plain = json!({"items": [{"spec": {"host": "p.example", "to": {"name": "p"}}, "status": admitted("p.example")}]});
        assert_eq!(
            parse_route_url(&plain, "p").as_deref(),
            Some("http://p.example")
        );
    }

    #[test]
    fn parses_token_requests_and_pem_bundles() {
        let response =
            json!({"status": {"token": "abc", "expirationTimestamp": "2026-09-25T12:00:00Z"}});
        assert_eq!(
            parse_token_request(&response).unwrap().expose_secret(),
            "abc"
        );
        assert!(parse_token_request(&json!({"status": {}})).is_none());

        let pem = "-----BEGIN CERTIFICATE-----\nAAEC\nAw==\n-----END CERTIFICATE-----\nnoise\n-----BEGIN CERTIFICATE-----\nBAU=\n-----END CERTIFICATE-----\n";
        assert_eq!(pem_certificates(pem), vec![vec![0, 1, 2, 3], vec![4, 5]]);
        assert!(pem_certificates("").is_empty());
        assert_eq!(
            default_service_account("openshift-monitoring"),
            Some(("openshift-monitoring", "prometheus-k8s"))
        );
        assert_eq!(default_service_account("monitoring"), None);
    }
}
