//! The JSON-RPC 2.0 pieces MCP's request/response transport needs: the error object and the
//! handling of one message. (The agent crate's ACP client has the streaming side.)

use serde_json::{Value, json};

/// A JSON-RPC error object.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// ACP: the agent needs `authenticate` first.
    pub const AUTH_REQUIRED: i64 = -32000;
    /// ACP: a resource (file, terminal) wasn't found.
    pub const RESOURCE_NOT_FOUND: i64 = -32002;

    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            Self::METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        )
    }

    pub fn invalid_params(err: impl std::fmt::Display) -> Self {
        Self::new(Self::INVALID_PARAMS, format!("Invalid params: {err}"))
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Self::INTERNAL_ERROR, message)
    }

    /// The connection closed before an answer came.
    pub fn closed() -> Self {
        Self::internal("The connection closed")
    }

    pub fn to_value(&self) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        error
    }

    pub fn from_value(value: &Value) -> Self {
        Self {
            code: value["code"].as_i64().unwrap_or(Self::INTERNAL_ERROR),
            message: value["message"].as_str().unwrap_or("Unknown error").into(),
            data: value.get("data").cloned(),
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// Handles one JSON-RPC message for a request/response transport (MCP over HTTP): returns the
/// response line for a request, `None` for a notification or a response.
pub async fn handle_single<F, Fut>(text: &str, handler: F) -> Option<Value>
where
    F: FnOnce(String, Value) -> Fut,
    Fut: std::future::Future<Output = Option<Result<Value, RpcError>>>,
{
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        let error = RpcError::new(RpcError::PARSE_ERROR, "Parse error").to_value();
        return Some(json!({ "jsonrpc": "2.0", "id": null, "error": error }));
    };
    let id = value.get("id").filter(|id| !id.is_null()).cloned();
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        // An answer to something we sent gets no reply; a request without a usable method does.
        let is_response = value.get("result").is_some() || value.get("error").is_some();
        return match id {
            Some(id) if !is_response => {
                let error = RpcError::new(RpcError::INVALID_REQUEST, "Invalid Request");
                Some(json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() }))
            }
            _ => None,
        };
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let result = handler(method.to_string(), params).await;
    let id = id?;
    Some(match result {
        Some(Ok(result)) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Some(Err(error)) => json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() }),
        None => {
            let error = RpcError::method_not_found(method).to_value();
            json!({ "jsonrpc": "2.0", "id": id, "error": error })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn single_messages_answer_requests_only() {
        let answer = handle_single(
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
            |_, _| async { Some(Ok(json!({}))) },
        )
        .await
        .unwrap();
        assert_eq!(answer["id"], 7);
        let none = handle_single(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            |_, _| async { Some(Ok(json!({}))) },
        )
        .await;
        assert!(none.is_none());
        let unknown = handle_single(r#"{"jsonrpc":"2.0","id":"a","method":"x"}"#, |_, _| async {
            None
        })
        .await
        .unwrap();
        assert_eq!(unknown["error"]["code"], RpcError::METHOD_NOT_FOUND);
        // A request with an id but no method is an error; a response is ignored.
        let no_method = handle_single(r#"{"jsonrpc":"2.0","id":3}"#, |_, _| async { None })
            .await
            .unwrap();
        assert_eq!(no_method["error"]["code"], RpcError::INVALID_REQUEST);
        assert_eq!(no_method["id"], 3);
        assert!(
            handle_single(r#"{"jsonrpc":"2.0","id":4,"result":{}}"#, |_, _| async {
                None
            })
            .await
            .is_none()
        );
        let broken = handle_single("{", |_, _| async { None }).await.unwrap();
        assert_eq!(broken["error"]["code"], RpcError::PARSE_ERROR);
    }
}
