//! SSO sign-in to Argo CD, like `argocd login --sso`: the browser signs in at Argo CD's Dex
//! (`<url>/api/dex`, client `argo-cd-cli`) or its external OIDC provider (`cliClientID`), with
//! PKCE and the redirect `http://localhost:8085/auth/callback` that Argo CD's Dex allows and
//! that admins register for the CLI. The ID token is the Argo CD session token; the refresh
//! token renews it.
//!
//! Where to sign in comes from `/api/v1/settings` of the install the user confirmed. Only the
//! authorization code (with its PKCE verifier) and the refresh token go to the issuer; the
//! issuer's certificate is verified.

use std::time::Duration;

use kubyl_kube::auth::oidc::{bind_loopback, wait_for_code};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse as _,
    PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, TokenResponse as _, reqwest,
};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value;

/// The CLI's default `--sso-port`.
pub const CALLBACK_PORT: u16 = 8085;
pub const REDIRECT_URI: &str = "http://localhost:8085/auth/callback";
/// The Dex client Argo CD registers for its CLI.
const DEX_CLI_CLIENT: &str = "argo-cd-cli";
const TIMEOUT: Duration = Duration::from_secs(300);

/// Where and how to sign in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsoConfig {
    pub issuer: String,
    pub client_id: String,
    /// Without `openid` (always requested).
    pub scopes: Vec<String>,
    /// "Dex (GitHub)", "Okta"…
    pub provider: String,
}

impl SsoConfig {
    /// What `argocd login --sso` uses, from Argo CD's `/api/v1/settings`.
    pub fn from_settings(settings: &Value) -> Result<Self, String> {
        let strings = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let text = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(String::from);
        let oidc = &settings["oidcConfig"];
        if let Some(issuer) = text(&oidc["issuer"]) {
            let client_id = text(&oidc["cliClientID"])
                .or_else(|| text(&oidc["clientID"]))
                .ok_or("Argo CD's OIDC config has no client ID.")?;
            let mut scopes = strings(&oidc["scopes"]);
            if scopes.is_empty() {
                scopes = default_scopes();
            }
            let provider = text(&oidc["name"]).unwrap_or_else(|| host_of(&issuer));
            return Ok(Self {
                issuer,
                client_id,
                scopes: without_openid(scopes),
                provider,
            });
        }
        let connectors: Vec<String> = settings["dexConfig"]["connectors"]
            .as_array()
            .map(|c| c.iter().filter_map(|c| text(&c["name"])).collect())
            .unwrap_or_default();
        if settings["dexConfig"]["connectors"]
            .as_array()
            .is_some_and(|c| !c.is_empty())
        {
            let url = text(&settings["url"]).ok_or(
                "Argo CD's Dex needs its external URL (`url` in argocd-cm) for SSO, and it isn't set.",
            )?;
            let mut scopes = default_scopes();
            scopes.push("federated:id".into());
            return Ok(Self {
                issuer: format!("{}/api/dex", url.trim_end_matches('/')),
                client_id: DEX_CLI_CLIENT.into(),
                scopes: without_openid(scopes),
                provider: if connectors.is_empty() {
                    "Dex".into()
                } else {
                    format!("Dex ({})", connectors.join(", "))
                },
            });
        }
        Err("This Argo CD has no SSO configured.".into())
    }
}

fn default_scopes() -> Vec<String> {
    ["openid", "profile", "email", "groups"]
        .map(String::from)
        .to_vec()
}

fn without_openid(scopes: Vec<String>) -> Vec<String> {
    scopes.into_iter().filter(|s| s != "openid").collect()
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_else(|| url.to_string())
}

/// The Argo CD session: the ID token, and the refresh token when the provider gave one.
pub struct SsoTokens {
    pub id_token: SecretString,
    pub refresh_token: Option<SecretString>,
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::ClientBuilder::new()
        // Following redirects would allow SSRF (openidconnect's recommendation).
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("couldn't create the HTTP client: {e}"))
}

fn chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(err) = source {
        message.push_str(": ");
        message.push_str(&err.to_string());
        source = err.source();
    }
    message
}

async fn metadata(
    config: &SsoConfig,
    http: &reqwest::Client,
) -> Result<CoreProviderMetadata, String> {
    let issuer =
        IssuerUrl::new(config.issuer.clone()).map_err(|e| format!("invalid issuer URL: {e}"))?;
    CoreProviderMetadata::discover_async(issuer, http)
        .await
        .map_err(|e| format!("couldn't reach {}: {}", config.issuer, chain(&e)))
}

/// Signs in through the browser. `open` gets the sign-in URL (it opens the browser). Dropping
/// the future cancels the sign-in and closes the loopback server.
pub async fn sign_in(
    config: &SsoConfig,
    open: impl FnOnce(String) + Send,
) -> Result<SsoTokens, String> {
    let http = http_client()?;
    let metadata = metadata(config, &http).await?;
    let mut scopes = config.scopes.clone();
    // Refresh tokens need `offline_access` where the provider has it (Dex does).
    let offline = metadata
        .scopes_supported()
        .is_some_and(|s| s.iter().any(|s| s.as_str() == "offline_access"));
    if offline && !scopes.iter().any(|s| s == "offline_access") {
        scopes.push("offline_access".into());
    }
    let (listener, _) = bind_loopback(&[CALLBACK_PORT])
        .await
        .map_err(|e| format!("{e} (is `argocd login --sso` running?)"))?;
    let client =
        CoreClient::from_provider_metadata(metadata, ClientId::new(config.client_id.clone()), None)
            .set_redirect_uri(RedirectUrl::new(REDIRECT_URI.into()).map_err(|e| e.to_string())?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scopes(scopes.into_iter().map(Scope::new))
        .set_pkce_challenge(challenge)
        .url();
    open(url.to_string());
    let code = tokio::time::timeout(TIMEOUT, wait_for_code(&listener, csrf.secret()))
        .await
        .map_err(|_| "Timed out waiting for the browser.".to_string())?
        .map_err(|e| e.to_string())?;
    let response = client
        .exchange_code(AuthorizationCode::new(code))
        .map_err(|e| format!("the provider has no token endpoint: {e}"))?
        .set_pkce_verifier(verifier)
        .request_async(&http)
        .await
        .map_err(|e| format!("code exchange failed: {}", chain(&e)))?;
    let id_token = response
        .id_token()
        .ok_or("The provider returned no ID token.")?;
    id_token
        .claims(&client.id_token_verifier(), &nonce)
        .map_err(|e| format!("invalid ID token: {e}"))?;
    Ok(SsoTokens {
        id_token: SecretString::from(id_token.to_string()),
        refresh_token: response
            .refresh_token()
            .map(|t| SecretString::from(t.secret().clone())),
    })
}

/// Renews the session with the refresh token. Providers that rotate refresh tokens return a
/// new one; otherwise the old one stays valid.
pub async fn refresh(
    config: &SsoConfig,
    refresh_token: &SecretString,
) -> Result<SsoTokens, String> {
    let http = http_client()?;
    let metadata = metadata(config, &http).await?;
    let client =
        CoreClient::from_provider_metadata(metadata, ClientId::new(config.client_id.clone()), None);
    let response = client
        .exchange_refresh_token(&RefreshToken::new(
            refresh_token.expose_secret().to_string(),
        ))
        .map_err(|e| format!("the provider has no token endpoint: {e}"))?
        .request_async(&http)
        .await
        .map_err(|e| format!("session refresh failed: {}", chain(&e)))?;
    let id_token = response
        .id_token()
        .ok_or("The provider returned no ID token.")?;
    // Refreshed ID tokens carry no nonce.
    id_token
        .claims(&client.id_token_verifier(), |_: Option<&Nonce>| Ok(()))
        .map_err(|e| format!("invalid ID token: {e}"))?;
    Ok(SsoTokens {
        id_token: SecretString::from(id_token.to_string()),
        refresh_token: Some(
            response
                .refresh_token()
                .map(|t| SecretString::from(t.secret().clone()))
                .unwrap_or_else(|| refresh_token.clone()),
        ),
    })
}

/// Whether a session token expires within a minute (or already has).
pub fn expiring(token: &SecretString) -> bool {
    kubyl_kube::auth::jwt_expiry(token.expose_secret())
        .is_some_and(|exp| exp.as_second() - jiff::Timestamp::now().as_second() < 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dex_sso_uses_the_cli_client_at_the_external_url() {
        let settings = json!({
            "url": "https://argocd.example.com/",
            "dexConfig": {"connectors": [{"name": "GitHub", "type": "github"}]}
        });
        let config = SsoConfig::from_settings(&settings).unwrap();
        assert_eq!(config.issuer, "https://argocd.example.com/api/dex");
        assert_eq!(config.client_id, "argo-cd-cli");
        assert_eq!(
            config.scopes,
            ["profile", "email", "groups", "federated:id"]
        );
        assert_eq!(config.provider, "Dex (GitHub)");

        let no_url = json!({"dexConfig": {"connectors": [{"name": "GitHub"}]}});
        assert!(
            SsoConfig::from_settings(&no_url)
                .unwrap_err()
                .contains("url")
        );
        assert!(SsoConfig::from_settings(&json!({"url": "https://a"})).is_err());
    }

    #[test]
    fn oidc_sso_prefers_the_cli_client() {
        let settings = json!({
            "url": "https://argocd.example.com",
            "oidcConfig": {
                "name": "Okta",
                "issuer": "https://example.okta.com",
                "clientID": "argocd",
                "cliClientID": "argocd-cli",
                "scopes": ["openid", "groups"]
            }
        });
        let config = SsoConfig::from_settings(&settings).unwrap();
        assert_eq!(config.issuer, "https://example.okta.com");
        assert_eq!(config.client_id, "argocd-cli");
        assert_eq!(config.scopes, ["groups"]);
        assert_eq!(config.provider, "Okta");

        let web_client_only =
            json!({"oidcConfig": {"issuer": "https://idp.example.com", "clientID": "argocd"}});
        let config = SsoConfig::from_settings(&web_client_only).unwrap();
        assert_eq!(config.client_id, "argocd");
        assert_eq!(config.scopes, ["profile", "email", "groups"]);
        assert_eq!(config.provider, "idp.example.com");
    }

    #[test]
    fn expiring_reads_the_jwt_exp() {
        use base64::Engine as _;
        let token = |exp: i64| {
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!("{{\"exp\":{exp}}}"));
            SecretString::from(format!("e30.{payload}.sig"))
        };
        let now = jiff::Timestamp::now().as_second();
        assert!(expiring(&token(now - 10)));
        assert!(expiring(&token(now + 30)));
        assert!(!expiring(&token(now + 3600)));
        assert!(!expiring(&SecretString::from("not-a-jwt")));
    }
}
