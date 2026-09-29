//! The HTTP stack under every `kube::Client`: kube's default stack, but speaking HTTP/2.
//!
//! kube only offers HTTP/1.1, where every watch and in-flight request holds its own socket; a
//! few clusters then use hundreds of file descriptors. Here TLS offers `h2` and `http/1.1`, so
//! all requests to an API server that speaks HTTP/2 share one connection, kept alive and
//! checked with HTTP/2 pings.
//!
//! WebSocket upgrades (exec, attach, port-forward) only work over HTTP/1.1: requests with an
//! `Upgrade` header go to a second, HTTP/1.1-only client.
//!
//! Everything else mirrors kube's `ClientBuilder::try_from(Config)`: proxies (HTTP, HTTPS,
//! SOCKS5), the TLS server name, timeouts, gzip, retries, auth and extra headers. kube's
//! per-request trace layer is left out: its error lines carry no cause, and connection
//! failures are reported by the connection manager.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use http::{Request, Response, Uri};
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_timeout::TimeoutConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioTimer};
use kube::Config;
use kube::client::retry::RetryPolicy;
use kube::client::{Body, ClientBuilder, ConfigExt as _, DynBody};
use tower::retry::RetryLayer;
use tower::util::BoxService;
use tower::{BoxError, Service, ServiceBuilder, ServiceExt as _};

/// HTTP/2 PING interval; a connection that doesn't answer within [`KEEP_ALIVE_TIMEOUT`] is
/// closed, which ends every watch on it so they reconnect.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// The boxed stack a `kube::Client` is built from.
pub type KubeService = BoxService<Request<Body>, Response<Box<DynBody>>, BoxError>;

/// Open API server connections, by protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenConnections {
    pub http2: usize,
    pub http1: usize,
}

impl OpenConnections {
    pub fn total(&self) -> usize {
        self.http2 + self.http1
    }
}

/// Live connection counters of one client (and, in [`TOTAL`], of all of them).
#[derive(Debug, Default)]
pub struct ConnectionCounts {
    http2: AtomicUsize,
    http1: AtomicUsize,
}

impl ConnectionCounts {
    pub fn snapshot(&self) -> OpenConnections {
        OpenConnections {
            http2: self.http2.load(Ordering::Relaxed),
            http1: self.http1.load(Ordering::Relaxed),
        }
    }

    fn counter(&self, http2: bool) -> &AtomicUsize {
        if http2 { &self.http2 } else { &self.http1 }
    }
}

static TOTAL: ConnectionCounts = ConnectionCounts {
    http2: AtomicUsize::new(0),
    http1: AtomicUsize::new(0),
};

/// Open API server connections of all clients, including ones still closing after their
/// cluster disconnected.
pub fn open_connections() -> OpenConnections {
    TOTAL.snapshot()
}

/// Builds kube's client stack for `config`, counting its connections into `counts`.
pub fn client_builder(
    config: &Config,
    counts: Arc<ConnectionCounts>,
) -> Result<ClientBuilder<KubeService>, BoxError> {
    let mut connector = HttpConnector::new();
    connector.enforce_http(false);
    match config.proxy_url.as_ref() {
        Some(proxy) if proxy.scheme_str() == Some("socks5") => {
            let connector =
                hyper_util::client::legacy::connect::proxy::SocksV5::new(proxy.clone(), connector);
            generic_builder(connector, config, counts)
        }
        Some(proxy) if proxy.scheme_str() == Some("http") => {
            let connector =
                hyper_util::client::legacy::connect::proxy::Tunnel::new(proxy.clone(), connector);
            generic_builder(with_proxy_auth(proxy, connector), config, counts)
        }
        Some(proxy) if proxy.scheme_str() == Some("https") => {
            // TLS to the proxy itself: HTTP/1.1 (CONNECT).
            let to_proxy = config.rustls_https_connector_with_connector(connector)?;
            let connector =
                hyper_util::client::legacy::connect::proxy::Tunnel::new(proxy.clone(), to_proxy);
            generic_builder(with_proxy_auth(proxy, connector), config, counts)
        }
        Some(proxy) => Err(format!("unsupported proxy protocol: {proxy}").into()),
        None => generic_builder(connector, config, counts),
    }
}

/// `Proxy-Authorization: Basic …` from the proxy URL's user info.
fn with_proxy_auth<C>(
    proxy: &Uri,
    connector: hyper_util::client::legacy::connect::proxy::Tunnel<C>,
) -> hyper_util::client::legacy::connect::proxy::Tunnel<C> {
    use base64::Engine as _;
    let Some((userinfo, _)) = proxy.authority().and_then(|a| a.as_str().split_once('@')) else {
        return connector;
    };
    let value = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(userinfo)
    );
    match http::HeaderValue::from_str(&value) {
        Ok(header) => connector.with_auth(header),
        Err(_) => connector,
    }
}

fn generic_builder<H>(
    connector: H,
    config: &Config,
    counts: Arc<ConnectionCounts>,
) -> Result<ClientBuilder<KubeService>, BoxError>
where
    H: Service<Uri> + Clone + Send + Sync + 'static,
    H::Response: Read + Write + Connection + Send + Unpin + 'static,
    H::Future: Send + 'static,
    H::Error: Into<BoxError>,
{
    let multiplexed = https_connector(connector.clone(), config, true)?;
    let multiplexed = timeouts(Counted::new(multiplexed, counts.clone()), config);
    let upgrades = https_connector(connector, config, false)?;
    let upgrades = timeouts(Counted::new(upgrades, counts), config);

    let http2 = HyperClient::builder(TokioExecutor::new())
        .timer(TokioTimer::new())
        .pool_timer(TokioTimer::new())
        .http2_keep_alive_interval(KEEP_ALIVE_INTERVAL)
        .http2_keep_alive_timeout(KEEP_ALIVE_TIMEOUT)
        .http2_keep_alive_while_idle(true)
        .build(multiplexed);
    let http1 = HyperClient::builder(TokioExecutor::new())
        .timer(TokioTimer::new())
        .pool_timer(TokioTimer::new())
        .build(upgrades);
    let router = Router { http2, http1 };

    let decompression = tower_http::decompression::DecompressionLayer::new()
        .no_br()
        .no_deflate()
        .no_zstd()
        .gzip(!config.disable_compression);
    let service = ServiceBuilder::new()
        .layer(config.base_uri_layer())
        .layer(decompression)
        .option_layer(
            config
                .default_retry
                .then(|| RetryLayer::new(RetryPolicy::server_retry())),
        )
        .option_layer(config.auth_layer()?)
        .layer(config.extra_headers_layer()?)
        .map_err(BoxError::from)
        .service(router)
        .map_response(|response| {
            response.map(|body| {
                Box::new(http_body_util::BodyExt::map_err(body, BoxError::from)) as Box<DynBody>
            })
        })
        .boxed();
    Ok(ClientBuilder::new(
        service,
        config.default_namespace.clone(),
    ))
}

/// TLS over `connector` with the kubeconfig's certificates and server name. `multiplexed`
/// offers HTTP/2 (falling back to HTTP/1.1 when the server doesn't pick it).
fn https_connector<H>(
    connector: H,
    config: &Config,
    multiplexed: bool,
) -> Result<hyper_rustls::HttpsConnector<H>, BoxError> {
    let builder = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(config.rustls_client_config()?)
        .https_or_http();
    let builder = match &config.tls_server_name {
        Some(name) => {
            let name = rustls_pki_types::ServerName::try_from(name.clone())
                .map_err(|err| format!("invalid TLS server name {name:?}: {err}"))?;
            builder.with_server_name_resolver(hyper_rustls::FixedServerNameResolver::new(name))
        }
        None => builder,
    };
    Ok(if multiplexed {
        builder.enable_all_versions().wrap_connector(connector)
    } else {
        builder.enable_http1().wrap_connector(connector)
    })
}

fn timeouts<C>(connector: C, config: &Config) -> TimeoutConnector<C>
where
    C: Service<Uri> + Send,
    C::Response: Read + Write + Connection + Send + Unpin,
    C::Future: Send + 'static,
    C::Error: Into<BoxError>,
{
    let mut connector = TimeoutConnector::new(connector);
    connector.set_connect_timeout(config.connect_timeout);
    connector.set_read_timeout(config.read_timeout);
    connector.set_write_timeout(config.write_timeout);
    connector
}

/// Sends WebSocket upgrades over HTTP/1.1, everything else over the multiplexed client.
#[derive(Clone)]
struct Router<C: Clone> {
    http2: HyperClient<C, Body>,
    http1: HyperClient<C, Body>,
}

impl<C> Service<Request<Body>> for Router<C>
where
    C: hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static,
{
    type Response = Response<hyper::body::Incoming>;
    type Error = hyper_util::client::legacy::Error;
    type Future = hyper_util::client::legacy::ResponseFuture;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        if is_upgrade(&request) {
            self.http1.request(request)
        } else {
            self.http2.request(request)
        }
    }
}

fn is_upgrade<B>(request: &Request<B>) -> bool {
    request.headers().contains_key(http::header::UPGRADE)
}

/// Counts the connections a connector opens, until they're dropped.
#[derive(Clone)]
struct Counted<C> {
    inner: C,
    counts: Arc<ConnectionCounts>,
}

impl<C> Counted<C> {
    fn new(inner: C, counts: Arc<ConnectionCounts>) -> Self {
        Self { inner, counts }
    }
}

impl<C> Service<Uri> for Counted<C>
where
    C: Service<Uri>,
    C::Response: Connection,
    C::Future: Send + 'static,
{
    type Response = CountedStream<C::Response>;
    type Error = C::Error;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let connecting = self.inner.call(uri);
        let counts = self.counts.clone();
        Box::pin(async move {
            let stream = connecting.await?;
            Ok(CountedStream::new(stream, counts))
        })
    }
}

struct CountedStream<S> {
    inner: S,
    counts: Arc<ConnectionCounts>,
    http2: bool,
}

impl<S: Connection> CountedStream<S> {
    fn new(inner: S, counts: Arc<ConnectionCounts>) -> Self {
        let http2 = inner.connected().is_negotiated_h2();
        counts.counter(http2).fetch_add(1, Ordering::Relaxed);
        TOTAL.counter(http2).fetch_add(1, Ordering::Relaxed);
        Self {
            inner,
            counts,
            http2,
        }
    }
}

impl<S> Drop for CountedStream<S> {
    fn drop(&mut self) {
        self.counts
            .counter(self.http2)
            .fetch_sub(1, Ordering::Relaxed);
        TOTAL.counter(self.http2).fetch_sub(1, Ordering::Relaxed);
    }
}

impl<S: Connection> Connection for CountedStream<S> {
    fn connected(&self) -> Connected {
        self.inner.connected()
    }
}

impl<S: Read + Unpin> Read for CountedStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: Write + Unpin> Write for CountedStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
}

/// This process's open file descriptors and its soft limit, where the platform tells.
/// Reads a directory: call it off the UI thread.
pub fn open_files() -> Option<(usize, Option<u64>)> {
    let dir = if cfg!(target_os = "macos") {
        "/dev/fd"
    } else if cfg!(target_os = "linux") {
        "/proc/self/fd"
    } else {
        return None;
    };
    // The directory handle itself is one of the entries.
    let open = std::fs::read_dir(dir).ok()?.count().saturating_sub(1);
    Some((open, fd_limit()))
}

#[cfg(unix)]
fn fd_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid, writable rlimit.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0).then_some(limit.rlim_cur)
}

#[cfg(not(unix))]
fn fd_limit() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_upgrades_take_the_http1_client() {
        let upgrade = Request::get("/api/v1/namespaces/a/pods/b/exec")
            .header(http::header::CONNECTION, "Upgrade")
            .header(http::header::UPGRADE, "websocket")
            .body(())
            .unwrap();
        assert!(is_upgrade(&upgrade));
        let watch = Request::get("/api/v1/pods?watch=true").body(()).unwrap();
        assert!(!is_upgrade(&watch));
    }
}
