//! L7 data is flow data, and some of it is credentials: sanitized on Tokio while a backend's
//! record is parsed, before anything stores it (README "Sensitive flow fields").
//!
//! - URLs lose their user info and fragment; query values become `…` (the names stay) unless
//!   `netflow.keep_query_values` is on.
//! - Header values of `Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie` and the
//!   usual API key headers are always dropped.

/// Headers whose values are never kept.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-amz-security-token",
    "x-vault-token",
    "private-token",
    "x-csrf-token",
    "x-xsrf-token",
    "x-goog-api-key",
];

/// Words that mark a header as a credential whatever its exact name.
const SECRET_WORDS: &[&str] = &["token", "secret", "password", "api-key", "apikey"];

fn is_secret_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_HEADERS.contains(&name.as_str()) || SECRET_WORDS.iter().any(|w| name.contains(w))
}

/// What replaces a dropped value.
pub const HIDDEN: &str = "…";

/// A URL (absolute or a path) without user info and fragment, query values hidden unless
/// `keep_query_values`.
pub fn url(raw: &str, keep_query_values: bool) -> String {
    let without_fragment = raw.split('#').next().unwrap_or_default();
    let (base, query) = match without_fragment.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (without_fragment, None),
    };
    let base = strip_user_info(base);
    match query {
        None => base,
        Some(query) if keep_query_values => format!("{base}?{query}"),
        Some(query) => {
            let pairs: Vec<String> = query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((name, value)) if !value.is_empty() => format!("{name}={HIDDEN}"),
                    Some((name, _)) => format!("{name}="),
                    None => pair.to_string(),
                })
                .collect();
            if pairs.is_empty() {
                base
            } else {
                format!("{base}?{}", pairs.join("&"))
            }
        }
    }
}

/// `http://user:pass@host/path` → `http://host/path`.
fn strip_user_info(base: &str) -> String {
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_string();
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}{path}"),
        None => base.to_string(),
    }
}

/// The path and query of a URL (`/search?q=…`), for summaries.
pub fn path_of(url: &str) -> &str {
    match url.split_once("://") {
        Some((_, rest)) => match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        },
        None => url,
    }
}

/// Headers with credential values dropped (the names stay, so it's visible they were sent).
pub fn headers<'a>(raw: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<(String, String)> {
    raw.into_iter()
        .map(|(name, value)| {
            let secret = is_secret_header(name);
            (
                name.to_string(),
                if secret {
                    HIDDEN.to_string()
                } else {
                    value.to_string()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_hidden() {
        assert_eq!(
            url("http://web/search?q=shoes&token=not-a-real-token", false),
            "http://web/search?q=…&token=…"
        );
        assert_eq!(
            url("/search?q=shoes&debug&empty=#frag", false),
            "/search?q=…&debug&empty="
        );
        assert_eq!(url("http://web/", false), "http://web/");
        assert_eq!(
            url("http://web/search?q=shoes", true),
            "http://web/search?q=shoes"
        );
    }

    #[test]
    fn user_info_is_dropped() {
        assert_eq!(
            url("https://jane:hunter2@api.example.com/v1?x=1", true),
            "https://api.example.com/v1?x=1"
        );
        assert_eq!(url("http://a@b@host", false), "http://host");
    }

    #[test]
    fn more_credential_headers_are_dropped() {
        let sanitized = headers([
            ("X-Vault-Token", "v"),
            ("PRIVATE-TOKEN", "p"),
            ("X-CSRF-Token", "c"),
            ("X-Goog-Api-Key", "g"),
            ("X-Custom-Secret", "s"),
            ("Accept", "*/*"),
        ]);
        assert!(sanitized[..5].iter().all(|(_, v)| v == HIDDEN));
        assert_eq!(sanitized[5].1, "*/*");
    }

    #[test]
    fn credential_headers_are_dropped() {
        let sanitized = headers([
            ("Authorization", "Bearer abc"),
            ("cookie", "session=abc"),
            ("Set-Cookie", "id=1"),
            ("X-Api-Key", "k"),
            ("User-Agent", "Wget"),
        ]);
        let joined = format!("{sanitized:?}");
        assert!(
            !joined.contains("abc") && !joined.contains("id=1"),
            "{joined}"
        );
        assert_eq!(sanitized[4], ("User-Agent".into(), "Wget".into()));
        assert_eq!(sanitized[0], ("Authorization".into(), HIDDEN.into()));
    }

    #[test]
    fn paths() {
        assert_eq!(path_of("http://web/search?q=…"), "/search?q=…");
        assert_eq!(path_of("http://web"), "/");
        assert_eq!(path_of("/x"), "/x");
    }
}
