//! OIDC sign-in and token refresh.
//!
//! Handles `auth-provider: oidc` users and kubelogin-style exec users
//! (`kubectl oidc-login get-token --oidc-issuer-url=… --oidc-client-id=…`) without running
//! kubelogin. The kube crate can refresh OIDC tokens but has no interactive sign-in, so both
//! live here:
//!
//! - **Browser sign-in**: authorization code + PKCE. A loopback server listens on
//!   `127.0.0.1:8000` (then `18000`, like kubelogin; `--listen-address` overrides), and the
//!   redirect URI is `http://localhost:<port>`, so IdP clients set up for kubelogin work as-is.
//! - **Device code** (RFC 8628) when the IdP supports it, for machines without a browser.
//! - **Refresh**: the ID token is sent to the API server; before it expires the refresh token
//!   gets a new one. Rotated refresh tokens replace the old ones.
//!
//! Refresh and ID tokens are kept in the OS keychain ([`super::store`]), keyed by issuer and
//! client id, so contexts sharing an IdP client share one sign-in. Tokens in the kubeconfig
//! (`id-token`, `refresh-token`) are used as a starting point but never written back.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use futures::channel::mpsc;
use jiff::Timestamp;
use kube::config::ExecConfig;
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthenticationFlow, CoreClaimName, CoreClaimType, CoreClient,
    CoreClientAuthMethod, CoreDeviceAuthorizationResponse, CoreGrantType, CoreJsonWebKey,
    CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
    CoreResponseType, CoreSubjectIdentifierType, CoreTokenResponse,
};
use openidconnect::{
    AdditionalProviderMetadata, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    DeviceAuthorizationUrl, IssuerUrl, Nonce, OAuth2TokenResponse as _, PkceCodeChallenge,
    ProviderMetadata, RedirectUrl, RefreshToken, Scope, TokenResponse as _, reqwest,
};
use parking_lot::Mutex;
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{AuthError, CaBundle, flag_value, flag_values, jwt_expiry, store};

/// Refresh ID tokens this long before they expire.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);
/// kubelogin's default loopback ports.
const DEFAULT_PORTS: [u16; 2] = [8000, 18000];
/// How long the browser sign-in waits for the redirect.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

/// The non-secret parts of an OIDC config, shown in the sign-in modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcParams {
    pub issuer: String,
    pub client_id: String,
    /// Scopes requested besides `openid`.
    pub extra_scopes: Vec<String>,
    /// CA for the IdP (`idp-certificate-authority`, kubelogin `--certificate-authority`).
    pub ca: CaBundle,
    /// Loopback ports to try, in order.
    pub listen_ports: Vec<u16>,
    /// kubelogin `--grant-type=device-code`: start with the device code.
    pub prefer_device_code: bool,
    /// The config came from a kubelogin exec user.
    pub kubelogin: bool,
}

impl OidcParams {
    /// From the `auth-provider: oidc` config map.
    pub fn from_auth_provider(config: &HashMap<String, String>) -> Self {
        Self {
            issuer: config.get("idp-issuer-url").cloned().unwrap_or_default(),
            client_id: config.get("client-id").cloned().unwrap_or_default(),
            extra_scopes: config
                .get("extra-scopes")
                .map(|s| {
                    s.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
            ca: CaBundle {
                file: config.get("idp-certificate-authority").map(Into::into),
                data: config.get("idp-certificate-authority-data").cloned(),
            },
            listen_ports: DEFAULT_PORTS.to_vec(),
            prefer_device_code: false,
            kubelogin: false,
        }
    }

    /// From a kubelogin exec config, or `None` if the exec config isn't kubelogin's OIDC mode.
    pub fn from_kubelogin(exec: &ExecConfig) -> Option<Self> {
        let command = exec.command.as_deref()?;
        let name = std::path::Path::new(command)
            .file_stem()?
            .to_string_lossy()
            .into_owned();
        let args = exec.args.clone().unwrap_or_default();
        let is_kubelogin = match name.as_str() {
            "kubelogin" | "kubectl-oidc_login" => args.first().is_some_and(|a| a == "get-token"),
            "kubectl" => args.first().is_some_and(|a| a == "oidc-login"),
            _ => false,
        };
        if !is_kubelogin {
            return None;
        }
        let issuer = flag_value(&args, "--oidc-issuer-url")?;
        let listen_ports: Vec<u16> = flag_values(&args, "--listen-address")
            .iter()
            .filter_map(|addr| addr.rsplit(':').next()?.parse().ok())
            .collect();
        Some(Self {
            issuer,
            client_id: flag_value(&args, "--oidc-client-id").unwrap_or_default(),
            extra_scopes: flag_values(&args, "--oidc-extra-scope")
                .iter()
                .flat_map(|s| s.split(','))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            ca: CaBundle {
                file: flag_value(&args, "--certificate-authority").map(Into::into),
                data: flag_value(&args, "--certificate-authority-data"),
            },
            listen_ports: if listen_ports.is_empty() {
                DEFAULT_PORTS.to_vec()
            } else {
                listen_ports
            },
            prefer_device_code: flag_value(&args, "--grant-type").as_deref() == Some("device-code"),
            kubelogin: true,
        })
    }

    /// `sso.example.com`.
    pub fn issuer_host(&self) -> String {
        url::Url::parse(&self.issuer)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_else(|| self.issuer.clone())
    }

    /// Every scope requested: `openid` plus the extra scopes.
    pub fn scopes(&self) -> Vec<String> {
        let mut scopes = vec!["openid".to_string()];
        for scope in &self.extra_scopes {
            if !scopes.contains(scope) {
                scopes.push(scope.clone());
            }
        }
        scopes
    }

    fn store_key(&self) -> String {
        format!(
            "oidc/{}/{}",
            self.issuer.trim_end_matches('/'),
            self.client_id
        )
    }
}

/// Secrets from the kubeconfig. Never logged.
#[derive(Clone, Default)]
pub struct OidcSecrets {
    pub client_secret: Option<SecretString>,
    pub id_token: Option<SecretString>,
    pub refresh_token: Option<SecretString>,
}

impl OidcSecrets {
    pub fn from_auth_provider(config: &HashMap<String, String>) -> Self {
        let secret = |key: &str| config.get(key).cloned().map(SecretString::from);
        Self {
            client_secret: secret("client-secret"),
            id_token: secret("id-token"),
            refresh_token: secret("refresh-token"),
        }
    }

    pub fn from_kubelogin(exec: &ExecConfig) -> Self {
        let args = exec.args.clone().unwrap_or_default();
        Self {
            client_secret: flag_value(&args, "--oidc-client-secret").map(SecretString::from),
            ..Default::default()
        }
    }
}

#[derive(Default)]
struct Tokens {
    loaded: bool,
    id_token: Option<SecretString>,
    expires_at: Option<Timestamp>,
    refresh_token: Option<SecretString>,
}

impl Tokens {
    fn valid_id_token(&self) -> Option<SecretString> {
        let expires_at = self.expires_at?;
        (Timestamp::now() + EXPIRY_MARGIN < expires_at)
            .then(|| self.id_token.clone())
            .flatten()
    }
}

type Slot = Arc<tokio::sync::Mutex<Tokens>>;

/// Tokens by store key, shared by contexts with the same issuer and client.
static SLOTS: LazyLock<Mutex<HashMap<String, Slot>>> = LazyLock::new(Default::default);

/// Progress of an interactive sign-in, for the modal.
#[derive(Clone, Debug)]
pub enum SignInEvent {
    /// The browser was opened on `url`; waiting for the redirect to `redirect_uri`.
    WaitingForBrowser { url: String, redirect_uri: String },
    /// Enter `user_code` at `verification_uri`.
    DeviceCode {
        user_code: String,
        verification_uri: String,
        verification_uri_complete: Option<String>,
    },
}

/// How to sign in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInMethod {
    Browser,
    DeviceCode,
}

/// Tokens for one issuer and client.
pub struct OidcAuth {
    pub params: OidcParams,
    secrets: OidcSecrets,
    slot: Slot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DeviceEndpointMetadata {
    device_authorization_endpoint: Option<DeviceAuthorizationUrl>,
}

impl AdditionalProviderMetadata for DeviceEndpointMetadata {}

type Metadata = ProviderMetadata<
    DeviceEndpointMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

fn fail(context: &str, err: impl std::fmt::Display) -> AuthError {
    AuthError::Failed(format!("{context}: {err}"))
}

/// Formats an error with its sources, like `anyhow`'s `{:#}`.
fn chain(err: &(dyn std::error::Error + 'static)) -> String {
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

impl OidcAuth {
    pub fn new(params: OidcParams, secrets: OidcSecrets) -> Self {
        let slot = SLOTS.lock().entry(params.store_key()).or_default().clone();
        Self {
            params,
            secrets,
            slot,
        }
    }

    /// A valid ID token, refreshing it when needed. [`AuthError::SignInRequired`] when the user
    /// has to sign in.
    pub async fn token(&self) -> Result<SecretString, AuthError> {
        let mut tokens = self.slot.lock().await;
        self.load(&mut tokens).await;
        if let Some(token) = tokens.valid_id_token() {
            return Ok(token);
        }
        let Some(refresh_token) = tokens.refresh_token.clone() else {
            return Err(AuthError::SignInRequired);
        };
        match self.refresh(&refresh_token).await {
            Ok(response) => {
                let expires_at = response.id_token().and_then(|t| jwt_expiry(&t.to_string()));
                self.accept(&mut tokens, response, expires_at).await?;
                tokens.valid_id_token().ok_or(AuthError::SignInRequired)
            }
            Err(err) => {
                tracing::info!(issuer = %self.params.issuer, "OIDC refresh failed: {err}");
                // The refresh token is expired or revoked; a new sign-in replaces it.
                tokens.refresh_token = None;
                Err(AuthError::SignInRequired)
            }
        }
    }

    pub async fn expires_at(&self) -> Option<Timestamp> {
        self.slot.lock().await.expires_at
    }

    /// Forgets the tokens (memory and keychain).
    pub async fn sign_out(&self) {
        let mut tokens = self.slot.lock().await;
        *tokens = Tokens {
            loaded: true,
            ..Default::default()
        };
        let key = self.params.store_key();
        tokio::task::spawn_blocking(move || {
            for suffix in ["refresh", "id"] {
                store::delete(&format!("{key}/{suffix}")).ok();
            }
        })
        .await
        .ok();
    }

    /// Reads tokens from the keychain (once), falling back to the kubeconfig's tokens.
    async fn load(&self, tokens: &mut Tokens) {
        if tokens.loaded {
            return;
        }
        tokens.loaded = true;
        let key = self.params.store_key();
        let stored = tokio::task::spawn_blocking(move || {
            (
                store::get(&format!("{key}/refresh")),
                store::get(&format!("{key}/id")),
            )
        })
        .await;
        let (refresh, id) = match stored {
            Ok((Ok(refresh), Ok(id))) => (refresh, id),
            Ok((Err(err), _)) | Ok((_, Err(err))) => {
                tracing::warn!(
                    "couldn't read OIDC tokens from the {}: {err}",
                    store::store_name()
                );
                (None, None)
            }
            Err(_) => (None, None),
        };
        tokens.refresh_token = refresh.or_else(|| self.secrets.refresh_token.clone());
        tokens.id_token = id.or_else(|| self.secrets.id_token.clone());
        tokens.expires_at = tokens
            .id_token
            .as_ref()
            .and_then(|t| jwt_expiry(t.expose_secret()));
    }

    fn http_client(&self) -> Result<reqwest::Client, AuthError> {
        let mut builder = reqwest::ClientBuilder::new()
            // Following redirects would allow SSRF (openidconnect's recommendation).
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30));
        let pem = self
            .params
            .ca
            .load()
            .map_err(|err| fail("couldn't read the IdP CA", err))?;
        if let Some(pem) = pem {
            let certs = reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|err| fail("invalid IdP CA", err))?;
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }
        builder
            .build()
            .map_err(|err| fail("couldn't create the HTTP client", err))
    }

    async fn metadata(&self, http: &reqwest::Client) -> Result<Metadata, AuthError> {
        let issuer = IssuerUrl::new(self.params.issuer.clone())
            .map_err(|err| fail("invalid issuer URL", err))?;
        Metadata::discover_async(issuer, http)
            .await
            .map_err(|err| fail("OIDC discovery failed", chain(&err)))
    }

    fn scopes(&self, metadata: &Metadata) -> Vec<Scope> {
        let mut scopes = self.params.scopes();
        // Dex only issues refresh tokens for `offline_access`; request it where it's supported.
        let supports_offline = metadata
            .scopes_supported()
            .is_some_and(|s| s.iter().any(|s| s.as_str() == "offline_access"));
        if supports_offline && !scopes.iter().any(|s| s == "offline_access") {
            scopes.push("offline_access".into());
        }
        scopes
            .into_iter()
            .filter(|s| s != "openid")
            .map(Scope::new)
            .collect()
    }

    fn client_id(&self) -> ClientId {
        ClientId::new(self.params.client_id.clone())
    }

    fn client_secret(&self) -> Option<ClientSecret> {
        self.secrets
            .client_secret
            .as_ref()
            .map(|s| ClientSecret::new(s.expose_secret().to_string()))
    }

    async fn refresh(&self, refresh_token: &SecretString) -> Result<CoreTokenResponse, AuthError> {
        let http = self.http_client()?;
        let metadata = self.metadata(&http).await?;
        let client =
            CoreClient::from_provider_metadata(metadata, self.client_id(), self.client_secret());
        let refresh_token = RefreshToken::new(refresh_token.expose_secret().to_string());
        client
            .exchange_refresh_token(&refresh_token)
            .map_err(|err| fail("the IdP has no token endpoint", err))?
            .request_async(&http)
            .await
            .map_err(|err| fail("token refresh failed", chain(&err)))
    }

    /// Stores a token response: keeps the (possibly rotated) refresh token and writes both
    /// tokens to the keychain. `expires_at` comes from the verified ID token claims.
    async fn accept(
        &self,
        tokens: &mut Tokens,
        response: CoreTokenResponse,
        expires_at: Option<Timestamp>,
    ) -> Result<(), AuthError> {
        let id_token = response
            .id_token()
            .ok_or_else(|| AuthError::Failed("the IdP returned no ID token".into()))?;
        tokens.id_token = Some(SecretString::from(id_token.to_string()));
        tokens.expires_at = expires_at;
        if let Some(refresh) = response.refresh_token() {
            tokens.refresh_token = Some(SecretString::from(refresh.secret().clone()));
        }

        let key = self.params.store_key();
        let id = tokens.id_token.clone();
        let refresh = tokens.refresh_token.clone();
        let result = tokio::task::spawn_blocking(move || {
            if let Some(refresh) = &refresh {
                store::set(&format!("{key}/refresh"), refresh)?;
            }
            // Windows Credential Manager limits entries to 2.5 KB; large ID tokens only live in
            // memory then, and the next start refreshes them.
            if let Some(id) = &id
                && let Err(err) = store::set(&format!("{key}/id"), id)
            {
                tracing::info!("ID token not stored: {err}");
            }
            Ok::<_, String>(())
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!(
                "couldn't store the refresh token in the {}: {err}",
                store::store_name()
            ),
            Err(_) => {}
        }
        Ok(())
    }

    /// Runs an interactive sign-in and stores the tokens. Progress goes to `events`.
    /// Dropping the future cancels the sign-in and closes the loopback server.
    pub async fn sign_in(
        &self,
        method: SignInMethod,
        events: mpsc::UnboundedSender<SignInEvent>,
    ) -> Result<(), AuthError> {
        let http = self.http_client()?;
        let metadata = self.metadata(&http).await?;
        let scopes = self.scopes(&metadata);
        let response = match method {
            SignInMethod::Browser => self.browser_flow(&http, metadata, scopes, &events).await?,
            SignInMethod::DeviceCode => self.device_flow(&http, metadata, scopes, &events).await?,
        };
        let (response, expires_at) = response;
        let mut tokens = self.slot.lock().await;
        tokens.loaded = true;
        self.accept(&mut tokens, response, expires_at).await
    }

    async fn browser_flow(
        &self,
        http: &reqwest::Client,
        metadata: Metadata,
        scopes: Vec<Scope>,
        events: &mpsc::UnboundedSender<SignInEvent>,
    ) -> Result<(CoreTokenResponse, Option<Timestamp>), AuthError> {
        let (listener, port) = bind_loopback(&self.params.listen_ports).await?;
        let redirect_uri = format!("http://localhost:{port}");
        let client =
            CoreClient::from_provider_metadata(metadata, self.client_id(), self.client_secret())
                .set_redirect_uri(
                    RedirectUrl::new(redirect_uri.clone())
                        .map_err(|err| fail("invalid redirect URI", err))?,
                );
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, csrf, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scopes(scopes)
            .set_pkce_challenge(challenge)
            .url();
        let url = url.to_string();
        events
            .unbounded_send(SignInEvent::WaitingForBrowser {
                url: url.clone(),
                redirect_uri,
            })
            .ok();
        if let Err(err) = open::that_detached(&url) {
            tracing::warn!("couldn't open the browser: {err}");
        }

        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&listener, csrf.secret()))
            .await
            .map_err(|_| AuthError::Failed("timed out waiting for the browser".into()))??;
        let response = client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|err| fail("the IdP has no token endpoint", err))?
            .set_pkce_verifier(verifier)
            .request_async(http)
            .await
            .map_err(|err| fail("code exchange failed", chain(&err)))?;
        let claims = response
            .id_token()
            .ok_or_else(|| AuthError::Failed("the IdP returned no ID token".into()))?
            .claims(&client.id_token_verifier(), &nonce)
            .map_err(|err| fail("invalid ID token", err))?;
        let expires_at = Timestamp::from_second(claims.expiration().timestamp()).ok();
        Ok((response, expires_at))
    }

    async fn device_flow(
        &self,
        http: &reqwest::Client,
        metadata: Metadata,
        scopes: Vec<Scope>,
        events: &mpsc::UnboundedSender<SignInEvent>,
    ) -> Result<(CoreTokenResponse, Option<Timestamp>), AuthError> {
        let endpoint = metadata
            .additional_metadata()
            .device_authorization_endpoint
            .clone()
            .ok_or_else(|| {
                AuthError::Failed("the identity provider doesn't support device codes".into())
            })?;
        let client =
            CoreClient::from_provider_metadata(metadata, self.client_id(), self.client_secret());
        let device_client = client.set_device_authorization_url(endpoint);
        let details: CoreDeviceAuthorizationResponse = device_client
            .exchange_device_code()
            .add_scopes(scopes)
            .request_async(http)
            .await
            .map_err(|err| fail("device authorization failed", chain(&err)))?;
        events
            .unbounded_send(SignInEvent::DeviceCode {
                user_code: details.user_code().secret().clone(),
                verification_uri: details.verification_uri().to_string(),
                verification_uri_complete: details
                    .verification_uri_complete()
                    .map(|u| u.secret().clone()),
            })
            .ok();
        let response = device_client
            .exchange_device_access_token(&details)
            .map_err(|err| fail("the IdP has no token endpoint", err))?
            .request_async(http, tokio::time::sleep, Some(SIGN_IN_TIMEOUT))
            .await
            .map_err(|err| fail("device sign-in failed", chain(&err)))?;
        // The device grant has no nonce.
        let claims = response
            .id_token()
            .ok_or_else(|| AuthError::Failed("the IdP returned no ID token".into()))?
            .claims(&device_client.id_token_verifier(), |_: Option<&Nonce>| {
                Ok(())
            })
            .map_err(|err| fail("invalid ID token", err))?;
        let expires_at = Timestamp::from_second(claims.expiration().timestamp()).ok();
        Ok((response, expires_at))
    }
}

async fn bind_loopback(ports: &[u16]) -> Result<(tokio::net::TcpListener, u16), AuthError> {
    let mut last_err = None;
    for &port in ports {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(err) => last_err = Some(err),
        }
    }
    Err(AuthError::Failed(format!(
        "no free loopback port ({}): {}",
        ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        last_err.map(|e| e.to_string()).unwrap_or_default()
    )))
}

const SUCCESS_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Kubyl</title>\
<body style=\"font:15px system-ui;background:#282c33;color:#dce0e5;display:grid;place-items:center;height:90vh\">\
<div><h2 style=\"font-weight:600\">Signed in to Kubyl</h2><p>You can close this tab and return to the app.</p></div>";

/// Serves the loopback redirect until a request carries `code` and the right `state`.
pub(super) async fn wait_for_code(
    listener: &tokio::net::TcpListener,
    state: &str,
) -> Result<String, AuthError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|err| fail("loopback server failed", err))?;
        let mut buf = vec![0u8; 8192];
        let mut len = 0;
        // Read the request head; the redirect is a small GET.
        while len < buf.len() {
            match stream.read(&mut buf[len..]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    len += n;
                    if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let head = String::from_utf8_lossy(&buf[..len]);
        let target = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        let query = callback_params(target);
        let respond = |status: &str, body: &str| {
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        if let Some(error) = query.get("error") {
            let description = query.get("error_description").cloned().unwrap_or_default();
            let page = format!("<p>Sign-in failed: {}</p>", html_escape(error));
            stream
                .write_all(respond("400 Bad Request", &page).as_bytes())
                .await
                .ok();
            return Err(AuthError::Failed(
                format!("sign-in failed: {error} {description}")
                    .trim()
                    .to_string(),
            ));
        }
        match (query.get("code"), query.get("state")) {
            (Some(code), Some(got)) if got == state => {
                stream
                    .write_all(respond("200 OK", SUCCESS_PAGE).as_bytes())
                    .await
                    .ok();
                return Ok(code.clone());
            }
            _ => {
                // Favicon requests, stale tabs from an earlier attempt…
                stream
                    .write_all(respond("404 Not Found", "").as_bytes())
                    .await
                    .ok();
            }
        }
    }
}

fn callback_params(target: &str) -> HashMap<String, String> {
    url::Url::parse(&format!("http://localhost{target}"))
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kubelogin_args() {
        let exec = ExecConfig {
            command: Some("kubectl".into()),
            args: Some(
                [
                    "oidc-login",
                    "get-token",
                    "--oidc-issuer-url=https://dex.local/dex",
                    "--oidc-client-id=kubyl",
                    "--oidc-client-secret=s3cret",
                    "--oidc-extra-scope=email",
                    "--oidc-extra-scope",
                    "groups,offline_access",
                    "--listen-address=127.0.0.1:9000",
                    "--grant-type=device-code",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ),
            ..Default::default()
        };
        let params = OidcParams::from_kubelogin(&exec).unwrap();
        assert_eq!(params.issuer, "https://dex.local/dex");
        assert_eq!(params.client_id, "kubyl");
        assert_eq!(
            params.scopes(),
            ["openid", "email", "groups", "offline_access"]
        );
        assert_eq!(params.listen_ports, [9000]);
        assert!(params.prefer_device_code && params.kubelogin);
        let secrets = OidcSecrets::from_kubelogin(&exec);
        assert_eq!(secrets.client_secret.unwrap().expose_secret(), "s3cret");
    }

    #[test]
    fn parses_auth_provider_config() {
        let config: HashMap<String, String> = [
            ("idp-issuer-url", "https://sso.example.com/realms/platform"),
            ("client-id", "kubernetes"),
            ("extra-scopes", "groups, email"),
            ("idp-certificate-authority-data", "Zm9v"),
            ("refresh-token", "r"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let params = OidcParams::from_auth_provider(&config);
        assert_eq!(params.scopes(), ["openid", "groups", "email"]);
        assert_eq!(params.listen_ports, DEFAULT_PORTS);
        assert_eq!(params.ca.load().unwrap().unwrap(), b"foo");
        assert!(
            OidcSecrets::from_auth_provider(&config)
                .refresh_token
                .is_some()
        );
    }

    #[test]
    fn parses_callback_queries() {
        let params = callback_params("/?code=abc&state=xyz");
        assert_eq!(params["code"], "abc");
        assert_eq!(params["state"], "xyz");
        assert!(callback_params("/favicon.ico").is_empty());
    }

    #[tokio::test]
    async fn loopback_server_ignores_wrong_state() {
        let (listener, port) = bind_loopback(&[0]).await.unwrap();
        let port = if port == 0 {
            listener.local_addr().unwrap().port()
        } else {
            port
        };
        let server = tokio::spawn(async move { wait_for_code(&listener, "good").await });
        for target in [
            "/favicon.ico",
            "/?code=bad&state=wrong",
            "/?code=c0de&state=good",
        ] {
            let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            stream
                .write_all(format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.ok();
        }
        assert_eq!(server.await.unwrap().ok().unwrap(), "c0de");
    }

    #[tokio::test]
    async fn without_tokens_sign_in_is_required() {
        // SAFETY: tests in this crate don't read the variable concurrently with this write.
        unsafe { std::env::set_var("KUBYL_CREDENTIAL_STORE", "memory") };
        let auth = OidcAuth::new(
            OidcParams::from_auth_provider(&HashMap::from([
                (
                    "idp-issuer-url".to_string(),
                    "https://unused.invalid".to_string(),
                ),
                ("client-id".to_string(), "none".to_string()),
            ])),
            OidcSecrets::default(),
        );
        assert!(matches!(auth.token().await, Err(AuthError::SignInRequired)));
    }
}
