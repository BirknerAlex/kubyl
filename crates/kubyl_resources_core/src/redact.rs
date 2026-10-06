//! Masking for everything that leaves Kubyl's own views: the clipboard, and what agents read
//! (phase 21).
//!
//! - [`mask_secret`]: a core Secret's values and kubectl's last-applied copy of them.
//! - [`mask_object`]: [`mask_secret`], a Route's inline TLS key ([`crate::route`]) and
//!   `managedFields`.
//! - [`is_helm_release`]: Helm's release Secrets and ConfigMaps, which hold the release's values
//!   (and so often credentials); agents never get them.
//! - [`scrub_text`]: best-effort removal of token shapes (JWTs, bearer and basic credentials,
//!   OpenShift tokens, private keys, well-known API key formats) from free text like logs. A
//!   safety net, not a guarantee.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// Placeholder for masked values, the same text the YAML editor and details show.
pub const MASK: &str = crate::route::MASK;

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// Masks the `data`/`stringData` values of a core Secret and kubectl's last-applied copy of
/// them. Returns whether it is a Secret.
pub fn mask_secret(object: &mut Value) -> bool {
    if object["kind"].as_str() != Some("Secret") || object["apiVersion"].as_str() != Some("v1") {
        return false;
    }
    for field in ["data", "stringData"] {
        if let Some(map) = object.get_mut(field).and_then(Value::as_object_mut) {
            for value in map.values_mut() {
                *value = Value::String(MASK.into());
            }
        }
    }
    if let Some(annotation) = object
        .pointer_mut("/metadata/annotations")
        .and_then(Value::as_object_mut)
        .and_then(|a| a.get_mut(LAST_APPLIED))
    {
        *annotation = Value::String(MASK.into());
    }
    true
}

/// Masks Secret values and a Route's inline key, and drops `managedFields` (noise, and they can
/// echo field names of masked values). Returns whether something was masked.
pub fn mask_object(object: &mut Value) -> bool {
    if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        metadata.remove("managedFields");
    }
    let fields = mask_credential_fields(object);
    mask_secret(object) | crate::route::mask_inline_key(object) | fields
}

/// Map keys and env var names that hold credentials: `password`, `dbPassword`, `authToken`,
/// `clientSecret`, `apiKey`, `DB_PASSWORD`… (the credential word at the end of the name).
static CREDENTIAL_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)$")
        .expect("valid credential name pattern")
});

/// Masks string values under credential-like keys anywhere in `value` (a custom resource's
/// spec, a ConfigMap), and the `value` of env vars with credential-like names. Returns whether
/// something was masked.
pub fn mask_credential_fields(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut masked = false;
            let env_name = map
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| CREDENTIAL_NAME.is_match(name));
            for (key, child) in map.iter_mut() {
                let sensitive = CREDENTIAL_NAME.is_match(key) || (env_name && key == "value");
                match child {
                    Value::String(text) if sensitive && !text.is_empty() && text != MASK => {
                        *text = MASK.into();
                        masked = true;
                    }
                    _ => masked |= mask_credential_fields(child),
                }
            }
            masked
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |masked, item| mask_credential_fields(item) | masked),
        _ => false,
    }
}

/// A Secret or ConfigMap that holds a Helm release (`owner=helm`): its payload is the release's
/// values and manifest.
pub fn is_helm_release(object: &Value) -> bool {
    let kind = object["kind"].as_str();
    matches!(kind, Some("Secret" | "ConfigMap"))
        && object
            .pointer("/metadata/labels/owner")
            .and_then(Value::as_str)
            == Some("helm")
}

struct Rule {
    pattern: Regex,
    /// Replacement with `$1` for a prefix the pattern keeps.
    replace: &'static str,
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    let rule = |pattern: &str, replace: &'static str| Rule {
        pattern: Regex::new(pattern).expect("valid scrub pattern"),
        replace,
    };
    vec![
        // PEM private keys (RSA, EC, OPENSSH, ENCRYPTED…), whole block.
        rule(
            r"-----BEGIN ([A-Z0-9 ]*PRIVATE KEY)-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
            "-----BEGIN $1-----\n••••••••\n-----END $1-----",
        ),
        // `Authorization: Bearer x`, `"authorization":"Basic x"`, `Authorization=token x`.
        rule(
            r#"(?i)(authorization["']?\s*[:=]\s*["']?(?:bearer\s+|basic\s+|token\s+)?)[^\s"',;]{6,}"#,
            "${1}••••••••",
        ),
        rule(r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/=-]{8,}", "${1}••••••••"),
        // JWTs (service-account tokens, OIDC ID tokens).
        rule(
            r"\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]*",
            "••••••••",
        ),
        // OpenShift OAuth tokens.
        rule(r"\bsha256~[A-Za-z0-9_-]{20,}", "sha256~••••••••"),
        // GitHub, GitLab, Slack tokens, AWS access key ids, Google API keys.
        rule(
            r"\b(gh[pousr]_|github_pat_)[A-Za-z0-9_]{20,}",
            "${1}••••••••",
        ),
        rule(r"\bglpat-[A-Za-z0-9_-]{20,}", "glpat-••••••••"),
        rule(r"\bxox[abprs]-[A-Za-z0-9-]{10,}", "xox-••••••••"),
        rule(r"\b(AKIA|ASIA)[0-9A-Z]{16}\b", "${1}••••••••"),
        rule(r"\bAIza[0-9A-Za-z_-]{35}\b", "AIza••••••••"),
        // `password=…`, `-DatabasePassword="…"`, `AuthToken=\"…\"`, `"client_secret": "…"`:
        // any key that ends in a credential word, also inside a longer name, with plain, quoted
        // or escaped-quoted values.
        rule(
            r#"(?i)((?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)\\?["']?\s*[:=]\s*\\?["']?)[^\s"'\\,;&]{4,}"#,
            "${1}••••••••",
        ),
        // Credentials in URLs: `scheme://user:password@host`.
        rule(
            r"(\b[a-z][a-z0-9+.-]*://[^/\s:@]+:)[^/\s@]+@",
            "${1}••••••••@",
        ),
    ]
});

/// `text` with token shapes replaced by [`MASK`]. Borrowed when nothing matched.
pub fn scrub_text(text: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(text);
    for rule in RULES.iter() {
        if let Cow::Owned(replaced) = rule.pattern.replace_all(&out, rule.replace) {
            out = Cow::Owned(replaced);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn secrets_lose_their_values_and_last_applied_copy() {
        let mut secret = json!({
            "apiVersion": "v1", "kind": "Secret",
            "metadata": {"name": "db", "managedFields": [{"manager": "kubectl"}],
                "annotations": {LAST_APPLIED: "{\"data\":{\"password\":\"aHVudGVyMg==\"}}"}},
            "data": {"password": "aHVudGVyMg=="},
            "stringData": {"user": "admin"},
        });
        assert!(mask_object(&mut secret));
        assert_eq!(secret["data"]["password"], MASK);
        assert_eq!(secret["stringData"]["user"], MASK);
        assert_eq!(secret["metadata"]["annotations"][LAST_APPLIED], MASK);
        assert!(secret["metadata"].get("managedFields").is_none());

        let mut config = json!({"apiVersion": "v1", "kind": "ConfigMap", "data": {"a": "b"}});
        assert!(!mask_object(&mut config));
        assert_eq!(config["data"]["a"], "b");
    }

    #[test]
    fn credential_fields_are_masked_anywhere() {
        let mut battle_group = json!({
            "apiVersion": "example.com/v1", "kind": "BattleGroup",
            "metadata": {"name": "bg"},
            "spec": {
                "dbPassword": "v2dU-4ERG",
                "authToken": "xObrc4oG",
                "secretName": "keep-me",
                "serviceAccountToken": true,
                "env": [
                    {"name": "DB_PASSWORD", "value": "s3cret"},
                    {"name": "LOG_LEVEL", "value": "info"},
                    {"name": "API_KEY", "valueFrom": {"secretKeyRef": {"name": "k", "key": "api-key"}}},
                ],
            },
        });
        assert!(mask_object(&mut battle_group));
        let spec = &battle_group["spec"];
        assert_eq!(spec["dbPassword"], MASK);
        assert_eq!(spec["authToken"], MASK);
        assert_eq!(spec["secretName"], "keep-me");
        assert_eq!(spec["serviceAccountToken"], true);
        assert_eq!(spec["env"][0]["value"], MASK);
        assert_eq!(spec["env"][1]["value"], "info");
        assert_eq!(
            spec["env"][2]["valueFrom"]["secretKeyRef"]["key"],
            "api-key"
        );
    }

    #[test]
    fn helm_release_storage_is_recognised() {
        let release = json!({"kind": "Secret", "metadata": {"labels": {"owner": "helm"}}});
        assert!(is_helm_release(&release));
        let config = json!({"kind": "ConfigMap", "metadata": {"labels": {"owner": "helm"}}});
        assert!(is_helm_release(&config));
        assert!(!is_helm_release(&json!({"kind": "Secret", "metadata": {}})));
    }

    #[test]
    fn token_shapes_are_scrubbed() {
        let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJzeXN0ZW06c2EifQ.c2lnbmF0dXJlLWJ5dGVz";
        let cases = [
            (format!("token={jwt} ok"), jwt),
            (
                "GET / Authorization: Bearer abcdef0123456789".to_string(),
                "abcdef0123456789",
            ),
            (
                r#"{"authorization":"Basic dXNlcjpwYXNz"}"#.to_string(),
                "dXNlcjpwYXNz",
            ),
            (
                "oc login --token=sha256~AbCdEfGhIjKlMnOpQrStUvWxYz012345".to_string(),
                "AbCdEfGhIjKlMnOpQrStUvWxYz012345",
            ),
            (
                "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAKC\n-----END RSA PRIVATE KEY-----"
                    .to_string(),
                "MIIEow",
            ),
            ("db password=hunter22 started".to_string(), "hunter22"),
            (
                r#"- -DatabasePassword="v2dU-4ERG.LEnoH8""#.to_string(),
                "v2dU-4ERG",
            ),
            (
                r#"[Live]:ServerCommandsAuthToken=\"xObrc4oG2E08AprDfTkP\","#.to_string(),
                "xObrc4oG2E08",
            ),
            ("clientSecret: abcd1234".to_string(), "abcd1234"),
            ("dsn postgres://app:s3cret@db:5432/x".to_string(), "s3cret"),
            (
                "key AKIAIOSFODNN7EXAMPLE used".to_string(),
                "IOSFODNN7EXAMPLE",
            ),
            (
                "ghp_0123456789abcdefghijABCDEFGHIJ012345".to_string(),
                "0123456789abcdefghij",
            ),
        ];
        for (text, secret) in cases {
            let scrubbed = scrub_text(&text);
            assert!(!scrubbed.contains(secret), "{text} -> {scrubbed}");
            assert!(scrubbed.contains(MASK), "{text} -> {scrubbed}");
        }
    }

    #[test]
    fn ordinary_text_is_borrowed() {
        let line = "2026-10-07T10:00:00Z level=info msg=\"listening on :8080\" pod=web-0";
        assert!(matches!(scrub_text(line), Cow::Borrowed(_)));
    }
}
