//! Authentication.
//!
//! Static credentials (client certificates, bearer tokens, token files, basic auth) are handled
//! by kube itself. Exec plugins, OIDC and OpenShift OAuth are handled here, because Kubyl needs
//! more than kube offers for them: exec output is captured (stderr shows up in the UI),
//! credentials are cached until they expire, OIDC and OpenShift sign in through the browser, and
//! tokens live in the OS keychain. For those contexts the kube client gets no credentials from the kubeconfig;
//! [`AuthLayer`] adds the `Authorization` header to every request instead.
//!
//! Tokens are wrapped in [`SecretString`] everywhere so they can't end up in logs by accident.

pub mod exec;
pub mod oidc;
pub mod openshift;
pub mod shell_env;
pub mod store;

use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use http::{HeaderValue, Request, header::AUTHORIZATION};
use kube::config::{AuthInfo, ExecInteractiveMode};
use secrecy::{ExposeSecret as _, SecretString};
use tower::BoxError;
use tower::filter::AsyncPredicate;

pub use exec::ExecAuth;
pub use oidc::{OidcAuth, OidcParams, SignInEvent};
pub use openshift::OpenShiftAuth;

/// How a context authenticates, as detected from its kubeconfig user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    ClientCertificate,
    Token,
    TokenFile,
    Basic,
    Exec(ExecSummary),
    /// `auth-provider: oidc`, or a kubelogin (`kubectl oidc-login get-token`) exec config.
    Oidc(OidcParams),
    /// An OpenShift OAuth token from `oc login` (`sha256~…`). It expires and can't be refreshed;
    /// Kubyl signs in again through the cluster's OAuth server.
    OpenShift,
    /// A legacy auth provider other than OIDC (`gcp`, `azure`).
    Provider(String),
}

/// What the UI shows about an exec plugin. Only the command and its subcommands, never flag
/// values, which may hold secrets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecSummary {
    pub command: String,
    /// Leading arguments that aren't flags: `eks get-token`.
    pub subcommands: Vec<String>,
    /// `--profile` or `AWS_PROFILE`, shown as `(profile staging)`.
    pub profile: Option<String>,
    pub interactive: bool,
}

impl AuthMethod {
    /// Detects the method from a kubeconfig user.
    pub fn detect(user: &AuthInfo) -> Self {
        if let Some(provider) = &user.auth_provider {
            return match provider.name.as_str() {
                "oidc" => AuthMethod::Oidc(OidcParams::from_auth_provider(&provider.config)),
                name => AuthMethod::Provider(name.to_string()),
            };
        }
        if let Some(exec) = &user.exec {
            if let Some(params) = OidcParams::from_kubelogin(exec) {
                return AuthMethod::Oidc(params);
            }
            let args = exec.args.clone().unwrap_or_default();
            let command = exec
                .command
                .as_deref()
                .map(|c| {
                    std::path::Path::new(c)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| c.to_string())
                })
                .unwrap_or_default();
            let subcommands = args
                .iter()
                .take_while(|a| !a.starts_with('-'))
                .take(3)
                .cloned()
                .collect();
            let profile = flag_value(&args, "--profile").or_else(|| {
                exec.env.as_ref()?.iter().find_map(|env| {
                    (env.get("name")? == "AWS_PROFILE").then(|| env.get("value").cloned())?
                })
            });
            return AuthMethod::Exec(ExecSummary {
                command,
                subcommands,
                profile,
                interactive: exec.interactive_mode == Some(ExecInteractiveMode::Always),
            });
        }
        if user.client_certificate.is_some() || user.client_certificate_data.is_some() {
            return AuthMethod::ClientCertificate;
        }
        if let Some(token) = &user.token {
            if openshift::is_openshift_token(token.expose_secret()) {
                return AuthMethod::OpenShift;
            }
            return AuthMethod::Token;
        }
        if user.token_file.is_some() {
            return AuthMethod::TokenFile;
        }
        if user.username.is_some() && user.password.is_some() {
            return AuthMethod::Basic;
        }
        AuthMethod::None
    }

    pub fn is_oidc(&self) -> bool {
        matches!(self, AuthMethod::Oidc(_))
    }

    /// Whether Kubyl can sign in interactively when the credentials are missing or rejected.
    pub fn supports_sign_in(&self) -> bool {
        matches!(self, AuthMethod::Oidc(_) | AuthMethod::OpenShift)
    }

    /// Short description for tables: `exec · aws eks get-token`, `OIDC · sso.example.com`.
    pub fn label(&self) -> String {
        match self {
            AuthMethod::None => "none".into(),
            AuthMethod::ClientCertificate => "client certificate".into(),
            AuthMethod::Token => "bearer token".into(),
            AuthMethod::TokenFile => "token file".into(),
            AuthMethod::Basic => "basic auth".into(),
            AuthMethod::Exec(exec) => {
                let mut label = format!("exec · {}", exec.command);
                if let Some(profile) = &exec.profile {
                    label.push_str(&format!(" (profile {profile})"));
                } else if !exec.subcommands.is_empty() {
                    label.push(' ');
                    label.push_str(&exec.subcommands.join(" "));
                }
                label
            }
            AuthMethod::Oidc(params) => format!("OIDC · {}", params.issuer_host()),
            AuthMethod::OpenShift => "OpenShift OAuth".into(),
            AuthMethod::Provider(name) => format!("auth provider · {name}"),
        }
    }
}

/// The value of `--flag value` or `--flag=value`.
pub(crate) fn flag_value(args: &[String], flag: &str) -> Option<String> {
    flag_values(args, flag).into_iter().next()
}

/// Every value of a repeatable flag.
pub(crate) fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            if let Some(value) = iter.next() {
                values.push(value.clone());
            }
        } else if let Some(value) = arg
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            values.push(value.to_string());
        }
    }
    values
}

/// Why a request couldn't get credentials.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AuthError {
    /// The user has to sign in (OIDC without a valid refresh token, a rejected OpenShift token).
    #[error("sign-in required")]
    SignInRequired,
    /// An exec plugin failed. `stderr` is the plugin's error output (trimmed).
    #[error("{message}")]
    Exec {
        message: String,
        stderr: Option<String>,
    },
    #[error("{0}")]
    Failed(String),
}

impl AuthError {
    /// Finds an `AuthError` in the source chain of a kube error.
    pub fn find<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a AuthError> {
        let mut current = Some(err);
        while let Some(err) = current {
            if let Some(auth) = err.downcast_ref::<AuthError>() {
                return Some(auth);
            }
            current = err.source();
        }
        None
    }
}

/// The credentials Kubyl manages for a context.
#[derive(Clone)]
pub enum CredentialSource {
    Exec(Arc<ExecAuth>),
    Oidc(Arc<OidcAuth>),
    OpenShift(Arc<OpenShiftAuth>),
}

impl CredentialSource {
    /// A bearer token, running the exec plugin or refreshing the OIDC token when needed.
    pub async fn token(&self) -> Result<SecretString, AuthError> {
        match self {
            CredentialSource::Exec(exec) => match exec.credential().await? {
                exec::Credential::Token(token) => Ok(token),
                exec::Credential::ClientCertificate { .. } => Err(AuthError::Failed(
                    "the exec plugin returned a client certificate instead of a token".into(),
                )),
            },
            CredentialSource::Oidc(oidc) => oidc.token().await,
            CredentialSource::OpenShift(openshift) => openshift.token().await,
        }
    }

    /// When the current credential expires, if known.
    pub async fn expires_at(&self) -> Option<jiff::Timestamp> {
        match self {
            CredentialSource::Exec(exec) => exec.expires_at().await,
            CredentialSource::Oidc(oidc) => oidc.expires_at().await,
            CredentialSource::OpenShift(openshift) => openshift.expires_at(),
        }
    }
}

/// The user's bearer token for a cluster, for in-cluster services that authenticate the user
/// themselves (OpenShift's monitoring Routes). The API server strips credentials from requests
/// it proxies, so those services must be called directly, with the same token.
///
/// Never log it or store it anywhere.
#[derive(Clone)]
pub enum BearerToken {
    /// `token` in the kubeconfig (`oc login`).
    Static(SecretString),
    /// `tokenFile`, read on every use (it rotates).
    File(PathBuf),
    /// An exec plugin, OIDC or OpenShift OAuth, refreshed as needed.
    Managed(CredentialSource),
}

impl BearerToken {
    pub async fn get(&self) -> Result<SecretString, AuthError> {
        match self {
            BearerToken::Static(token) => Ok(token.clone()),
            BearerToken::File(path) => tokio::fs::read_to_string(path)
                .await
                .map(|token| SecretString::from(token.trim().to_string()))
                .map_err(|err| AuthError::Failed(format!("reading the token file: {err}"))),
            BearerToken::Managed(source) => source.token().await,
        }
    }
}

/// Adds `Authorization: Bearer …` from a [`CredentialSource`] to every request.
///
/// Used as `tower::filter::AsyncFilterLayer::new(AuthLayer(source))` on the kube client.
#[derive(Clone)]
pub struct AuthLayer(pub CredentialSource);

impl<B: Send + 'static> AsyncPredicate<Request<B>> for AuthLayer {
    type Future = BoxFuture<'static, Result<Request<B>, BoxError>>;
    type Request = Request<B>;

    fn check(&mut self, mut request: Request<B>) -> Self::Future {
        let source = self.0.clone();
        Box::pin(async move {
            let token = source.token().await.map_err(BoxError::from)?;
            let mut value = HeaderValue::try_from(format!("Bearer {}", token.expose_secret()))
                .map_err(|_| BoxError::from(AuthError::Failed("invalid token".into())))?;
            value.set_sensitive(true);
            request.headers_mut().insert(AUTHORIZATION, value);
            Ok(request)
        })
    }
}

/// Seconds until the `exp` claim of a JWT, without verifying it (the API server does that).
pub(crate) fn jwt_expiry(token: &str) -> Option<jiff::Timestamp> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    jiff::Timestamp::from_second(claims.get("exp")?.as_i64()?).ok()
}

/// A CA bundle: a file path from the kubeconfig or base64 PEM data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaBundle {
    pub file: Option<PathBuf>,
    /// Base64-encoded PEM.
    pub data: Option<String>,
}

impl CaBundle {
    pub fn is_empty(&self) -> bool {
        self.file.is_none() && self.data.is_none()
    }

    /// PEM bytes.
    pub fn load(&self) -> std::io::Result<Option<Vec<u8>>> {
        use base64::Engine as _;
        if let Some(data) = &self.data {
            return base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .map(Some)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err));
        }
        match &self.file {
            Some(file) => std::fs::read(file).map(Some),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::config::Kubeconfig;

    fn user(yaml: &str) -> AuthInfo {
        let config = Kubeconfig::from_yaml(&format!(
            "apiVersion: v1\nkind: Config\nusers:\n  - name: u\n    user:\n{}",
            yaml.lines()
                .map(|l| format!("      {l}\n"))
                .collect::<String>()
        ))
        .unwrap();
        config.auth_infos[0].auth_info.clone().unwrap()
    }

    #[test]
    fn detects_static_methods() {
        assert_eq!(AuthMethod::detect(&user("token: abc")), AuthMethod::Token);
        let openshift = AuthMethod::detect(&user("token: sha256~abc"));
        assert_eq!(openshift, AuthMethod::OpenShift);
        assert!(openshift.supports_sign_in());
        assert_eq!(
            AuthMethod::detect(&user("tokenFile: /t")),
            AuthMethod::TokenFile
        );
        assert_eq!(
            AuthMethod::detect(&user("username: a\npassword: b")),
            AuthMethod::Basic
        );
        assert_eq!(
            AuthMethod::detect(&user("client-certificate: /c\nclient-key: /k")),
            AuthMethod::ClientCertificate
        );
    }

    #[test]
    fn labels_exec_plugins_without_flag_values() {
        let aws = AuthMethod::detect(&user(
            "exec:\n  command: aws\n  args: [eks, get-token, --cluster-name, prod, --token, s3cret]",
        ));
        assert_eq!(aws.label(), "exec · aws eks get-token");
        let profile = AuthMethod::detect(&user(
            "exec:\n  command: aws\n  args: [eks, get-token]\n  env:\n    - {name: AWS_PROFILE, value: staging}",
        ));
        assert_eq!(profile.label(), "exec · aws (profile staging)");
        let gke = AuthMethod::detect(&user(
            "exec:\n  command: /usr/local/bin/gke-gcloud-auth-plugin\n  interactiveMode: Always",
        ));
        assert_eq!(gke.label(), "exec · gke-gcloud-auth-plugin");
        assert!(matches!(
            gke,
            AuthMethod::Exec(ExecSummary {
                interactive: true,
                ..
            })
        ));
    }

    #[test]
    fn detects_oidc_provider_and_kubelogin() {
        let provider = AuthMethod::detect(&user(
            "auth-provider:\n  name: oidc\n  config:\n    idp-issuer-url: https://sso.example.com/realms/p\n    client-id: kubernetes",
        ));
        assert_eq!(provider.label(), "OIDC · sso.example.com");
        let kubelogin = AuthMethod::detect(&user(
            "exec:\n  command: kubectl\n  args: [oidc-login, get-token, --oidc-issuer-url=https://dex.local:5556/dex, --oidc-client-id=kubyl]",
        ));
        assert!(kubelogin.is_oidc());
        assert_eq!(kubelogin.label(), "OIDC · dex.local");
        // Azure's kubelogin is a plain exec plugin.
        let azure = AuthMethod::detect(&user(
            "exec:\n  command: kubelogin\n  args: [get-token, --login, azurecli, --server-id, x]",
        ));
        assert_eq!(azure.label(), "exec · kubelogin get-token");
    }

    #[test]
    fn reads_flag_values() {
        let args: Vec<String> = ["a", "--x", "1", "--x=2", "--y"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(flag_values(&args, "--x"), ["1", "2"]);
        assert_eq!(flag_value(&args, "--y"), None);
    }

    #[test]
    fn decodes_jwt_expiry() {
        use base64::Engine as _;
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"exp":1900000000}"#);
        let token = format!("h.{payload}.s");
        assert_eq!(jwt_expiry(&token).unwrap().as_second(), 1_900_000_000);
        assert!(jwt_expiry("not-a-jwt").is_none());
    }

    #[test]
    fn finds_auth_errors_in_error_chains() {
        let err: BoxError = Box::new(AuthError::SignInRequired);
        let kube_err = kube::Error::Service(err);
        assert!(matches!(
            AuthError::find(&kube_err),
            Some(AuthError::SignInRequired)
        ));
    }
}
