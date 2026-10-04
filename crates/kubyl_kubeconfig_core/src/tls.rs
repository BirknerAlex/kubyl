//! TLS checks for the connection test, and fetching a cluster's CA (trust on first use).
//!
//! Both only do a TLS handshake: no client certificate, no `Authorization` header, nothing a
//! server could misuse. The CA fetch additionally asks for `kube-public/cluster-info`
//! anonymously (kubeadm clusters such as kind publish their CA there, readable without
//! credentials) over a connection that isn't verified yet. A CA found that way (or in the
//! certificates the server sent) is only offered when it verifies the server's certificate,
//! and the user confirms its fingerprint before it's used.
//!
//! Runs on the Tokio runtime (`kubyl_core::spawn_kube`).

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::{CertificateError, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use crate::certs::{self, CertInfo};

/// Connect and handshake timeout.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The most `cluster-info` bytes read.
const MAX_BODY: usize = 1 << 20;

/// Where to connect and which name the certificate must be valid for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    /// `tls-server-name`, else the host.
    pub server_name: String,
    pub https: bool,
}

impl Target {
    /// From a kubeconfig `server` URL and optional `tls-server-name`.
    pub fn parse(server: &str, tls_server_name: Option<&str>) -> Result<Self, String> {
        let url = url::Url::parse(server.trim())
            .map_err(|e| format!("\"{server}\" isn't a valid URL: {e}"))?;
        let https = match url.scheme() {
            "https" => true,
            "http" => false,
            other => {
                return Err(format!(
                    "the server URL must start with https:// (not {other}://)"
                ));
            }
        };
        let host = url
            .host_str()
            .ok_or_else(|| format!("\"{server}\" has no host"))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let port = url.port_or_known_default().unwrap_or(443);
        let server_name = tls_server_name
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| host.clone());
        Ok(Self {
            host,
            port,
            server_name,
            https,
        })
    }

    /// `host:port` for display (IPv6 in brackets).
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// What the TLS step trusts.
#[derive(Clone, Debug)]
pub enum Trust {
    /// The OS trust store (no CA in the kubeconfig).
    System,
    /// PEM text of the kubeconfig's CA.
    Pem(String),
    /// `insecure-skip-tls-verify`: nothing is verified.
    Insecure,
}

/// The result of a handshake.
#[derive(Clone, Debug)]
pub struct Handshake {
    /// `TLS 1.3`.
    pub protocol: String,
    /// What the server sent, leaf first.
    pub chain: Vec<CertInfo>,
    pub millis: u64,
}

/// Why a handshake failed.
#[derive(Clone, Debug, PartialEq)]
pub enum TlsError {
    /// Couldn't connect at all.
    Connect(String),
    /// The server's certificate isn't signed by a trusted CA.
    UnknownIssuer,
    /// The certificate is for other names.
    WrongName {
        expected: String,
        names: Vec<String>,
    },
    Expired {
        not_after: String,
    },
    NotYetValid,
    /// Anything else (alerts, protocol errors, bad signatures).
    Other(String),
}

impl TlsError {
    /// A plain-language explanation.
    pub fn message(&self) -> String {
        match self {
            TlsError::Connect(err) => err.clone(),
            TlsError::UnknownIssuer => "certificate signed by unknown authority: the server's CA isn't the one this kubeconfig trusts".into(),
            TlsError::WrongName { expected, names } if names.is_empty() => {
                format!("the server's certificate isn't valid for {expected}")
            }
            TlsError::WrongName { expected, names } => format!(
                "the server's certificate isn't valid for {expected} (it's for {}); set the TLS server name if you connect through another address",
                names.join(", ")
            ),
            TlsError::Expired { not_after } => {
                format!("the server's certificate expired on {not_after}")
            }
            TlsError::NotYetValid => {
                "the server's certificate isn't valid yet (is this computer's clock right?)".into()
            }
            TlsError::Other(err) => format!("TLS handshake failed: {err}"),
        }
    }
}

fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The OS trust store, loaded once.
fn system_roots() -> Arc<RootCertStore> {
    static ROOTS: OnceLock<Arc<RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut store = RootCertStore::empty();
            let loaded = rustls_native_certs::load_native_certs();
            for cert in loaded.certs {
                store.add(cert).ok();
            }
            Arc::new(store)
        })
        .clone()
}

/// A root store with the certificates of PEM text.
pub fn roots_from_pem(pem: &str) -> Result<Arc<RootCertStore>, String> {
    let mut store = RootCertStore::empty();
    let blocks = certs::pem_blocks(pem);
    for block in blocks.iter().filter(|b| b.label == "CERTIFICATE") {
        store
            .add(CertificateDer::from(block.der.clone()))
            .map_err(|e| format!("the CA certificate isn't usable: {e}"))?;
    }
    if store.is_empty() {
        return Err("the CA data holds no certificate".into());
    }
    Ok(Arc::new(store))
}

/// Records the chain the server sent, then verifies it (or not, for `inner: None`).
#[derive(Debug)]
struct Recorder {
    inner: Option<Arc<WebPkiServerVerifier>>,
    chain: Arc<Mutex<Vec<Vec<u8>>>>,
    error: Arc<Mutex<Option<rustls::Error>>>,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut chain = vec![end_entity.to_vec()];
        chain.extend(intermediates.iter().map(|c| c.to_vec()));
        *self.chain.lock() = chain;
        match &self.inner {
            Some(inner) => inner
                .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
                .inspect_err(|err| *self.error.lock() = Some(err.clone())),
            None => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn verifier(roots: Arc<RootCertStore>) -> Result<Arc<WebPkiServerVerifier>, String> {
    WebPkiServerVerifier::builder_with_provider(roots, provider())
        .build()
        .map_err(|e| e.to_string())
}

type Stream = tokio_rustls::client::TlsStream<TcpStream>;

async fn connect(
    target: &Target,
    inner: Option<Arc<WebPkiServerVerifier>>,
) -> Result<(Stream, Handshake), (TlsError, Vec<CertInfo>)> {
    let started = Instant::now();
    let chain = Arc::new(Mutex::new(Vec::new()));
    let error = Arc::new(Mutex::new(None));
    let recorder = Recorder {
        inner,
        chain: chain.clone(),
        error: error.clone(),
    };
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| (TlsError::Other(e.to_string()), Vec::new()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(recorder))
        .with_no_client_auth();
    let name = ServerName::try_from(target.server_name.clone()).map_err(|e| {
        (
            TlsError::Other(format!("invalid server name: {e}")),
            Vec::new(),
        )
    })?;
    let tcp = tokio::time::timeout(
        TIMEOUT,
        TcpStream::connect((target.host.as_str(), target.port)),
    )
    .await
    .map_err(|_| {
        (
            TlsError::Connect(format!("connecting to {} timed out", target.authority())),
            Vec::new(),
        )
    })?
    .map_err(|e| {
        (
            TlsError::Connect(format!("couldn't connect to {}: {e}", target.authority())),
            Vec::new(),
        )
    })?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let result = tokio::time::timeout(TIMEOUT, connector.connect(name, tcp)).await;
    let served: Vec<CertInfo> = chain
        .lock()
        .iter()
        .filter_map(|der| CertInfo::parse(der).ok())
        .collect();
    let stream = match result {
        Err(_) => {
            return Err((
                TlsError::Other("the TLS handshake timed out".into()),
                served,
            ));
        }
        Ok(Err(io_err)) => {
            let err = error.lock().clone();
            return Err((classify(err, &io_err, target, &served), served));
        }
        Ok(Ok(stream)) => stream,
    };
    let protocol = match stream.get_ref().1.protocol_version() {
        Some(rustls::ProtocolVersion::TLSv1_3) => "TLS 1.3".to_string(),
        Some(rustls::ProtocolVersion::TLSv1_2) => "TLS 1.2".to_string(),
        Some(other) => format!("{other:?}"),
        None => "TLS".to_string(),
    };
    let handshake = Handshake {
        protocol,
        chain: served,
        millis: started.elapsed().as_millis() as u64,
    };
    Ok((stream, handshake))
}

fn classify(
    verify: Option<rustls::Error>,
    io_err: &std::io::Error,
    target: &Target,
    served: &[CertInfo],
) -> TlsError {
    let names = served.first().map(|c| c.names.clone()).unwrap_or_default();
    match verify {
        Some(rustls::Error::InvalidCertificate(err)) => match err {
            CertificateError::UnknownIssuer | CertificateError::BadSignature => {
                TlsError::UnknownIssuer
            }
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                TlsError::WrongName {
                    expected: target.server_name.clone(),
                    names,
                }
            }
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                TlsError::Expired {
                    not_after: served
                        .first()
                        .map(|c| c.not_after_date())
                        .unwrap_or_default(),
                }
            }
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                TlsError::NotYetValid
            }
            other => TlsError::Other(format!("{other:?}")),
        },
        Some(other) => TlsError::Other(other.to_string()),
        None => TlsError::Other(io_err.to_string()),
    }
}

/// A handshake that verifies the server against `trust`. Sends nothing after it.
pub async fn check(target: &Target, trust: &Trust) -> Result<Handshake, (TlsError, Vec<CertInfo>)> {
    let inner = match trust {
        Trust::Insecure => None,
        Trust::System => {
            Some(verifier(system_roots()).map_err(|e| (TlsError::Other(e), Vec::new()))?)
        }
        Trust::Pem(pem) => {
            let roots = roots_from_pem(pem).map_err(|e| (TlsError::Other(e), Vec::new()))?;
            Some(verifier(roots).map_err(|e| (TlsError::Other(e), Vec::new()))?)
        }
    };
    let (mut stream, handshake) = connect(target, inner).await?;
    stream.shutdown().await.ok();
    Ok(handshake)
}

/// Where a CA candidate came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaOrigin {
    /// `kube-public/cluster-info`, read anonymously.
    ClusterInfo,
    /// The certificate chain the server sent.
    ServedChain,
}

impl CaOrigin {
    pub fn label(&self) -> &'static str {
        match self {
            CaOrigin::ClusterInfo => "kube-public/cluster-info (anonymous)",
            CaOrigin::ServedChain => "the server's certificate chain",
        }
    }
}

/// A CA that verifies the server's certificate.
#[derive(Clone, Debug)]
pub struct CaCandidate {
    pub cert: CertInfo,
    pub origin: CaOrigin,
}

/// What fetching the CA found.
#[derive(Clone, Debug)]
pub struct FetchedCa {
    /// The server's certificate chain, leaf first.
    pub served: Vec<CertInfo>,
    /// CAs that verify it, best first.
    pub candidates: Vec<CaCandidate>,
    /// Why `cluster-info` gave nothing (shown when there is no candidate).
    pub cluster_info_error: Option<String>,
}

/// Fetches the CA of the server at `target` (trust on first use; see the module docs).
pub async fn fetch_ca(target: &Target) -> Result<FetchedCa, String> {
    if !target.https {
        return Err("the server uses plain HTTP; there is no CA to fetch".into());
    }
    let (mut stream, handshake) = connect(target, None)
        .await
        .map_err(|(err, _)| err.message())?;
    let served = handshake.chain.clone();
    let body = cluster_info(&mut stream, target).await;
    stream.shutdown().await.ok();

    let mut found: Vec<(CertInfo, CaOrigin)> = Vec::new();
    let cluster_info_error = match body.and_then(|b| ca_from_cluster_info(&b)) {
        Ok(pem) => {
            for cert in certs::certificates(&pem).unwrap_or_default() {
                found.push((cert, CaOrigin::ClusterInfo));
            }
            None
        }
        Err(err) => Some(err),
    };
    for cert in served.iter().skip(1).rev() {
        if cert.is_ca && !found.iter().any(|(c, _)| c.sha256 == cert.sha256) {
            found.push((cert.clone(), CaOrigin::ServedChain));
        }
    }
    let candidates = found
        .into_iter()
        .filter(|(cert, _)| verifies(cert, &served, target))
        .map(|(cert, origin)| CaCandidate { cert, origin })
        .collect();
    Ok(FetchedCa {
        served,
        candidates,
        cluster_info_error,
    })
}

/// Whether `ca` verifies the served chain for the target's name.
pub fn verifies(ca: &CertInfo, served: &[CertInfo], target: &Target) -> bool {
    let Some((leaf, intermediates)) = served.split_first() else {
        return false;
    };
    let mut store = RootCertStore::empty();
    if store.add(CertificateDer::from(ca.der.clone())).is_err() {
        return false;
    }
    let Ok(verifier) = verifier(Arc::new(store)) else {
        return false;
    };
    let Ok(name) = ServerName::try_from(target.server_name.clone()) else {
        return false;
    };
    let intermediates: Vec<CertificateDer<'static>> = intermediates
        .iter()
        .map(|c| CertificateDer::from(c.der.clone()))
        .collect();
    verifier
        .verify_server_cert(
            &CertificateDer::from(leaf.der.clone()),
            &intermediates,
            &name,
            &[],
            UnixTime::now(),
        )
        .is_ok()
}

/// `GET /api/v1/namespaces/kube-public/configmaps/cluster-info` without credentials (HTTP/1.0,
/// so the body ends with the connection). Returns the body of a 200.
async fn cluster_info(stream: &mut Stream, target: &Target) -> Result<Vec<u8>, String> {
    let request = format!(
        "GET /api/v1/namespaces/kube-public/configmaps/cluster-info HTTP/1.0\r\nHost: {}\r\nAccept: application/json\r\nUser-Agent: kubyl\r\n\r\n",
        target.authority()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("couldn't ask for cluster-info: {e}"))?;
    let mut response = Vec::new();
    let read = async {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    response.extend_from_slice(&buf[..n]);
                    if response.len() > MAX_BODY {
                        return Err("cluster-info is too large".to_string());
                    }
                }
                // Servers often close without a TLS close_notify.
                Err(_) if !response.is_empty() => break,
                Err(e) => return Err(format!("couldn't read cluster-info: {e}")),
            }
        }
        Ok(())
    };
    tokio::time::timeout(TIMEOUT, read)
        .await
        .map_err(|_| "reading cluster-info timed out".to_string())??;
    let split = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("the server's answer isn't HTTP")?;
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("the server's answer isn't HTTP")?;
    match status {
        200 => Ok(response[split + 4..].to_vec()),
        401 | 403 => Err("the server doesn't publish cluster-info to anonymous users".into()),
        404 => Err("the server has no kube-public/cluster-info (not a kubeadm cluster)".into()),
        other => Err(format!("cluster-info answered HTTP {other}")),
    }
}

/// The CA (PEM) in a `cluster-info` ConfigMap: its `kubeconfig` data holds one cluster.
fn ca_from_cluster_info(body: &[u8]) -> Result<String, String> {
    let configmap: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| "cluster-info isn't JSON".to_string())?;
    let kubeconfig = configmap
        .get("data")
        .and_then(|d| d.get("kubeconfig"))
        .and_then(|k| k.as_str())
        .ok_or("cluster-info has no kubeconfig")?;
    let doc = crate::yaml::load(kubeconfig)?;
    let data = doc
        .get("clusters")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("cluster"))
        .and_then(|c| c.get("certificate-authority-data"))
        .and_then(|d| d.as_str())
        .ok_or("cluster-info has no CA")?;
    certs::pem_from_data(data)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::certs::tests::{CA, OTHER_CA};

    const SERVER: &str = include_str!("../tests/fixtures/server.pem");
    const SERVER_KEY: &str = include_str!("../tests/fixtures/server-key.pem");

    /// A local TLS server with the fixture certificate. It answers every request with
    /// `response` and records the requests it got.
    pub(crate) async fn server(response: String) -> (u16, Arc<Mutex<Vec<String>>>) {
        let certs: Vec<CertificateDer<'static>> = certs::pem_blocks(SERVER)
            .into_iter()
            .map(|b| CertificateDer::from(b.der))
            .collect();
        let key_der = certs::pem_blocks(SERVER_KEY).remove(0).der;
        let key = rustls_pki_types::PrivateKeyDer::Pkcs8(key_der.into());
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                let response = response.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut buf = vec![0u8; 4096];
                    let n = tls.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    seen.lock()
                        .push(String::from_utf8_lossy(&buf[..n]).to_string());
                    tls.write_all(response.as_bytes()).await.ok();
                    tls.shutdown().await.ok();
                });
            }
        });
        (port, requests)
    }

    fn cluster_info_response() -> String {
        let kubeconfig = format!(
            "apiVersion: v1\nkind: Config\nclusters:\n- name: \"\"\n  cluster:\n    certificate-authority-data: {}\n    server: https://127.0.0.1:6443\n",
            certs::data_from_pem(CA)
        );
        let body = serde_json::json!({"kind": "ConfigMap", "data": {"kubeconfig": kubeconfig}})
            .to_string();
        format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{body}")
    }

    fn target(port: u16) -> Target {
        Target::parse(&format!("https://127.0.0.1:{port}"), None).unwrap()
    }

    #[test]
    fn parses_server_urls() {
        let t = Target::parse("https://[::1]:6443", Some("kubernetes")).unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.server_name.as_str()),
            ("::1", 6443, "kubernetes")
        );
        assert_eq!(t.authority(), "[::1]:6443");
        assert_eq!(
            Target::parse("https://example.com", None).unwrap().port,
            443
        );
        assert!(!Target::parse("http://x:8080", None).unwrap().https);
        assert!(Target::parse("ftp://x", None).is_err());
        assert!(Target::parse("not a url", None).is_err());
    }

    #[tokio::test]
    async fn verifies_with_the_right_ca_and_explains_the_wrong_one() {
        let (port, requests) = server(String::new()).await;
        let ok = check(&target(port), &Trust::Pem(CA.into())).await.unwrap();
        assert!(ok.protocol.starts_with("TLS"));
        assert_eq!(ok.chain[0].common_name(), "kube-apiserver");

        let (err, served) = check(&target(port), &Trust::Pem(OTHER_CA.into()))
            .await
            .unwrap_err();
        assert_eq!(err, TlsError::UnknownIssuer);
        assert!(err.message().contains("unknown authority"));
        assert_eq!(served[0].common_name(), "kube-apiserver");

        let wrong_name = Target::parse(
            &format!("https://127.0.0.1:{port}"),
            Some("other.example.com"),
        )
        .unwrap();
        let (err, _) = check(&wrong_name, &Trust::Pem(CA.into()))
            .await
            .unwrap_err();
        assert!(matches!(err, TlsError::WrongName { .. }), "{err:?}");
        assert!(err.message().contains("127.0.0.1"), "{}", err.message());

        // Insecure: nothing verified, details still shown.
        let insecure = check(&target(port), &Trust::Insecure).await.unwrap();
        assert_eq!(insecure.chain.len(), 1);
        // The checks never send a request.
        assert!(requests.lock().is_empty());

        let closed = Target::parse("https://127.0.0.1:1", None).unwrap();
        assert!(matches!(
            check(&closed, &Trust::System).await,
            Err((TlsError::Connect(_), _))
        ));
    }

    #[tokio::test]
    async fn fetches_the_ca_from_cluster_info_anonymously() {
        let (port, requests) = server(cluster_info_response()).await;
        let fetched = fetch_ca(&target(port)).await.unwrap();
        assert_eq!(fetched.candidates.len(), 1);
        let ca = &fetched.candidates[0];
        assert_eq!(ca.origin, CaOrigin::ClusterInfo);
        assert_eq!(ca.cert.common_name(), "kubyl-test-ca");
        let request = requests.lock()[0].clone();
        assert!(request.starts_with("GET /api/v1/namespaces/kube-public/configmaps/cluster-info"));
        assert!(!request.to_ascii_lowercase().contains("authorization"));

        // A CA that doesn't sign the server's certificate isn't offered.
        let other = format!(
            "HTTP/1.0 200 OK\r\n\r\n{}",
            serde_json::json!({"data": {"kubeconfig": format!(
                "clusters:\n- cluster:\n    certificate-authority-data: {}\n", certs::data_from_pem(OTHER_CA))}})
        );
        let (port, _) = server(other).await;
        let fetched = fetch_ca(&target(port)).await.unwrap();
        assert!(fetched.candidates.is_empty());

        let (port, _) = server("HTTP/1.0 403 Forbidden\r\n\r\n{}".into()).await;
        let fetched = fetch_ca(&target(port)).await.unwrap();
        assert!(fetched.candidates.is_empty());
        assert!(fetched.cluster_info_error.unwrap().contains("anonymous"));
    }
}
