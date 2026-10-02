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

use futures::io::AsyncBufReadExt as _;
use futures::stream::{BoxStream, StreamExt as _};
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

    /// Plain HTTP to a loopback port (a temporary port-forward), under `path`. Nothing is
    /// authenticated: the forward is the access, so no credentials ever go this way. There's no
    /// read timeout, so a quiet event stream stays open (requests still time out). Call it on
    /// the Tokio runtime (`spawn_kube`): making a kube client spawns a task.
    pub fn loopback(port: u16, path: &str) -> Result<Self, PromError> {
        let uri = parse_url(&format!("http://127.0.0.1:{port}"))?;
        let mut config = kube::Config::new(uri);
        config.connect_timeout = Some(Duration::from_secs(10));
        config.read_timeout = None;
        Ok(Self {
            client: build(config)?,
            base: normalize_prefix(path),
            bearer: None,
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

    /// Adds the bearer token and the extra headers.
    async fn authorize(&self, request: &mut http::Request<Vec<u8>>) -> Result<(), PromError> {
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
        Ok(())
    }

    async fn send(&self, mut request: http::Request<Vec<u8>>) -> Result<String, PromError> {
        self.authorize(&mut request).await?;
        match tokio::time::timeout(REQUEST_TIMEOUT, self.client.request_text(request)).await {
            Err(_) => Err(PromError::Timeout),
            Ok(Ok(text)) => Ok(text),
            Ok(Err(err)) => Err(map_error(err)),
        }
    }

    /// `GET` whose body arrives line by line for as long as the server keeps the response open
    /// (server-sent events, JSON lines). Only the wait for the response has a timeout; the
    /// stream ends when the server closes it or the stream is dropped.
    pub async fn get_lines(
        &self,
        path: &str,
        params: &[(&str, String)],
        accept: &'static str,
    ) -> Result<BoxStream<'static, Result<String, PromError>>, PromError> {
        let mut request = http::Request::get(self.uri(path, params))
            .header(http::header::ACCEPT, accept)
            .body(Vec::new())
            .map_err(|e| PromError::Transport(e.to_string()))?;
        self.authorize(&mut request).await?;
        match tokio::time::timeout(REQUEST_TIMEOUT, self.client.request_stream(request)).await {
            Err(_) => Err(PromError::Timeout),
            Ok(Ok(body)) => Ok(Box::pin(body)
                .lines()
                .map(|line| line.map_err(|e| PromError::Transport(short(&e.to_string()))))
                .boxed()),
            Ok(Err(err)) => Err(map_error(err)),
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

    /// Whether `GET path` is refused with `401` and a `WWW-Authenticate: Basic` challenge: the
    /// server wants a username and password (Prometheus' `--web.config.file`, an nginx in front).
    /// Through the service proxy the challenge arrives, but credentials can't be sent back that
    /// way (the API server strips them).
    pub async fn asks_for_basic_auth(&self, path: &str, params: &[(&str, String)]) -> bool {
        let Ok(mut request) = http::Request::get(self.uri(path, params)).body(Vec::new()) else {
            return false;
        };
        if self.authorize(&mut request).await.is_err() {
            return false;
        }
        let response = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.client.send(request.map(kube::client::Body::from)),
        )
        .await;
        let Ok(Ok(response)) = response else {
            return false;
        };
        response.status() == http::StatusCode::UNAUTHORIZED
            && response
                .headers()
                .get_all(http::header::WWW_AUTHENTICATE)
                .iter()
                .any(|v| is_basic_challenge(v.to_str().unwrap_or_default()))
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

fn map_error(err: kube::Error) -> PromError {
    match err {
        kube::Error::Api(status) => {
            // Prometheus reports bad queries as 400/422 with its own JSON body.
            if let Ok(body) = serde_json::from_str::<Value>(&status.message)
                && body.get("status").and_then(Value::as_str) == Some("error")
            {
                return query_error(&body);
            }
            PromError::Http(status.code, short(&status.message))
        }
        err => PromError::Transport(short(&err.to_string())),
    }
}

/// `Basic realm="…"` (one of possibly several challenges in the header).
fn is_basic_challenge(value: &str) -> bool {
    value.split(',').any(|part| {
        part.split_whitespace()
            .next()
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic"))
    })
}

/// `Basic base64(username:password)`, for an Authorization header.
pub fn basic_authorization(username: &str, password: &SecretString) -> SecretString {
    use base64::Engine as _;
    let pair = format!("{username}:{}", password.expose_secret());
    SecretString::from(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(pair)
    ))
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

    #[test]
    fn basic_challenges_and_headers() {
        assert!(is_basic_challenge(r#"Basic realm="prometheus""#));
        assert!(is_basic_challenge("Bearer realm=\"x\", basic"));
        assert!(!is_basic_challenge(r#"Bearer realm="oauth""#));
        assert!(!is_basic_challenge(""));
        let header = basic_authorization("admin", &SecretString::from("s3cr3t".to_string()));
        assert_eq!(header.expose_secret(), "Basic YWRtaW46czNjcjN0");
    }

    /// A 401 with a Basic challenge asks for a password; one without (an auth proxy) doesn't.
    #[tokio::test]
    async fn basic_auth_is_told_apart_from_other_401s() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for challenge in ["www-authenticate: Basic realm=\"prom\"\r\n", ""] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0; 8192];
                let _ = socket.read(&mut buffer).await.unwrap();
                let answer = format!(
                    "HTTP/1.1 401 Unauthorized\r\n{challenge}content-length: 12\r\nconnection: close\r\n\r\nUnauthorized"
                );
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
        });
        let transport =
            Transport::external(&format!("http://{address}"), None, &ExternalTls::default())
                .unwrap();
        assert!(transport.asks_for_basic_auth("/api/v1/query", &[]).await);
        assert!(!transport.asks_for_basic_auth("/api/v1/query", &[]).await);
    }

    /// Streamed lines arrive while the response is still open (server-sent events); an error
    /// status fails the request itself.
    #[tokio::test]
    async fn lines_arrive_before_the_response_ends() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sent_first, first_sent) = tokio::sync::oneshot::channel::<()>();
        let (go_on, carry_on) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 8192];
            let n = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..n]).to_lowercase();
            assert!(request.starts_with("get /flows?watch=true"), "{request}");
            assert!(request.contains("accept: text/event-stream"), "{request}");
            let chunk = |text: &str| format!("{:x}\r\n{text}\r\n", text.len());
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
            socket
                .write_all(chunk("data: {\"n\":1}\n\n").as_bytes())
                .await
                .unwrap();
            sent_first.send(()).ok();
            carry_on.await.ok();
            socket
                .write_all(chunk("data: {\"n\":2}\n\n").as_bytes())
                .await
                .unwrap();
            socket.write_all(b"0\r\n\r\n").await.unwrap();
            drop(socket);
            // A second connection answers 403.
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut buffer).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 9\r\nconnection: close\r\n\r\nforbidden")
                .await
                .unwrap();
        });
        let transport =
            Transport::external(&format!("http://{address}"), None, &ExternalTls::default())
                .unwrap();
        let mut lines = transport
            .get_lines("/flows", &[("watch", "true".into())], "text/event-stream")
            .await
            .unwrap();
        first_sent.await.unwrap();
        assert_eq!(lines.next().await.unwrap().unwrap(), r#"data: {"n":1}"#);
        assert_eq!(lines.next().await.unwrap().unwrap(), "");
        go_on.send(()).unwrap();
        let rest: Vec<String> = lines.map(|l| l.unwrap()).collect().await;
        assert_eq!(rest, vec![r#"data: {"n":2}"#.to_string(), String::new()]);
        let err = transport
            .get_lines("/flows", &[], "text/event-stream")
            .await
            .err()
            .unwrap();
        assert!(matches!(err, PromError::Http(403, _)), "{err:?}");
        server.await.unwrap();
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

    /// A loopback transport sends no credentials, keeps its path prefix and streams lines that
    /// arrive after a quiet spell.
    #[tokio::test]
    async fn loopback_streams_without_credentials() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let n = socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: 1\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            socket.write_all(b"data: 2\n").await.unwrap();
            drop(socket);
            String::from_utf8_lossy(&request[..n]).to_string()
        });
        let transport = Transport::loopback(port, "whisker-backend/").unwrap();
        let lines: Vec<String> = transport
            .get_lines("/flows", &[("watch", "true".into())], "text/event-stream")
            .await
            .unwrap()
            .filter_map(|line| async move { line.ok() })
            .collect()
            .await;
        assert_eq!(lines, ["data: 1", "data: 2"]);
        let request = server.await.unwrap().to_lowercase();
        assert!(
            request.starts_with("get /whisker-backend/flows?watch=true "),
            "{request}"
        );
        assert!(!request.contains("authorization"));
    }
}
