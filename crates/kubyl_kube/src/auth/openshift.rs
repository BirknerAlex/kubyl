//! OpenShift's built-in OAuth server, the way `oc login` uses it.
//!
//! `oc login` writes an OAuth access token (`sha256~…`) into the kubeconfig. It expires (a day by
//! default) and can't be refreshed, and OpenShift's OAuth server isn't the OIDC flow kubectl
//! plugins use, so these contexts sign in here:
//!
//! - **Browser**, like `oc login --web`: authorization code + PKCE with the `openshift-cli-client`
//!   and a loopback redirect (`http://127.0.0.1:<port>/callback`). Works with every identity
//!   provider the cluster has.
//! - **Username and password**, like `oc login -u`: the `openshift-challenging-client` with
//!   basic auth (kube:admin, htpasswd, LDAP).
//! - **Token**: paste the token from the console's "Copy login command" page
//!   (`<oauth>/oauth/token/request`), for any identity provider and any OpenShift version.
//!
//! The endpoints come from `/.well-known/oauth-authorization-server` on the API server. Tokens
//! are kept in the OS keychain per API server and kubeconfig user; the kubeconfig's token is only
//! a starting point and is never written back.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine as _;
use futures::channel::mpsc;
use jiff::Timestamp;
use openidconnect::{CsrfToken, PkceCodeChallenge, reqwest};
use parking_lot::Mutex;
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;

use super::{AuthError, SignInEvent, store};

/// The OAuth client `oc login --web` uses (loopback redirects allowed).
const CLI_CLIENT: &str = "openshift-cli-client";
/// The OAuth client `oc login -u` uses (basic-auth challenges).
const CHALLENGING_CLIENT: &str = "openshift-challenging-client";
/// How long the browser sign-in waits for the redirect.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

/// Whether a kubeconfig token is an OpenShift OAuth access token.
pub fn is_openshift_token(token: &str) -> bool {
    token.starts_with("sha256~")
}

/// The login name of an `oc login` kubeconfig user (`developer/api-lab:6443` → `developer`).
/// The cluster admin shows up as `kube:admin` but logs in as `kubeadmin`.
pub fn username_hint(kubeconfig_user: &str) -> String {
    let user = kubeconfig_user
        .split_once('/')
        .map(|(user, _)| user)
        .unwrap_or(kubeconfig_user);
    match user {
        "kube:admin" => "kubeadmin".into(),
        user => user.into(),
    }
}

/// Where and how to reach the cluster (from the built kube config).
#[derive(Clone, Debug)]
pub struct OpenShiftParams {
    /// The API server URL.
    pub server: String,
    /// The kubeconfig user, for the keychain key and the username hint.
    pub user: String,
    /// CA certificates of the API server (DER), added to the OS roots.
    pub roots: Vec<Vec<u8>>,
    /// `insecure-skip-tls-verify` in the kubeconfig: the same trust applies to its OAuth server.
    pub insecure: bool,
    pub proxy: Option<String>,
}

impl OpenShiftParams {
    fn store_key(&self) -> String {
        store_key(&self.server, &self.user)
    }
}

/// Keychain and registry key: `openshift/<server>/<kubeconfig user>`.
fn store_key(server: &str, user: &str) -> String {
    format!("openshift/{}/{user}", server.trim_end_matches('/'))
}

/// The OAuth server's endpoints.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Metadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
}

impl Metadata {
    /// The page that shows a token after signing in (the console's "Copy login command").
    pub fn token_request_url(&self) -> String {
        format!("{}/oauth/token/request", self.issuer.trim_end_matches('/'))
    }
}

/// Which token the last request used, so a 401 forgets the right one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Used {
    Stored,
    Kubeconfig,
}

#[derive(Default)]
struct Tokens {
    /// From a sign-in in Kubyl (keychain).
    stored: Option<SecretString>,
    expires_at: Option<Timestamp>,
    /// From the kubeconfig (`oc login`); not used anymore once the API server rejected it.
    kubeconfig: Option<SecretString>,
    kubeconfig_rejected: bool,
    used: Option<Used>,
}

impl Tokens {
    fn usable(&self) -> bool {
        self.stored.is_some() || (self.kubeconfig.is_some() && !self.kubeconfig_rejected)
    }
}

/// Tokens of one kubeconfig user on one OpenShift cluster, shared by every context and client
/// for them.
pub struct OpenShiftAuth {
    pub params: OpenShiftParams,
    tokens: Mutex<Tokens>,
    /// The keychain is read once, on first use.
    loaded: tokio::sync::OnceCell<()>,
}

static SHARED: LazyLock<Mutex<HashMap<String, Arc<OpenShiftAuth>>>> =
    LazyLock::new(Default::default);

impl OpenShiftAuth {
    /// The auth for these params, shared by key. A changed kubeconfig token (a new `oc login`)
    /// replaces the old one and gets another chance.
    pub fn shared(params: OpenShiftParams, kubeconfig_token: Option<SecretString>) -> Arc<Self> {
        let auth = SHARED
            .lock()
            .entry(params.store_key())
            .or_insert_with(|| {
                Arc::new(Self {
                    params,
                    tokens: Mutex::new(Tokens::default()),
                    loaded: tokio::sync::OnceCell::new(),
                })
            })
            .clone();
        if let Some(token) = kubeconfig_token {
            let mut tokens = auth.tokens.lock();
            let changed = tokens
                .kubeconfig
                .as_ref()
                .is_none_or(|old| old.expose_secret() != token.expose_secret());
            if changed {
                tokens.kubeconfig = Some(token);
                tokens.kubeconfig_rejected = false;
            }
        }
        auth
    }

    /// The auth of a kubeconfig user on a server, once a client was built for it.
    pub fn find(server: &str, user: &str) -> Option<Arc<Self>> {
        SHARED.lock().get(&store_key(server, user)).cloned()
    }

    /// A token to send: the one from Kubyl's last sign-in, else the kubeconfig's.
    /// [`AuthError::SignInRequired`] when neither is usable.
    pub async fn token(&self) -> Result<SecretString, AuthError> {
        self.loaded.get_or_init(|| self.load()).await;
        let mut tokens = self.tokens.lock();
        let fresh = tokens
            .expires_at
            .is_none_or(|at| at > Timestamp::now() + Duration::from_secs(30));
        if let Some(token) = tokens.stored.clone().filter(|_| fresh) {
            tokens.used = Some(Used::Stored);
            return Ok(token);
        }
        if let Some(token) = tokens
            .kubeconfig
            .clone()
            .filter(|_| !tokens.kubeconfig_rejected)
        {
            tokens.used = Some(Used::Kubeconfig);
            return Ok(token);
        }
        Err(AuthError::SignInRequired)
    }

    /// When the token in use expires, if known (only tokens from a sign-in in Kubyl).
    pub fn expires_at(&self) -> Option<Timestamp> {
        let tokens = self.tokens.lock();
        (tokens.used == Some(Used::Stored))
            .then_some(tokens.expires_at)
            .flatten()
    }

    /// The API server rejected the last token: stop using it. Returns whether another token is
    /// left to try.
    pub async fn invalidate(&self) -> bool {
        let (forget, usable) = {
            let mut tokens = self.tokens.lock();
            let used = tokens.used.take();
            match used {
                Some(Used::Stored) => {
                    tokens.stored = None;
                    tokens.expires_at = None;
                }
                Some(Used::Kubeconfig) => tokens.kubeconfig_rejected = true,
                None => {}
            }
            (used == Some(Used::Stored), tokens.usable())
        };
        if forget {
            let key = self.params.store_key();
            tokio::task::spawn_blocking(move || {
                for suffix in ["token", "expires"] {
                    store::delete(&format!("{key}/{suffix}")).ok();
                }
            })
            .await
            .ok();
        }
        usable
    }

    async fn load(&self) {
        let key = self.params.store_key();
        let stored = tokio::task::spawn_blocking(move || {
            (
                store::get(&format!("{key}/token")),
                store::get(&format!("{key}/expires")),
            )
        })
        .await;
        match stored {
            Ok((Ok(Some(token)), expires)) => {
                let mut tokens = self.tokens.lock();
                // A sign-in that finished in the meantime wins.
                if tokens.stored.is_none() {
                    tokens.stored = Some(token);
                    tokens.expires_at = expires
                        .ok()
                        .flatten()
                        .and_then(|e| e.expose_secret().parse::<i64>().ok())
                        .and_then(|s| Timestamp::from_second(s).ok());
                }
            }
            Ok((Err(err), _)) => tracing::warn!(
                "couldn't read the OpenShift token from the {}: {err}",
                store::store_name()
            ),
            _ => {}
        }
    }

    /// Checks a new token against the API server, then keeps it. Returns the user it belongs to.
    async fn accept(
        &self,
        token: SecretString,
        expires_in: Option<u64>,
    ) -> Result<Option<String>, AuthError> {
        let user = self.verify(&token).await?;
        self.keep(token, expires_in).await;
        Ok(user)
    }

    /// Who the token belongs to (`GET /apis/user.openshift.io/v1/users/~`). A rejected token is
    /// an error; an unreachable API server isn't (the next connect shows that).
    async fn verify(&self, token: &SecretString) -> Result<Option<String>, AuthError> {
        let url = format!(
            "{}/apis/user.openshift.io/v1/users/~",
            self.params.server.trim_end_matches('/')
        );
        let mut header =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|_| AuthError::Failed("that doesn't look like a token".into()))?;
        header.set_sensitive(true);
        let response = match self
            .http_client()?
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, header)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!("couldn't check the new OpenShift token: {}", chain(&err));
                return Ok(None);
            }
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AuthError::Failed(
                "the API server rejected the token (401 Unauthorized)".into(),
            ));
        }
        let body: serde_json::Value = match response.bytes().await {
            Ok(body) => serde_json::from_slice(&body).unwrap_or_default(),
            Err(_) => serde_json::Value::Null,
        };
        Ok(body
            .pointer("/metadata/name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }

    /// Keeps a token (memory and keychain).
    async fn keep(&self, token: SecretString, expires_in: Option<u64>) {
        let expires_at = expires_in.and_then(|s| {
            Timestamp::now()
                .checked_add(jiff::SignedDuration::from_secs(s as i64))
                .ok()
        });
        {
            let mut tokens = self.tokens.lock();
            tokens.stored = Some(token.clone());
            tokens.expires_at = expires_at;
            tokens.used = None;
        }
        let key = self.params.store_key();
        let result = tokio::task::spawn_blocking(move || {
            store::set(&format!("{key}/token"), &token)?;
            match expires_at {
                Some(at) => store::set(
                    &format!("{key}/expires"),
                    &SecretString::from(at.as_second().to_string()),
                ),
                None => store::delete(&format!("{key}/expires")),
            }
        })
        .await;
        if let Ok(Err(err)) = result {
            tracing::warn!(
                "couldn't store the OpenShift token in the {}: {err}",
                store::store_name()
            );
        }
    }

    fn http_client(&self) -> Result<reqwest::Client, AuthError> {
        let mut builder = reqwest::ClientBuilder::new()
            // The password flow reads the redirect itself; never follow redirects blindly.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .danger_accept_invalid_certs(self.params.insecure);
        // The OAuth server has the ingress certificate: often signed by a CA only the OS (or the
        // kubeconfig) trusts.
        for der in native_roots().iter().chain(&self.params.roots) {
            if let Ok(cert) = reqwest::Certificate::from_der(der) {
                builder = builder.add_root_certificate(cert);
            }
        }
        if let Some(proxy) = &self.params.proxy
            && let Ok(proxy) = reqwest::Proxy::all(proxy)
        {
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .map_err(|err| AuthError::Failed(format!("couldn't create the HTTP client: {err}")))
    }

    /// The OAuth server's endpoints, from the API server.
    pub async fn metadata(&self) -> Result<Metadata, AuthError> {
        let url = format!(
            "{}/.well-known/oauth-authorization-server",
            self.params.server.trim_end_matches('/')
        );
        let response = self
            .http_client()?
            .get(&url)
            .send()
            .await
            .map_err(|err| AuthError::Failed(format!("reaching {url}: {}", chain(&err))))?;
        if !response.status().is_success() {
            return Err(AuthError::Failed(format!(
                "the cluster has no OAuth server ({} from {url})",
                response.status()
            )));
        }
        let body = response
            .bytes()
            .await
            .map_err(|err| AuthError::Failed(format!("reading {url}: {}", chain(&err))))?;
        serde_json::from_slice(&body)
            .map_err(|err| AuthError::Failed(format!("invalid OAuth metadata: {err}")))
    }

    /// Whether the cluster has the OAuth client for browser sign-in (`openshift-cli-client`,
    /// newer OpenShift versions). Older clusters answer `unauthorized_client`.
    pub async fn browser_supported(&self, metadata: &Metadata) -> Result<bool, AuthError> {
        let (challenge, _) = PkceCodeChallenge::new_random_sha256();
        let url = authorize_url(
            metadata,
            "http://127.0.0.1/callback",
            challenge.as_str(),
            "probe",
        );
        let response = self
            .http_client()?
            .get(&url)
            .send()
            .await
            .map_err(|err| oauth_unreachable("reaching the OAuth server", &err))?;
        if response.status() != reqwest::StatusCode::BAD_REQUEST {
            return Ok(true);
        }
        let body: serde_json::Value = match response.bytes().await {
            Ok(body) => serde_json::from_slice(&body).unwrap_or_default(),
            Err(_) => serde_json::Value::Null,
        };
        Ok(body["error"].as_str() != Some("unauthorized_client"))
    }

    /// Browser sign-in (`oc login --web`). Dropping the future closes the loopback port.
    /// Returns the user name.
    pub async fn sign_in_browser(
        &self,
        events: mpsc::UnboundedSender<SignInEvent>,
    ) -> Result<Option<String>, AuthError> {
        let metadata = self.metadata().await?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|err| AuthError::Failed(format!("no loopback port: {err}")))?;
        let port = listener
            .local_addr()
            .map_err(|err| AuthError::Failed(err.to_string()))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let state = CsrfToken::new_random();
        let url = authorize_url(&metadata, &redirect_uri, challenge.as_str(), state.secret());
        events
            .unbounded_send(SignInEvent::WaitingForBrowser {
                url: url.clone(),
                redirect_uri: redirect_uri.clone(),
            })
            .ok();
        if let Err(err) = open::that_detached(&url) {
            tracing::warn!("couldn't open the browser: {err}");
        }
        let code = tokio::time::timeout(
            SIGN_IN_TIMEOUT,
            super::oidc::wait_for_code(&listener, state.secret()),
        )
        .await
        .map_err(|_| AuthError::Failed("timed out waiting for the browser".into()))??;
        let response = self
            .http_client()?
            .post(&metadata.token_endpoint)
            // A public client: its id with an empty secret.
            .basic_auth(CLI_CLIENT, Some(""))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("client_id", CLI_CLIENT),
                ("code_verifier", verifier.secret().as_str()),
            ])
            .send()
            .await
            .map_err(|err| oauth_unreachable("token exchange", &err))?;
        let status = response.status();
        let body: serde_json::Value = match response.bytes().await {
            Ok(body) => serde_json::from_slice(&body).unwrap_or_default(),
            Err(_) => serde_json::Value::Null,
        };
        let token = body["access_token"]
            .as_str()
            .filter(|_| status.is_success());
        let Some(token) = token else {
            return Err(AuthError::Failed(format!(
                "token exchange failed ({status}): {}",
                body["error_description"]
                    .as_str()
                    .or(body["error"].as_str())
                    .unwrap_or("no token")
            )));
        };
        self.accept(
            SecretString::from(token.to_string()),
            body["expires_in"].as_u64(),
        )
        .await
    }

    /// Username and password (`oc login -u`), for identity providers that take them.
    /// Returns the user name.
    pub async fn sign_in_password(
        &self,
        username: &str,
        password: &SecretString,
    ) -> Result<Option<String>, AuthError> {
        let metadata = self.metadata().await?;
        let url = format!(
            "{}?response_type=token&client_id={CHALLENGING_CLIENT}",
            metadata.authorization_endpoint
        );
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{username}:{}", password.expose_secret()));
        let mut header = reqwest::header::HeaderValue::from_str(&format!("Basic {basic}"))
            .map_err(|_| AuthError::Failed("invalid username or password characters".into()))?;
        header.set_sensitive(true);
        let response = self
            .http_client()?
            .get(&url)
            .header(reqwest::header::AUTHORIZATION, header)
            // The challenging client insists on this CSRF guard.
            .header("X-CSRF-Token", "1")
            .send()
            .await
            .map_err(|err| oauth_unreachable("reaching the OAuth server", &err))?;
        match response.status().as_u16() {
            401 => return Err(AuthError::Failed("wrong username or password".into())),
            302 => {}
            status => {
                return Err(AuthError::Failed(format!(
                    "the OAuth server answered {status}; this identity provider may not accept passwords, try the browser"
                )));
            }
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        let (token, expires_in) = implicit_token(location).map_err(AuthError::Failed)?;
        self.accept(token, expires_in).await
    }

    /// A token pasted by the user, or the whole `oc login --token=…` command. Returns the user
    /// name.
    pub async fn use_token(&self, pasted: &str) -> Result<Option<String>, AuthError> {
        let token = token_from_paste(pasted)
            .ok_or_else(|| AuthError::Failed("that doesn't look like a token".into()))?;
        self.accept(SecretString::from(token), None).await
    }
}

/// CA certificates of the OS trust store (DER), read once.
fn native_roots() -> &'static [Vec<u8>] {
    static ROOTS: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
        let result = rustls_native_certs::load_native_certs();
        if let Some(err) = result.errors.first() {
            tracing::debug!("reading the OS trust store: {err}");
        }
        result.certs.into_iter().map(|c| c.to_vec()).collect()
    });
    &ROOTS
}

/// The authorization URL of the browser flow.
fn authorize_url(metadata: &Metadata, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", CLI_CLIENT)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .finish();
    format!("{}?{query}", metadata.authorization_endpoint)
}

/// The token in an implicit-grant redirect (`…/oauth/token/implicit#access_token=…&expires_in=…`),
/// or the error it carries instead.
fn implicit_token(location: &str) -> Result<(SecretString, Option<u64>), String> {
    let params = |part: &str| -> HashMap<String, String> {
        url::form_urlencoded::parse(part.as_bytes())
            .into_owned()
            .collect()
    };
    let (rest, fragment) = location.split_once('#').unwrap_or((location, ""));
    let mut pairs = params(fragment);
    if let Some((_, query)) = rest.split_once('?') {
        for (key, value) in params(query) {
            pairs.entry(key).or_insert(value);
        }
    }
    if let Some(token) = pairs.get("access_token").filter(|t| !t.is_empty()) {
        return Ok((
            SecretString::from(token.clone()),
            pairs.get("expires_in").and_then(|e| e.parse().ok()),
        ));
    }
    Err(match (pairs.get("error"), pairs.get("error_description")) {
        (_, Some(description)) => format!("sign-in failed: {description}"),
        (Some(error), None) => format!("sign-in failed: {error}"),
        (None, None) => "the OAuth server's answer had no token".into(),
    })
}

/// A token from what the user pasted: the token, `--token=…`, or a whole `oc login` command.
fn token_from_paste(pasted: &str) -> Option<String> {
    let pasted = pasted.trim();
    let token = pasted
        .split_whitespace()
        .find_map(|word| word.strip_prefix("--token="))
        .unwrap_or(pasted)
        .trim_matches(|c| c == '"' || c == '\'');
    (!token.is_empty() && !token.contains(char::is_whitespace)).then(|| token.to_string())
}

/// A request to the OAuth server (on the cluster's ingress) failed. An untrusted certificate gets
/// a hint: the token method only talks to the API server.
fn oauth_unreachable(what: &str, err: &reqwest::Error) -> AuthError {
    let message = chain(err);
    let hint = if message.contains("certificate") {
        " (its certificate isn't trusted by this machine or the kubeconfig; use a token instead)"
    } else {
        ""
    };
    AuthError::Failed(format!("{what}: {message}{hint}"))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn params(user: &str) -> OpenShiftParams {
        OpenShiftParams {
            server: "https://api.lab.example:6443".into(),
            user: user.into(),
            roots: Vec::new(),
            insecure: false,
            proxy: None,
        }
    }

    #[test]
    fn recognizes_oc_login_contexts() {
        assert!(is_openshift_token("sha256~abc"));
        assert!(!is_openshift_token("eyJhbGciOi"));
        assert_eq!(
            username_hint("kube:admin/api-lab-example:6443"),
            "kubeadmin"
        );
        assert_eq!(username_hint("developer/api-lab-example:6443"), "developer");
        assert_eq!(username_hint("developer"), "developer");
    }

    #[test]
    fn parses_redirects_and_pastes() {
        let (token, expires) = implicit_token(
            "https://oauth-openshift.apps.lab/oauth/token/implicit#access_token=sha256~xyz&expires_in=86400&scope=user%3Afull&token_type=Bearer",
        )
        .unwrap();
        assert_eq!(token.expose_secret(), "sha256~xyz");
        assert_eq!(expires, Some(86400));
        assert_eq!(
            implicit_token("https://oauth/oauth/token/implicit?error=access_denied").err(),
            Some("sign-in failed: access_denied".into())
        );
        assert!(implicit_token("https://oauth/oauth/token/implicit").is_err());

        assert_eq!(
            token_from_paste(" sha256~abc \n").as_deref(),
            Some("sha256~abc")
        );
        assert_eq!(
            token_from_paste("oc login --token=sha256~abc --server=https://api.lab:6443")
                .as_deref(),
            Some("sha256~abc")
        );
        assert_eq!(token_from_paste("two words"), None);
        assert_eq!(token_from_paste(""), None);
    }

    #[test]
    fn builds_the_authorize_url() {
        let metadata = Metadata {
            issuer: "https://oauth-openshift.apps.lab".into(),
            authorization_endpoint: "https://oauth-openshift.apps.lab/oauth/authorize".into(),
            token_endpoint: "https://oauth-openshift.apps.lab/oauth/token".into(),
        };
        let url = authorize_url(&metadata, "http://127.0.0.1:4321/callback", "CH", "ST");
        assert_eq!(
            url,
            "https://oauth-openshift.apps.lab/oauth/authorize?client_id=openshift-cli-client&response_type=code&redirect_uri=http%3A%2F%2F127.0.0.1%3A4321%2Fcallback&code_challenge=CH&code_challenge_method=S256&state=ST"
        );
        assert_eq!(
            metadata.token_request_url(),
            "https://oauth-openshift.apps.lab/oauth/token/request"
        );
    }

    #[tokio::test]
    async fn tokens_fall_back_and_get_invalidated() {
        // SAFETY: tests in this crate use the in-memory store.
        unsafe { std::env::set_var("KUBYL_CREDENTIAL_STORE", "memory") };
        let auth = OpenShiftAuth::shared(
            params("tester/api-lab:6443"),
            Some(SecretString::from("sha256~old".to_string())),
        );
        assert_eq!(auth.token().await.unwrap().expose_secret(), "sha256~old");
        // The API server rejects it: nothing left, a sign-in is needed.
        assert!(!auth.invalidate().await);
        assert!(matches!(auth.token().await, Err(AuthError::SignInRequired)));
        // A token from a sign-in is used next.
        auth.keep(SecretString::from("sha256~new".to_string()), Some(86400))
            .await;
        assert!(auth.expires_at().is_none(), "not used yet");
        assert_eq!(auth.token().await.unwrap().expose_secret(), "sha256~new");
        assert!(auth.expires_at().is_some());
        // A new `oc login` writes another kubeconfig token: it gets a chance again.
        let again = OpenShiftAuth::shared(
            params("tester/api-lab:6443"),
            Some(SecretString::from("sha256~fresh".to_string())),
        );
        assert!(Arc::ptr_eq(&auth, &again));
        assert!(again.invalidate().await, "the kubeconfig token is left");
        assert_eq!(again.token().await.unwrap().expose_secret(), "sha256~fresh");
    }
}
