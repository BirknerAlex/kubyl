//! Building a `kube::Client` for a context and checking that it can talk to the cluster.

use std::sync::Arc;
use std::time::{Duration, Instant};

use http::Uri;
use k8s_openapi::api::authentication::v1::SelfSubjectReview;
use kube::config::{AuthInfo, ExecAuthCluster, KubeConfigOptions, Kubeconfig};
use kube::{Api, Client, Config};
use tower::buffer::BufferLayer;
use tower::filter::AsyncFilterLayer;

use crate::auth::oidc::OidcSecrets;
use crate::auth::openshift::OpenShiftParams;
use crate::auth::{
    AuthError, AuthLayer, AuthMethod, BearerToken, CredentialSource, ExecAuth, OidcAuth,
    OpenShiftAuth, exec,
};
use crate::kubeconfig::ContextInfo;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Requests queued in front of the auth layer.
const BUFFER_SIZE: usize = 1024;

/// Why connecting failed. Messages never contain credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectError {
    /// OIDC needs an interactive sign-in.
    SignInRequired,
    /// Credentials were rejected or couldn't be obtained. `detail` is exec plugin stderr.
    Auth {
        message: String,
        detail: Option<String>,
    },
    /// Authenticated, but not allowed to do even basic reads.
    Forbidden(String),
    /// Network, TLS or server errors.
    Unreachable(String),
    /// The kubeconfig entry is unusable.
    Config(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::SignInRequired => f.write_str("sign-in required"),
            ConnectError::Auth { message, .. } => write!(f, "authentication failed: {message}"),
            ConnectError::Forbidden(message) => write!(f, "forbidden: {message}"),
            ConnectError::Unreachable(message) => f.write_str(message),
            ConnectError::Config(message) => write!(f, "invalid kubeconfig: {message}"),
        }
    }
}

impl ConnectError {
    /// Classifies a kube error.
    pub fn from_kube(err: &kube::Error) -> Self {
        if let Some(auth) = AuthError::find(err) {
            return match auth {
                AuthError::SignInRequired => ConnectError::SignInRequired,
                AuthError::Exec { message, stderr } => ConnectError::Auth {
                    message: message.clone(),
                    detail: stderr.clone(),
                },
                AuthError::Failed(message) => ConnectError::Auth {
                    message: message.clone(),
                    detail: None,
                },
            };
        }
        match err {
            kube::Error::Api(status) if status.code == 401 => ConnectError::Auth {
                message: "the API server rejected the credentials (401 Unauthorized)".into(),
                detail: None,
            },
            kube::Error::Api(status) if status.code == 403 => {
                ConnectError::Forbidden(status.message.clone())
            }
            kube::Error::Api(status) => {
                ConnectError::Unreachable(format!("{} ({})", status.message, status.code))
            }
            other => ConnectError::Unreachable(error_chain(other)),
        }
    }
}

/// `error: source: source…`, deduplicated.
pub(crate) fn error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(err) = source {
        let next = err.to_string();
        if !message.contains(&next) {
            message.push_str(": ");
            message.push_str(&next);
        }
        source = err.source();
    }
    message
}

/// A client plus what Kubyl manages for it.
#[derive(Clone)]
pub struct BuiltClient {
    pub client: Client,
    pub credentials: Option<CredentialSource>,
    /// The user's bearer token, when they authenticate with one (not client certificates).
    pub bearer: Option<BearerToken>,
    pub default_namespace: String,
    /// The proxy in use (kubeconfig `proxy-url` or `HTTPS_PROXY`).
    pub proxy: Option<String>,
    /// A client certificate from an exec plugin expires then; the client must be rebuilt.
    pub rebuild_at: Option<jiff::Timestamp>,
}

/// Builds a client for `info` from its parsed kubeconfig. Runs exec plugins that return
/// client certificates (they're needed for the TLS setup); token plugins run lazily.
pub async fn build(
    info: &ContextInfo,
    kubeconfig: Arc<Kubeconfig>,
) -> Result<BuiltClient, ConnectError> {
    let options = KubeConfigOptions {
        context: Some(info.context.clone()),
        ..Default::default()
    };
    let mut config = Config::from_custom_kubeconfig((*kubeconfig).clone(), &options)
        .await
        .map_err(|err| ConnectError::Config(error_chain(&err)))?;
    config.connect_timeout = Some(CONNECT_TIMEOUT);
    if config.proxy_url.is_none() {
        config.proxy_url = env_proxy(&config.cluster_url);
    }

    let mut credentials = None;
    let mut rebuild_at = None;
    match &info.auth {
        AuthMethod::Oidc(params) => {
            let secrets = oidc_secrets(&config.auth_info);
            credentials = Some(CredentialSource::Oidc(Arc::new(OidcAuth::new(
                params.clone(),
                secrets,
            ))));
            strip_managed_auth(&mut config.auth_info);
        }
        AuthMethod::OpenShift => {
            // The kubeconfig token is only the first one to try; see `auth::openshift`.
            let token = config.auth_info.token.take();
            credentials = Some(CredentialSource::OpenShift(OpenShiftAuth::shared(
                OpenShiftParams {
                    server: info
                        .server
                        .clone()
                        .unwrap_or_else(|| config.cluster_url.to_string()),
                    user: info.user.clone().unwrap_or_default(),
                    roots: config.root_cert.clone().unwrap_or_default(),
                    insecure: config.accept_invalid_certs,
                    proxy: config.proxy_url.as_ref().map(|u| u.to_string()),
                },
                token,
            )));
        }
        AuthMethod::Exec(_) => {
            let exec_config = config
                .auth_info
                .exec
                .clone()
                .expect("exec auth has an exec config");
            let cluster = kubeconfig
                .clusters
                .iter()
                .find(|c| c.name == info.cluster)
                .and_then(|c| c.cluster.as_ref())
                .and_then(|c| ExecAuthCluster::try_from(c).ok());
            let auth = Arc::new(ExecAuth::new(info.name.clone(), exec_config, cluster));
            match auth.credential().await {
                Ok(exec::Credential::ClientCertificate { certificate, key }) => {
                    config.auth_info.client_certificate_data = Some(certificate);
                    config.auth_info.client_key_data = Some(key);
                    rebuild_at = auth.expires_at().await;
                }
                Ok(exec::Credential::Token(_)) => {
                    credentials = Some(CredentialSource::Exec(auth));
                }
                Err(err) => return Err(ConnectError::from_auth(err)),
            }
            config.auth_info.exec = None;
        }
        _ => {}
    }

    let bearer = match (
        &credentials,
        &config.auth_info.token,
        &config.auth_info.token_file,
    ) {
        (Some(source), _, _) => Some(BearerToken::Managed(source.clone())),
        (None, Some(token), _) => Some(BearerToken::Static(token.clone())),
        (None, None, Some(file)) => Some(BearerToken::File(file.into())),
        _ => None,
    };

    let proxy = config
        .proxy_url
        .as_ref()
        .map(|u| redact_userinfo(&u.to_string()));
    let default_namespace = config.default_namespace.clone();

    let builder = kube::client::ClientBuilder::try_from(config)
        .map_err(|err| ConnectError::Config(error_chain(&err)))?;
    let client = match &credentials {
        // `AsyncFilter` clones its inner service per request; kube's boxed stack isn't `Clone`,
        // so a buffer (a channel to a worker task on the Tokio runtime) sits in between.
        Some(source) => builder
            .with_layer(&BufferLayer::new(BUFFER_SIZE))
            .with_layer(&AsyncFilterLayer::new(AuthLayer(source.clone())))
            .build(),
        None => builder.build(),
    };
    Ok(BuiltClient {
        client,
        credentials,
        bearer,
        default_namespace,
        proxy,
        rebuild_at,
    })
}

impl ConnectError {
    fn from_auth(err: AuthError) -> Self {
        match err {
            AuthError::SignInRequired => ConnectError::SignInRequired,
            AuthError::Exec { message, stderr } => ConnectError::Auth {
                message,
                detail: stderr,
            },
            AuthError::Failed(message) => ConnectError::Auth {
                message,
                detail: None,
            },
        }
    }
}

fn oidc_secrets(user: &AuthInfo) -> OidcSecrets {
    match (&user.auth_provider, &user.exec) {
        (Some(provider), _) => OidcSecrets::from_auth_provider(&provider.config),
        (None, Some(exec)) => OidcSecrets::from_kubelogin(exec),
        _ => OidcSecrets::default(),
    }
}

/// The OIDC credentials of an OIDC context, straight from its kubeconfig (for signing in before
/// a client exists).
pub fn oidc_auth(info: &ContextInfo, kubeconfig: &Kubeconfig) -> Option<OidcAuth> {
    let AuthMethod::Oidc(params) = &info.auth else {
        return None;
    };
    let user = kubeconfig
        .auth_infos
        .iter()
        .find(|u| Some(&u.name) == info.user.as_ref())?
        .auth_info
        .as_ref()?;
    Some(OidcAuth::new(params.clone(), oidc_secrets(user)))
}

/// Removes the credentials Kubyl provides itself, so kube doesn't run plugins or refresh OIDC.
fn strip_managed_auth(auth: &mut AuthInfo) {
    auth.exec = None;
    auth.auth_provider = None;
}

/// `HTTPS_PROXY`/`HTTP_PROXY` unless the host matches `NO_PROXY`.
fn env_proxy(cluster_url: &Uri) -> Option<Uri> {
    let var = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
    };
    let proxy = if cluster_url.scheme_str() == Some("http") {
        var(&["HTTP_PROXY", "http_proxy"])
    } else {
        var(&["HTTPS_PROXY", "https_proxy"])
    }?;
    let host = cluster_url.host()?;
    if no_proxy_matches(&var(&["NO_PROXY", "no_proxy"]).unwrap_or_default(), host) {
        return None;
    }
    // A bare `host:port` means HTTP.
    let proxy = if proxy.contains("://") {
        proxy
    } else {
        format!("http://{proxy}")
    };
    proxy.parse().ok()
}

/// `NO_PROXY` matching: `*`, exact hosts, domain suffixes (`.example.com` or `example.com`)
/// and IPv4 CIDRs.
fn no_proxy_matches(no_proxy: &str, host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    no_proxy
        .split(',')
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
        .any(|entry| {
            if entry == "*" || entry == host {
                return true;
            }
            if let Some((net, bits)) = entry.split_once('/')
                && let (Ok(net), Ok(bits), Ok(ip)) = (
                    net.parse::<std::net::Ipv4Addr>(),
                    bits.parse::<u32>(),
                    host.parse::<std::net::Ipv4Addr>(),
                )
                && bits <= 32
            {
                let mask = if bits == 0 {
                    0
                } else {
                    u32::MAX << (32 - bits)
                };
                return u32::from(net) & mask == u32::from(ip) & mask;
            }
            let suffix = entry.trim_start_matches('.');
            host.ends_with(&format!(".{suffix}"))
        })
}

fn redact_userinfo(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((_, host)) => format!("{scheme}://***@{host}"),
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

/// What a successful connection check found.
#[derive(Clone, Debug)]
pub struct Probe {
    pub latency: Duration,
    pub version: k8s_openapi::apimachinery::pkg::version::Info,
    /// The authenticated user (SelfSubjectReview, Kubernetes 1.28+).
    pub user: Option<String>,
}

/// `GET /version` for latency and version, then a SelfSubjectReview to check the credentials
/// (`/version` is readable anonymously on most clusters).
pub async fn probe(client: &Client) -> Result<Probe, ConnectError> {
    let started = Instant::now();
    let version = client
        .apiserver_version()
        .await
        .map_err(|err| ConnectError::from_kube(&err))?;
    let latency = started.elapsed();
    let reviews: Api<SelfSubjectReview> = Api::all(client.clone());
    let user = match reviews
        .create(&Default::default(), &SelfSubjectReview::default())
        .await
    {
        Ok(review) => review
            .status
            .and_then(|s| s.user_info)
            .and_then(|u| u.username),
        // Older servers don't serve SelfSubjectReview; `/version` has to do.
        Err(kube::Error::Api(status)) if status.code == 404 => None,
        Err(err) => return Err(ConnectError::from_kube(&err)),
    };
    Ok(Probe {
        latency,
        version,
        user,
    })
}

/// A health ping: `GET /version` round trip. Also exercises the credentials.
pub async fn ping(client: &Client) -> Result<Duration, ConnectError> {
    let started = Instant::now();
    client
        .apiserver_version()
        .await
        .map_err(|err| ConnectError::from_kube(&err))?;
    Ok(started.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_rules() {
        let rules = "localhost, .internal ,example.com,10.0.0.0/8";
        assert!(no_proxy_matches(rules, "localhost"));
        assert!(no_proxy_matches(rules, "k8s.platform.internal"));
        assert!(no_proxy_matches(rules, "api.example.com"));
        assert!(no_proxy_matches(rules, "example.com"));
        assert!(no_proxy_matches(rules, "10.2.3.4"));
        assert!(!no_proxy_matches(rules, "11.2.3.4"));
        assert!(!no_proxy_matches(rules, "notexample.com"));
        assert!(no_proxy_matches("*", "anything"));
        assert!(!no_proxy_matches("", "anything"));
    }

    #[test]
    fn redacts_proxy_credentials() {
        assert_eq!(
            redact_userinfo("http://u:p@proxy:3128"),
            "http://***@proxy:3128"
        );
        assert_eq!(redact_userinfo("http://proxy:3128"), "http://proxy:3128");
    }

    #[test]
    fn classifies_errors() {
        let status = |code| {
            kube::Error::Api(
                kube::core::Status::failure("nope", "Reason")
                    .with_code(code)
                    .boxed(),
            )
        };
        assert!(matches!(
            ConnectError::from_kube(&status(401)),
            ConnectError::Auth { .. }
        ));
        assert!(matches!(
            ConnectError::from_kube(&status(403)),
            ConnectError::Forbidden(_)
        ));
        assert!(matches!(
            ConnectError::from_kube(&status(500)),
            ConnectError::Unreachable(_)
        ));
        let auth = kube::Error::Service(Box::new(AuthError::SignInRequired));
        assert_eq!(ConnectError::from_kube(&auth), ConnectError::SignInRequired);
    }

    #[tokio::test]
    async fn builds_clients_for_static_and_oidc_contexts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        // A fake client certificate can't be loaded into rustls; use a token here.
        let dev = crate::kubeconfig::tests::DEV.replace(
            "client-certificate-data: Zm9v\n      client-key-data: Zm9v",
            "token: abc",
        );
        assert_ne!(dev, crate::kubeconfig::tests::DEV);
        std::fs::write(&file, dev).unwrap();
        std::fs::write(dir.path().join("other"), crate::kubeconfig::tests::OTHER).unwrap();
        let specs = crate::kubeconfig::source_specs(
            &crate::settings::KubeSettings {
                load_default_kubeconfig: false,
                kubeconfigs: vec![
                    file.display().to_string(),
                    dir.path().join("other").display().to_string(),
                ],
                ..Default::default()
            },
            None,
            None,
            &dir.path().join("pasted"),
        );
        let loaded = crate::kubeconfig::load(&specs);
        let dev = &loaded.contexts[0];
        let built = build(dev, loaded.configs[&dev.file].clone())
            .await
            .ok()
            .unwrap();
        assert!(built.credentials.is_none());
        assert_eq!(built.default_namespace, "payments");

        let oidc = loaded.contexts.iter().find(|c| c.auth.is_oidc()).unwrap();
        let built = build(oidc, loaded.configs[&oidc.file].clone())
            .await
            .ok()
            .unwrap();
        assert!(matches!(built.credentials, Some(CredentialSource::Oidc(_))));
    }
}
