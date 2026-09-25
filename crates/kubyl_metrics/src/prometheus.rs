//! A small Prometheus HTTP API client (`/api/v1/query`, `/api/v1/query_range`).
//!
//! In-cluster Prometheus is reached through the API server's service proxy
//! (`/api/v1/namespaces/{ns}/services/{scheme}:{svc}:{port}/proxy/…`) with the cluster's own
//! kube client, so it works wherever the cluster works (VPNs, bastions, exec/OIDC auth) and
//! needs only `get services/proxy`. An external URL uses a separate client with an optional
//! Authorization header from the OS keychain.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use http::header::{AUTHORIZATION, HeaderValue};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value;

use crate::openshift::Bearer;

/// Where a Prometheus-compatible API lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// A Service, through the API server's service proxy.
    Service {
        namespace: String,
        service: String,
        /// Port number or name.
        port: String,
        /// `http` or `https`.
        scheme: String,
        /// API path prefix (`""`, or e.g. `/select/0/prometheus`).
        path: String,
    },
    /// An external URL (may include a path prefix).
    Url { url: String, insecure: bool },
    /// A Service reached through its OpenShift Route (the service proxy strips credentials).
    Route {
        namespace: String,
        service: String,
        url: String,
    },
}

impl Target {
    pub fn service(namespace: &str, service: &str, port: &str) -> Self {
        Target::Service {
            namespace: namespace.into(),
            service: service.into(),
            port: port.into(),
            scheme: "http".into(),
            path: String::new(),
        }
    }

    /// `monitoring/prometheus-k8s` or the URL's host.
    pub fn label(&self) -> String {
        match self {
            Target::Service {
                namespace, service, ..
            } => format!("{namespace}/{service}"),
            Target::Url { url, .. } => url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| url.clone()),
            Target::Route {
                namespace, service, ..
            } => format!("{namespace}/{service} (route)"),
        }
    }

    /// The path every API request is appended to.
    fn base_path(&self) -> String {
        match self {
            Target::Service {
                namespace,
                service,
                port,
                scheme,
                path,
            } => {
                let scheme = if scheme == "https" { "https:" } else { "" };
                format!(
                    "/api/v1/namespaces/{namespace}/services/{scheme}{service}:{port}/proxy{}",
                    normalize_prefix(path)
                )
            }
            // The client's base URL carries the host and any prefix.
            Target::Url { .. } | Target::Route { .. } => String::new(),
        }
    }
}

fn normalize_prefix(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// What went wrong talking to Prometheus.
#[derive(Clone, Debug, PartialEq)]
pub enum PromError {
    /// HTTP status from the proxy or Prometheus (404 no such service, 503 no endpoints, 403…).
    Http(u16, String),
    /// Prometheus answered `status: error` (bad query, timeout, too many samples).
    Query(String),
    /// Not a Prometheus API response.
    Invalid(String),
    Timeout,
    Transport(String),
}

impl PromError {
    /// The target itself is unusable (as opposed to one bad query).
    pub fn is_target_error(&self) -> bool {
        !matches!(self, PromError::Query(_))
    }
}

impl fmt::Display for PromError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PromError::Http(403, _) => write!(f, "forbidden (needs get services/proxy)"),
            PromError::Http(404, _) => write!(f, "not found"),
            PromError::Http(503, _) => write!(f, "no ready endpoints (503)"),
            PromError::Http(code, message) if message.is_empty() => write!(f, "HTTP {code}"),
            PromError::Http(code, message) => write!(f, "HTTP {code}: {message}"),
            PromError::Query(message) => write!(f, "query failed: {message}"),
            PromError::Invalid(message) => write!(f, "not a Prometheus API: {message}"),
            PromError::Timeout => write!(f, "timed out"),
            PromError::Transport(message) => write!(f, "{message}"),
        }
    }
}

/// One series of an instant query.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub labels: BTreeMap<String, String>,
    pub value: f64,
}

/// One series of a range query: `(unix seconds, value)`.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeSeries {
    pub labels: BTreeMap<String, String>,
    pub values: Vec<(f64, f64)>,
}

impl RangeSeries {
    pub fn label(&self, name: &str) -> &str {
        self.labels
            .get(name)
            .map(String::as_str)
            .unwrap_or_default()
    }
}

/// A client for one target.
#[derive(Clone)]
pub struct PromClient {
    client: kube::Client,
    base: String,
    target: Target,
    /// Sent as `Authorization: Bearer …` on every request (direct calls only).
    bearer: Option<Bearer>,
}

impl fmt::Debug for PromClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromClient")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

impl PromClient {
    /// Uses the cluster's client (service proxy) for [`Target::Service`].
    pub fn new(cluster: kube::Client, target: Target) -> Self {
        Self {
            client: cluster,
            base: target.base_path(),
            target,
            bearer: None,
        }
    }

    /// A direct HTTPS client for `url` that authenticates with `bearer` on every request.
    /// `roots` (DER) replaces the OS trust store when given. TLS is always verified: a bearer
    /// token must not go to an unverified server.
    pub fn direct(
        url: &str,
        target: Target,
        roots: Option<Vec<Vec<u8>>>,
        bearer: Bearer,
    ) -> Result<Self, PromError> {
        let uri: http::Uri = url
            .trim()
            .trim_end_matches('/')
            .parse()
            .map_err(|e| PromError::Transport(format!("invalid URL: {e}")))?;
        let mut config = kube::Config::new(uri);
        config.root_cert = roots;
        config.connect_timeout = Some(Duration::from_secs(10));
        config.read_timeout = Some(QUERY_TIMEOUT);
        let client = kube::Client::try_from(config)
            .map_err(|e| PromError::Transport(format!("client: {e}")))?;
        Ok(Self {
            client,
            base: String::new(),
            target,
            bearer: Some(bearer),
        })
    }

    /// A direct client for [`Target::Url`]. `authorization` is the full header value, e.g.
    /// `Bearer …` or `Basic …`; it's marked sensitive and never logged.
    pub fn external(
        url: &str,
        insecure: bool,
        authorization: Option<&SecretString>,
    ) -> Result<Self, PromError> {
        let uri: http::Uri = url
            .trim()
            .trim_end_matches('/')
            .parse()
            .map_err(|e| PromError::Transport(format!("invalid URL: {e}")))?;
        let mut config = kube::Config::new(uri);
        config.accept_invalid_certs = insecure;
        config.connect_timeout = Some(Duration::from_secs(10));
        config.read_timeout = Some(QUERY_TIMEOUT);
        if let Some(value) = authorization {
            let mut header = HeaderValue::from_str(value.expose_secret().trim())
                .map_err(|_| PromError::Transport("invalid Authorization header".into()))?;
            header.set_sensitive(true);
            config.headers.push((AUTHORIZATION, header));
        }
        let client = kube::Client::try_from(config)
            .map_err(|e| PromError::Transport(format!("client: {e}")))?;
        let target = Target::Url {
            url: url.to_string(),
            insecure,
        };
        Ok(Self {
            client,
            base: String::new(),
            target,
            bearer: None,
        })
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// An instant query (`/api/v1/query`).
    pub async fn query(&self, promql: &str) -> Result<Vec<Sample>, PromError> {
        let body = self
            .get("/api/v1/query", &[("query", promql.to_string())])
            .await?;
        parse_vector(&body)
    }

    /// A range query (`/api/v1/query_range`).
    pub async fn query_range(
        &self,
        promql: &str,
        start: f64,
        end: f64,
        step: f64,
    ) -> Result<Vec<RangeSeries>, PromError> {
        let body = self
            .get(
                "/api/v1/query_range",
                &[
                    ("query", promql.to_string()),
                    ("start", format!("{start:.3}")),
                    ("end", format!("{end:.3}")),
                    ("step", format!("{step}")),
                ],
            )
            .await?;
        parse_matrix(&body)
    }

    /// Every metric name the server has (`/api/v1/label/__name__/values`): an index lookup, so
    /// it's cheap even on big servers. Recording rules are metric names too.
    pub async fn metric_names(&self) -> Result<Vec<String>, PromError> {
        let body = self.get("/api/v1/label/__name__/values", &[]).await?;
        parse_names(&body)
    }

    /// A cheap query that any Prometheus-compatible API answers.
    pub async fn probe(&self, timeout: Duration) -> Result<(), PromError> {
        match tokio::time::timeout(timeout, self.query("vector(1)")).await {
            Ok(result) => result.map(|_| ()),
            Err(_) => Err(PromError::Timeout),
        }
    }

    async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, PromError> {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())))
            .finish();
        let uri = if query.is_empty() {
            format!("{}{path}", self.base)
        } else {
            format!("{}{path}?{query}", self.base)
        };
        let mut request = http::Request::get(uri)
            .header(http::header::ACCEPT, "application/json")
            .body(Vec::new())
            .map_err(|e| PromError::Transport(e.to_string()))?;
        if let Some(bearer) = &self.bearer {
            let token = bearer.get().await.map_err(PromError::Transport)?;
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|_| PromError::Transport("invalid token".into()))?;
            value.set_sensitive(true);
            request.headers_mut().insert(AUTHORIZATION, value);
        }
        let text =
            match tokio::time::timeout(QUERY_TIMEOUT, self.client.request_text(request)).await {
                Err(_) => return Err(PromError::Timeout),
                Ok(Ok(text)) => text,
                Ok(Err(kube::Error::Api(status))) => {
                    // Prometheus reports bad queries as 400/422 with its own JSON body.
                    if let Ok(body) = serde_json::from_str::<Value>(&status.message)
                        && body.get("status").and_then(Value::as_str) == Some("error")
                    {
                        return Err(query_error(&body));
                    }
                    return Err(PromError::Http(status.code, short(&status.message)));
                }
                Ok(Err(err)) => return Err(PromError::Transport(short(&err.to_string()))),
            };
        serde_json::from_str(&text).map_err(|_| PromError::Invalid(short(&text)))
    }
}

/// The first line of a message, cut to a readable length (proxy errors can be HTML pages).
fn short(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default().trim();
    if line.chars().count() > 160 {
        format!("{}…", line.chars().take(160).collect::<String>())
    } else {
        line.to_string()
    }
}

fn query_error(body: &Value) -> PromError {
    PromError::Query(
        body.get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
    )
}

fn data(body: &Value, expected: &str) -> Result<Vec<Value>, PromError> {
    match body.get("status").and_then(Value::as_str) {
        Some("success") => {}
        Some("error") => return Err(query_error(body)),
        _ => return Err(PromError::Invalid("missing status".into())),
    }
    let data = &body["data"];
    let kind = data["resultType"].as_str().unwrap_or_default();
    match (kind, &data["result"]) {
        // A scalar result (`vector(1)` in some implementations, `scalar(…)`).
        ("scalar", Value::Array(pair)) if expected == "vector" => {
            Ok(vec![serde_json::json!({"metric": {}, "value": pair})])
        }
        (kind, Value::Array(items)) if kind == expected => Ok(items.clone()),
        (kind, _) => Err(PromError::Invalid(format!(
            "expected a {expected}, got {kind:?}"
        ))),
    }
}

fn labels(item: &Value) -> BTreeMap<String, String> {
    item["metric"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// `[<unix time>, "<value>"]`. NaN and ±Inf are dropped.
fn point(pair: &Value) -> Option<(f64, f64)> {
    let time = pair.get(0)?.as_f64()?;
    let value: f64 = pair.get(1)?.as_str()?.parse().ok()?;
    value.is_finite().then_some((time, value))
}

/// Parses a label-values response (`{"status":"success","data":["a","b"]}`).
pub fn parse_names(body: &Value) -> Result<Vec<String>, PromError> {
    match body.get("status").and_then(Value::as_str) {
        Some("success") => {}
        Some("error") => return Err(query_error(body)),
        _ => return Err(PromError::Invalid("missing status".into())),
    }
    body["data"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect()
        })
        .ok_or_else(|| PromError::Invalid("expected a list of names".into()))
}

/// Parses an instant-query response.
pub fn parse_vector(body: &Value) -> Result<Vec<Sample>, PromError> {
    Ok(data(body, "vector")?
        .iter()
        .filter_map(|item| {
            let (_, value) = point(&item["value"])?;
            Some(Sample {
                labels: labels(item),
                value,
            })
        })
        .collect())
}

/// Parses a range-query response.
pub fn parse_matrix(body: &Value) -> Result<Vec<RangeSeries>, PromError> {
    Ok(data(body, "matrix")?
        .iter()
        .map(|item| RangeSeries {
            labels: labels(item),
            values: item["values"]
                .as_array()
                .map(|values| values.iter().filter_map(point).collect())
                .unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn service_targets_go_through_the_proxy() {
        let target = Target::service("monitoring", "kube-prometheus-stack-prometheus", "9090");
        assert_eq!(
            target.base_path(),
            "/api/v1/namespaces/monitoring/services/kube-prometheus-stack-prometheus:9090/proxy"
        );
        assert_eq!(
            target.label(),
            "monitoring/kube-prometheus-stack-prometheus"
        );
        let vm = Target::Service {
            namespace: "vm".into(),
            service: "vmselect-vm".into(),
            port: "8481".into(),
            scheme: "https".into(),
            path: "select/0/prometheus/".into(),
        };
        assert_eq!(
            vm.base_path(),
            "/api/v1/namespaces/vm/services/https:vmselect-vm:8481/proxy/select/0/prometheus"
        );
        let url = Target::Url {
            url: "https://prom.example.com/prefix".into(),
            insecure: false,
        };
        assert_eq!(url.label(), "prom.example.com");
        let route = Target::Route {
            namespace: "openshift-monitoring".into(),
            service: "thanos-querier".into(),
            url: "https://thanos.apps.example".into(),
        };
        assert_eq!(route.label(), "openshift-monitoring/thanos-querier (route)");
        assert_eq!(route.base_path(), "");
    }

    #[test]
    fn parses_vectors_and_matrices() {
        let vector = json!({"status": "success", "data": {"resultType": "vector", "result": [
            {"metric": {"namespace": "payments", "pod": "a"}, "value": [1790000000.5, "0.25"]},
            {"metric": {"pod": "nan"}, "value": [1790000000.5, "NaN"]}
        ]}});
        let samples = parse_vector(&vector).unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].labels["namespace"], "payments");
        assert_eq!(samples[0].value, 0.25);

        let scalar =
            json!({"status": "success", "data": {"resultType": "scalar", "result": [1.0, "1"]}});
        assert_eq!(parse_vector(&scalar).unwrap()[0].value, 1.0);

        let matrix = json!({"status": "success", "data": {"resultType": "matrix", "result": [
            {"metric": {"namespace": "a"}, "values": [[10, "1"], [20, "2"], [30, "+Inf"]]}
        ]}});
        let series = parse_matrix(&matrix).unwrap();
        assert_eq!(series[0].values, vec![(10.0, 1.0), (20.0, 2.0)]);
        assert_eq!(series[0].label("namespace"), "a");
        assert_eq!(series[0].label("missing"), "");
    }

    /// Direct calls carry the bearer token (a local server stands in for the Route).
    #[tokio::test]
    async fn direct_calls_send_the_bearer_token() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_string();
            let body = r#"{"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1,"1"]}]}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let bearer = Bearer::User(kubyl_kube::auth::BearerToken::Static(SecretString::from(
            "s3cr3t".to_string(),
        )));
        let target = Target::Route {
            namespace: "openshift-monitoring".into(),
            service: "thanos-querier".into(),
            url: format!("http://{address}"),
        };
        let prom = PromClient::direct(&format!("http://{address}"), target, None, bearer).unwrap();
        let samples = prom.query("vector(1)").await.unwrap();
        assert_eq!(samples[0].value, 1.0);
        let request = server.await.unwrap().to_lowercase();
        assert!(
            request.starts_with("get /api/v1/query?query=vector%281%29"),
            "{request}"
        );
        assert!(
            request.contains("authorization: bearer s3cr3t"),
            "{request}"
        );
        // The token never shows up in debug output.
        assert!(!format!("{prom:?}").contains("s3cr3t"));
    }

    #[test]
    fn parses_metric_names() {
        let body = json!({"status": "success", "data": ["up", "node_uname_info"]});
        assert_eq!(parse_names(&body).unwrap(), ["up", "node_uname_info"]);
        assert!(parse_names(&json!({"status": "success", "data": {}})).is_err());
    }

    #[test]
    fn reports_errors() {
        let error = json!({"status": "error", "errorType": "bad_data", "error": "parse error"});
        assert_eq!(
            parse_vector(&error),
            Err(PromError::Query("parse error".into()))
        );
        assert!(matches!(
            parse_matrix(&json!({"x": 1})),
            Err(PromError::Invalid(_))
        ));
        let wrong = json!({"status": "success", "data": {"resultType": "vector", "result": []}});
        assert!(matches!(parse_matrix(&wrong), Err(PromError::Invalid(_))));
        assert!(!PromError::Query("x".into()).is_target_error());
        assert!(PromError::Http(503, String::new()).is_target_error());
        assert_eq!(short(&"x".repeat(300)).chars().count(), 161);
    }
}
