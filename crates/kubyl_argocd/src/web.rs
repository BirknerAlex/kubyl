//! Argo CD's web UI in a web view, signed in with API mode's session. SSO can't work through
//! the loopback forward (Argo CD only accepts its own `url` as a redirect), so the page gets the
//! session token as its `argocd.token` cookie instead: only for the argocd-server Service of the
//! install the user confirmed, and only while API mode is signed in there.

use gpui::App;
use kubyl_webview::SessionCookie;
use kubyl_webview::target::{TargetKind, WebTarget};
use secrecy::{ExposeSecret as _, SecretString};

use crate::state::ArgoCd;

/// Argo CD's session cookie.
pub const COOKIE: &str = "argocd.token";
/// Value bytes per cookie (browsers keep at most 4 KB per cookie, name and attributes included).
const CHUNK: usize = 3800;

pub(crate) fn init(cx: &mut App) {
    kubyl_webview::add_session_provider(cx, cookies_for);
}

fn cookies_for(target: &WebTarget, cx: &App) -> Vec<SessionCookie> {
    if target.kind != TargetKind::Service {
        return Vec::new();
    }
    ArgoCd::try_global(cx)
        .and_then(|argo| {
            argo.read(cx)
                .web_session_token(&target.cluster, &target.namespace, &target.name, cx)
        })
        .map(|token| session_cookies(&token))
        .unwrap_or_default()
}

/// Argo CD's cookie format (argocd-server's `MakeCookieMetadata`): one `argocd.token`, or
/// `argocd.token=<n>:<part>` plus `argocd.token-1`… for tokens over 4 KB (SSO tokens with many
/// groups).
pub fn session_cookies(token: &SecretString) -> Vec<SessionCookie> {
    let value = token.expose_secret();
    if value.len() <= CHUNK {
        return vec![SessionCookie {
            name: COOKIE.into(),
            value: token.clone(),
        }];
    }
    // JWTs are ASCII: byte chunks are char boundaries.
    let parts: Vec<&str> = value
        .as_bytes()
        .chunks(CHUNK)
        .map(|c| std::str::from_utf8(c).unwrap_or_default())
        .collect();
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| match i {
            0 => SessionCookie {
                name: COOKIE.into(),
                value: SecretString::from(format!("{}:{part}", parts.len())),
            },
            i => SessionCookie {
                name: format!("{COOKIE}-{i}"),
                value: SecretString::from(part.to_string()),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_tokens_split_like_argo_cd() {
        let short = session_cookies(&SecretString::from("abc.def.ghi"));
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].name, "argocd.token");
        assert_eq!(short[0].value.expose_secret(), "abc.def.ghi");

        let token: String = (0..9000).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let cookies = session_cookies(&SecretString::from(token.clone()));
        let names: Vec<&str> = cookies.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["argocd.token", "argocd.token-1", "argocd.token-2"]);
        let first = cookies[0].value.expose_secret();
        assert!(first.starts_with("3:"));
        // argocd-server's JoinCookies: the count, then the parts in order.
        let joined: String = std::iter::once(&first[2..])
            .chain(cookies[1..].iter().map(|c| c.value.expose_secret()))
            .collect();
        assert_eq!(joined, token);
    }
}
