//! Small raw requests the Kubernetes-native providers and checks make on Tokio.

use serde_json::Value;

/// `GET path` as JSON.
pub async fn get(client: &kube::Client, path: &str) -> Result<Value, kube::Error> {
    let request = http::Request::get(path)
        .body(Vec::new())
        .map_err(kube::Error::HttpError)?;
    client.request::<Value>(request).await
}

/// `GET path` as text (`/metrics`).
pub async fn get_text(client: &kube::Client, path: &str) -> Result<String, kube::Error> {
    let request = http::Request::get(path)
        .body(Vec::new())
        .map_err(kube::Error::HttpError)?;
    client.request_text(request).await
}

/// A JSON merge patch of the object at `path`.
pub async fn merge_patch(
    client: &kube::Client,
    path: &str,
    patch: &Value,
) -> Result<Value, kube::Error> {
    let body = serde_json::to_vec(patch).map_err(kube::Error::SerdeError)?;
    let request = http::Request::patch(path)
        .header(http::header::CONTENT_TYPE, "application/merge-patch+json")
        .body(body)
        .map_err(kube::Error::HttpError)?;
    client.request::<Value>(request).await
}

/// The items of a list response.
pub fn items(list: &Value) -> &[Value] {
    list.get("items")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

pub fn array_at<'a>(value: &'a Value, pointer: &str) -> &'a [Value] {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub fn int_at(value: &Value, pointer: &str) -> Option<i64> {
    value.pointer(pointer).and_then(Value::as_i64)
}

pub fn timestamp_at(value: &Value, pointer: &str) -> Option<jiff::Timestamp> {
    str_at(value, pointer).and_then(|s| s.parse().ok())
}

/// Whether a kube error is a 404 (a kind the cluster doesn't serve, an object that's gone).
pub fn is_not_found(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 404)
}

/// Whether a kube error is a 403.
pub fn is_forbidden(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(status) if status.code == 403)
}

/// The conditions of an object's status as [`crate::model::Condition`]s.
pub fn conditions(object: &Value) -> Vec<crate::model::Condition> {
    array_at(object, "/status/conditions")
        .iter()
        .map(|c| crate::model::Condition {
            kind: str_at(c, "/type").unwrap_or_default().to_string(),
            status: match str_at(c, "/status") {
                Some("True") => Some(true),
                Some("False") => Some(false),
                _ => None,
            },
            reason: str_at(c, "/reason").map(str::to_string),
            message: str_at(c, "/message").map(str::to_string),
            since: timestamp_at(c, "/lastTransitionTime"),
        })
        .collect()
}
