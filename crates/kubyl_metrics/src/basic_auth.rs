//! Credentials for an in-cluster service behind HTTP basic auth (Prometheus, Alertmanager), as
//! kept in the keychain: the `Basic …` header and the UID of the Service it was given to. A
//! re-created Service (another UID) never gets it: the user signs in again, like Argo CD asks
//! again when its Service is replaced.

use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Value, json};

/// A keychain entry.
pub struct SavedBasic {
    pub header: SecretString,
    pub service_uid: String,
}

impl SavedBasic {
    pub fn to_secret(&self) -> SecretString {
        SecretString::from(
            json!({"header": self.header.expose_secret(), "service_uid": self.service_uid})
                .to_string(),
        )
    }

    pub fn parse(secret: &SecretString) -> Option<Self> {
        let value: Value = serde_json::from_str(secret.expose_secret()).ok()?;
        Some(Self {
            header: SecretString::from(value["header"].as_str()?.to_string()),
            service_uid: value["service_uid"].as_str()?.to_string(),
        })
    }
}

/// The UID of `namespace/service`.
pub async fn service_uid(
    client: &kube::Client,
    namespace: &str,
    service: &str,
) -> Result<String, String> {
    let request = http::Request::get(format!("/api/v1/namespaces/{namespace}/services/{service}"))
        .body(Vec::new())
        .map_err(|e| e.to_string())?;
    let svc: Value = client
        .request(request)
        .await
        .map_err(|e| format!("reading Service {namespace}/{service}: {e}"))?;
    svc.pointer("/metadata/uid")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("Service {namespace}/{service} has no UID"))
}

/// The keychain keys to look for a server's entry in: the entry's id first (new entries go
/// there), then its members' ids (`ConnectionManager::settings_keys`; ids have an `@`, plain
/// context names are left out), so an entry made before contexts were grouped (or regrouped)
/// is still found.
pub fn keys(prefix: &str, cluster_ids: &[String], server: &str) -> Vec<String> {
    cluster_ids
        .iter()
        .enumerate()
        .filter(|(i, id)| *i == 0 || id.contains('@'))
        .map(|(_, id)| format!("{prefix}:{id}/{server}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_and_keys_follow_the_ids() {
        let saved = SavedBasic {
            header: SecretString::from("Basic YWRtaW46eA==".to_string()),
            service_uid: "427266f0".into(),
        };
        let parsed = SavedBasic::parse(&saved.to_secret()).unwrap();
        assert_eq!(parsed.header.expose_secret(), "Basic YWRtaW46eA==");
        assert_eq!(parsed.service_uid, "427266f0");
        // A bare header (an older format) isn't an entry.
        assert!(SavedBasic::parse(&SecretString::from("Basic x".to_string())).is_none());
        let ids = [
            "group:c,u@/k/".to_string(),
            "shop/c/u@/k".to_string(),
            "shop".to_string(),
        ];
        assert_eq!(
            keys("prometheus-auth", &ids, "prometheus/prometheus-server"),
            [
                "prometheus-auth:group:c,u@/k//prometheus/prometheus-server",
                "prometheus-auth:shop/c/u@/k/prometheus/prometheus-server"
            ]
        );
    }
}
