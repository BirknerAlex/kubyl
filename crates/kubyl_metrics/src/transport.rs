//! HTTP to in-cluster services and external URLs, shared by the Prometheus client and the
//! alerts crate (phase 14).
//!
//! - [`Transport::service_proxy`]: the API server's service proxy with the cluster's own kube
//!   client (`/api/v1/namespaces/{ns}/services/{scheme}:{svc}:{port}/proxy/…`). Works wherever
//!   the cluster works and needs only `services/proxy`. Nothing but the API server sees the
//!   user's credentials.
//! - [`Transport::direct`]: HTTPS with a bearer token (an OpenShift Route, a loopback forward),
//!   TLS always verified: against given roots (and a TLS server name) or the OS trust store.
//! - [`Transport::external`]: a URL with an optional Authorization header (from the keychain),
//!   optional CA and client certificate (mTLS).
//!
//! Credentials only go over HTTPS or to loopback ([`credentials_allowed`]). Headers holding
//! credentials are marked sensitive; `Debug` never shows them.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value;

use crate::openshift::Bearer;

/// Requests give up after this long.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What went wrong talking to a service.
#[derive(Clone, Debug, PartialEq)]
pub enum PromError {
    /// HTTP status from the proxy or the service (404 no such service, 503 no endpoints, 403…).
    Http(u16, String),
    /// Prometheus answered `status: error` (bad query, timeout, too many samples).
    Query(String),
    /// Not the expected API.
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

/// Transport errors are the same for every service.
pub type Error = PromError;

/// Credentials only go over HTTPS, or plain HTTP to this machine (`kubectl port-forward`).
pub fn credentials_allowed(uri: &http::Uri) -> Result<(), PromError> {
    let loopback = match uri
        .host()
        .map(|h| h.trim_start_matches('[').trim_end_matches(']'))
    {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    };
    if uri.scheme_str() == Some("https") || loopback {
        Ok(())
    } else {
        Err(PromError::Transport(format!(
            "not sending credentials to {uri} over plain HTTP; use an https:// URL"
        )))
    }
}

/// `/api/v1/namespaces/{ns}/services/{https:}{svc}:{port}/proxy{prefix}`.
pub fn proxy_base(namespace: &str, service: &str, port: &str, scheme: &str, path: &str) -> String {
    let scheme = if scheme == "https" { "https:" } else { "" };
    format!(
        "/api/v1/namespaces/{namespace}/services/{scheme}{service}:{port}/proxy{}",
        normalize_prefix(path)
    )
}

/// `""`, or a path prefix with a leading and without a trailing slash.
pub fn normalize_prefix(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// The first line of a message, cut to a readable length (proxy errors can be HTML pages).
pub fn short(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default().trim();
    if line.chars().count() > 160 {
        format!("{}…", line.chars().take(160).collect::<String>())
    } else {
        line.to_string()
    }
}

/// TLS options of an external URL.
#[derive(Clone, Debug, Default)]
pub struct ExternalTls {
    /// Accept any certificate.
    pub insecure: bool,
    /// Trust these CA certificates (DER) instead of the OS trust store.
    pub roots: Option<Vec<Vec<u8>>>,
    /// Client certificate and key files (PEM) for mutual TLS. The key stays in its file.
    pub client_certificate: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
}

/// One way to reach a service. Cheap to clone.
#[derive(Clone)]
pub struct Transport {
    client: kube::Client,
    base: String,
    /// Sent as `Authorization: Bearer …` on every request (direct calls only).
    bearer: Option<Bearer>,
    /// Extra headers, e.g. `X-Scope-OrgID` (never credentials: those are sensitive and set by
    /// the constructors).
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl fmt::Debug for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transport")
            .field("base", &self.base)
            .field("bearer", &self.bearer.as_ref().map(Bearer::describe))
            .finish_non_exhaustive()
    }
}

fn parse_url(url: &str) -> Result<http::Uri, PromError> {
    url.trim()
        .trim_end_matches('/')
        .parse()
        .map_err(|e| PromError::Transport(format!("invalid URL: {e}")))
}

fn build(config: kube::Config) -> Result<kube::Client, PromError> {
    kube::Client::try_from(config).map_err(|e| PromError::Transport(format!("client: {e}")))
}

impl Transport {
    /// Through the API server's service proxy, with the cluster's own client.
    pub fn service_proxy(
        cluster: kube::Client,
        namespace: &str,
        service: &str,
        port: &str,
        scheme: &str,
        path: &str,
    ) -> Self {
        Self {
            client: cluster,
            base: proxy_base(namespace, service, port, scheme, path),
            bearer: None,
            headers: Vec::new(),
        }
    }

    /// Paths relative to the client's own base (a kube client for another API server).
    pub fn with_base(client: kube::Client, base: impl Into<String>) -> Self {
        Self {
            client,
            base: base.into(),
            bearer: None,
            headers: Vec::new(),
        }
    }

    /// A direct HTTPS client for `url` that authenticates with `bearer` on every request.
    /// `roots` (DER) replace the OS trust store when given; `tls_server_name` is the name the
    /// certificate must have (a loopback forward to a Service). TLS is always verified: a bearer
    /// token must not go to an unverified server.
    pub fn direct(
        url: &str,
        roots: Option<Vec<Vec<u8>>>,
        tls_server_name: Option<String>,
        bearer: Bearer,
    ) -> Result<Self, PromError> {
        let uri = parse_url(url)?;
        credentials_allowed(&uri)?;
        let mut config = kube::Config::new(uri);
        config.root_cert = roots;
        config.tls_server_name = tls_server_name;
        config.connect_timeout = Some(Duration::from_secs(10));
        config.read_timeout = Some(REQUEST_TIMEOUT);
        Ok(Self {
            client: build(config)?,
            base: String::new(),
            bearer: Some(bearer),
            headers: Vec::new(),
        })
    }

    /// A client for an external URL. `authorization` is the full header value (`Bearer …`,
    /// `Basic …`), marked sensitive and never logged; it's only sent over HTTPS (or to
    /// loopback). Callers that must not send it without TLS verification check `tls.insecure`
    /// first (the alerts crate does).
    pub fn external(
        url: &str,
        authorization: Option<&SecretString>,
        tls: &ExternalTls,
    ) -> Result<Self, PromError> {
        let uri = parse_url(url)?;
        if authorization.is_some() {
            credentials_allowed(&uri)?;
        }
        let mut config = kube::Config::new(uri);
        config.accept_invalid_certs = tls.insecure;
        config.root_cert = tls.roots.clone();
        config.auth_info.client_certificate = tls
            .client_certificate
            .as_ref()
            .map(|p| p.display().to_string());
        config.auth_info.client_key = tls.client_key.as_ref().map(|p| p.display().to_string());
        config.connect_timeout = Some(Duration::from_secs(10));
        config.read_timeout = Some(REQUEST_TIMEOUT);
        if let Some(value) = authorization {
            let mut header = HeaderValue::from_str(value.expose_secret().trim())
                .map_err(|_| PromError::Transport("invalid Authorization header".into()))?;
            header.set_sensitive(true);
            config.headers.push((AUTHORIZATION, header));
        }
        Ok(Self {
            client: build(config)?,
            base: String::new(),
            bearer: None,
            headers: Vec::new(),
        })
    }

    /// Adds a header to every request (e.g. `X-Scope-OrgID` for Mimir/Cortex).
    pub fn with_header(mut self, name: &'static str, value: &str) -> Result<Self, PromError> {
        let value = HeaderValue::from_str(value)
            .map_err(|_| PromError::Transport(format!("invalid {name} header")))?;
        self.headers.push((HeaderName::from_static(name), value));
        Ok(self)
    }

    /// Who the requests authenticate as, for messages (`your token`, `service account …`).
    pub fn bearer(&self) -> Option<&Bearer> {
        self.bearer.as_ref()
    }

    /// The path every API request is appended to.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn uri(&self, path: &str, params: &[(&str, String)]) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())))
            .finish();
        match (query.is_empty(), path.contains('?')) {
            (true, _) => format!("{}{path}", self.base),
            (false, true) => format!("{}{path}&{query}", self.base),
            (false, false) => format!("{}{path}?{query}", self.base),
        }
    }

    async fn send(&self, mut request: http::Request<Vec<u8>>) -> Result<String, PromError> {
        if let Some(bearer) = &self.bearer {
            let token = bearer.get().await.map_err(PromError::Transport)?;
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|_| PromError::Transport("invalid token".into()))?;
            value.set_sensitive(true);
            request.headers_mut().insert(AUTHORIZATION, value);
        }
        for (name, value) in &self.headers {
            request.headers_mut().insert(name.clone(), value.clone());
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, self.client.request_text(request)).await {
            Err(_) => Err(PromError::Timeout),
            Ok(Ok(text)) => Ok(text),
            Ok(Err(kube::Error::Api(status))) => {
                // Prometheus reports bad queries as 400/422 with its own JSON body.
                if let Ok(body) = serde_json::from_str::<Value>(&status.message)
                    && body.get("status").and_then(Value::as_str) == Some("error")
                {
                    return Err(query_error(&body));
                }
                Err(PromError::Http(status.code, short(&status.message)))
            }
            Ok(Err(err)) => Err(PromError::Transport(short(&err.to_string()))),
        }
    }

    /// `GET` returning the body as text.
    pub async fn get_text(
        &self,
        path: &str,
        params: &[(&str, String)],
    ) -> Result<String, PromError> {
        let request = http::Request::get(self.uri(path, params))
            .header(http::header::ACCEPT, "application/json")
            .body(Vec::new())
            .map_err(|e| PromError::Transport(e.to_string()))?;
        self.send(request).await
    }

    /// `GET` returning JSON.
    pub async fn get(&self, path: &str, params: &[(&str, String)]) -> Result<Value, PromError> {
        let text = self.get_text(path, params).await?;
        serde_json::from_str(&text).map_err(|_| PromError::Invalid(short(&text)))
    }

    /// `POST` a JSON body, returning the JSON answer (`Null` for an empty one).
    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value, PromError> {
        let request = http::Request::post(self.uri(path, &[]))
            .header(http::header::ACCEPT, "application/json")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(body).unwrap_or_default())
            .map_err(|e| PromError::Transport(e.to_string()))?;
        let text = self.send(request).await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|_| PromError::Invalid(short(&text)))
    }

    /// `DELETE`.
    pub async fn delete(&self, path: &str) -> Result<(), PromError> {
        let request = http::Request::delete(self.uri(path, &[]))
            .body(Vec::new())
            .map_err(|e| PromError::Transport(e.to_string()))?;
        self.send(request).await.map(|_| ())
    }
}

/// The error of a `status: error` answer.
pub fn query_error(body: &Value) -> PromError {
    PromError::Query(
        body.get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_paths() {
        assert_eq!(
            proxy_base(
                "monitoring",
                "alertmanager-operated",
                "9093",
                "http",
                "/am/"
            ),
            "/api/v1/namespaces/monitoring/services/alertmanager-operated:9093/proxy/am"
        );
        assert_eq!(
            proxy_base("m", "s", "web", "https", ""),
            "/api/v1/namespaces/m/services/https:s:web/proxy"
        );
    }

    #[tokio::test]
    async fn external_headers_need_https() {
        let header = SecretString::from("Bearer s3cr3t".to_string());
        assert!(
            Transport::external("http://am.example.com", Some(&header), &Default::default())
                .is_err()
        );
        let ok = Transport::external("https://am.example.com", Some(&header), &Default::default())
            .unwrap()
            .with_header("x-scope-orgid", "tenant-a")
            .unwrap();
        assert!(!format!("{ok:?}").contains("s3cr3t"));
    }

    /// Direct calls carry the bearer token, posts send JSON, deletes work with empty answers.
    #[tokio::test]
    async fn direct_posts_and_deletes() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in [r#"{"silenceID":"abc"}"#, ""] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 8192];
                let n = socket.read(&mut request).await.unwrap();
                requests.push(String::from_utf8_lossy(&request[..n]).to_string());
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let bearer = Bearer::User(kubyl_kube::auth::BearerToken::Static(SecretString::from(
            "s3cr3t".to_string(),
        )));
        let transport = Transport::direct(&format!("http://{address}"), None, None, bearer)
            .unwrap()
            .with_header("x-scope-orgid", "tenant-a")
            .unwrap();
        let answer = transport
            .post_json("/api/v2/silences", &serde_json::json!({"comment": "x"}))
            .await
            .unwrap();
        assert_eq!(answer["silenceID"], "abc");
        transport.delete("/api/v2/silence/abc").await.unwrap();
        let requests = server.await.unwrap();
        let post = requests[0].to_lowercase();
        assert!(post.starts_with("post /api/v2/silences"), "{post}");
        assert!(post.contains("authorization: bearer s3cr3t"));
        assert!(post.contains("x-scope-orgid: tenant-a"));
        assert!(post.contains("content-type: application/json"));
        assert!(post.ends_with(r#"{"comment":"x"}"#), "{post}");
        assert!(requests[1].starts_with("DELETE /api/v2/silence/abc"));
        assert!(!format!("{transport:?}").contains("s3cr3t"));
    }
}
