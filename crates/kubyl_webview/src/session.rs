//! Session cookies for web views of a target another crate holds a session for: Argo CD's API
//! mode signs its web UI in with the token the user signed in with. Nothing else gets
//! credentials: a provider answers only for the targets it trusts.

use std::rc::Rc;

use gpui::{App, Global};
use secrecy::SecretString;

use crate::target::WebTarget;

/// A cookie for the view's (loopback) origin, set before the first page loads. Session only
/// (never written to the view's data store), HttpOnly, Secure on HTTPS.
#[derive(Clone, Debug)]
pub struct SessionCookie {
    pub name: String,
    pub value: SecretString,
}

type Provider = Rc<dyn Fn(&WebTarget, &App) -> Vec<SessionCookie>>;

#[derive(Default)]
struct Providers(Vec<Provider>);

impl Global for Providers {}

/// Registers a provider of session cookies. It's asked each time a web view of a target is
/// created; return nothing for targets you don't hold a session for.
pub fn add_session_provider(
    cx: &mut App,
    provider: impl Fn(&WebTarget, &App) -> Vec<SessionCookie> + 'static,
) {
    cx.default_global::<Providers>().0.push(Rc::new(provider));
}

/// The session cookies of `target`, from every provider.
pub fn cookies_for(target: &WebTarget, cx: &App) -> Vec<SessionCookie> {
    cx.try_global::<Providers>()
        .map(|p| {
            p.0.iter()
                .flat_map(|provider| provider(target, cx))
                .collect()
        })
        .unwrap_or_default()
}

/// `cookies` for the page at `url`, in wry's form. Only for loopback URLs (the view's forward).
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub(crate) fn wry_cookies(
    url: &str,
    cookies: &[SessionCookie],
) -> Vec<wry::cookie::Cookie<'static>> {
    use secrecy::ExposeSecret as _;
    use wry::cookie::{Cookie, SameSite};

    let Ok(url) = url::Url::parse(url) else {
        return Vec::new();
    };
    let Some(host) = url.host_str().filter(|h| crate::native::is_loopback(h)) else {
        return Vec::new();
    };
    let secure = url.scheme() == "https";
    cookies
        .iter()
        .map(|cookie| {
            Cookie::build((
                cookie.name.clone(),
                cookie.value.expose_secret().to_string(),
            ))
            .domain(host.to_string())
            .path("/")
            .secure(secure)
            .http_only(true)
            .same_site(SameSite::Lax)
            .build()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    #[test]
    fn cookies_only_for_loopback_pages() {
        let cookies = [SessionCookie {
            name: "argocd.token".into(),
            value: SecretString::from("t0ken"),
        }];
        let set = wry_cookies("https://127.0.0.1:41234/applications", &cookies);
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].domain(), Some("127.0.0.1"));
        assert_eq!(set[0].secure(), Some(true));
        assert_eq!(set[0].http_only(), Some(true));
        assert_eq!(set[0].expires(), None, "a session cookie");
        assert_eq!(
            wry_cookies("http://127.0.0.1:8080/", &cookies)[0].secure(),
            Some(false)
        );
        assert!(wry_cookies("https://argocd.example.com/", &cookies).is_empty());
        assert!(format!("{:?}", cookies[0]).contains("REDACTED"));
    }
}
