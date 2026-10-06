//! JSON-RPC 2.0 over newline-delimited JSON, the framing ACP and MCP's stdio transport share.
//!
//! [`connect`] starts a reader and a writer task on the current Tokio runtime and returns a
//! [`Peer`] for outgoing requests and notifications, and a receiver of what the other side
//! sends: requests (answered through [`Responder`]) and notifications. Responses to our own
//! requests never show up there; they resolve the future [`Peer::request`] returned.
//!
//! Nothing here logs message contents: they hold prompts, tool results and file contents.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, BufReader,
};
use tokio::sync::{mpsc, oneshot};

/// Lines longer than this end the connection (a runaway or hostile peer).
const MAX_LINE: usize = 64 * 1024 * 1024;

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

    fn to_value(&self) -> Value {
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        error
    }

    fn from_value(value: &Value) -> Self {
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

/// Something the other side sent us.
#[derive(Debug)]
pub enum Incoming {
    Request {
        method: String,
        params: Value,
        responder: Responder,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// The answer to a request sent with [`Peer::send_ordered`], in order with everything else
    /// the peer sent.
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
}

/// Answers one incoming request. Dropping it unanswered sends an internal error, so the peer
/// never waits forever.
#[derive(Debug)]
pub struct Responder {
    id: Option<Value>,
    out: mpsc::UnboundedSender<String>,
}

impl Responder {
    pub fn ok(mut self, result: impl Serialize) -> bool {
        match serde_json::to_value(result) {
            Ok(result) => self.send(Ok(result)),
            Err(err) => self.send(Err(RpcError::internal(err.to_string()))),
        }
    }

    pub fn err(mut self, error: RpcError) -> bool {
        self.send(Err(error))
    }

    pub fn respond(mut self, result: Result<Value, RpcError>) -> bool {
        self.send(result)
    }

    fn send(&mut self, result: Result<Value, RpcError>) -> bool {
        let Some(id) = self.id.take() else {
            return false;
        };
        let message = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error.to_value() }),
        };
        self.out.send(message.to_string()).is_ok()
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if self.id.is_some() {
            self.send(Err(RpcError::internal("The request wasn't answered")));
        }
    }
}

/// Waiters of requests: a future ([`Peer::request`]) or the incoming stream
/// ([`Peer::send_ordered`]).
#[derive(Debug)]
enum Waiter {
    Future(oneshot::Sender<Result<Value, RpcError>>),
    Ordered,
}

type Pending = Arc<Mutex<HashMap<u64, Waiter>>>;

/// Our side of a connection: sends requests and notifications. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Peer {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl Peer {
    /// Sends a request and waits for its result.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, RpcError> {
        let value = self.request_value(method, params).await?;
        serde_json::from_value(value)
            .map_err(|err| RpcError::internal(format!("Unexpected answer to {method}: {err}")))
    }

    /// Like [`Peer::request`], without decoding the result.
    pub async fn request_value(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<Value, RpcError> {
        let params = serde_json::to_value(params).map_err(|e| RpcError::internal(e.to_string()))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, Waiter::Future(tx));
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if self.out.send(message.to_string()).is_err() {
            self.pending.lock().remove(&id);
            return Err(RpcError::closed());
        }
        rx.await.unwrap_or_else(|_| Err(RpcError::closed()))
    }

    /// Sends a request whose answer arrives as [`Incoming::Response`] on the incoming stream,
    /// after everything the peer sent before it. Returns the request's id.
    pub fn send_ordered(&self, method: &str, params: impl Serialize) -> Result<u64, RpcError> {
        let params = serde_json::to_value(params).map_err(|e| RpcError::internal(e.to_string()))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().insert(id, Waiter::Ordered);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if self.out.send(message.to_string()).is_err() {
            self.pending.lock().remove(&id);
            return Err(RpcError::closed());
        }
        Ok(id)
    }

    pub fn notify(&self, method: &str, params: impl Serialize) -> bool {
        let Ok(params) = serde_json::to_value(params) else {
            return false;
        };
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.out.send(message.to_string()).is_ok()
    }

    /// Whether the writer is still running.
    pub fn is_open(&self) -> bool {
        !self.out.is_closed()
    }
}

/// Starts the reader and writer tasks for one connection. Must run inside a Tokio runtime.
///
/// The receiver ends when the reader hits EOF or an I/O error; pending requests then fail with
/// [`RpcError::closed`].
pub fn connect<R, W>(reader: R, writer: W) -> (Peer, mpsc::UnboundedReceiver<Incoming>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let pending: Pending = Arc::default();

    let mut writer = writer;
    tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err()
                || writer.write_all(b"\n").await.is_err()
                || writer.flush().await.is_err()
            {
                break;
            }
        }
    });

    let peer = Peer {
        out: out_tx.clone(),
        pending: pending.clone(),
        next_id: Arc::new(AtomicU64::new(1)),
    };
    tokio::spawn(async move {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match (&mut reader)
                .take(MAX_LINE as u64)
                .read_until(b'\n', &mut line)
                .await
            {
                Ok(0) | Err(_) => break,
                Ok(_) if line.len() >= MAX_LINE && line.last() != Some(&b'\n') => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            dispatch(text, &out_tx, &pending, &in_tx);
        }
        // Fail whatever still waits for an answer.
        for (id, waiter) in pending.lock().drain() {
            match waiter {
                Waiter::Future(waiter) => {
                    waiter.send(Err(RpcError::closed())).ok();
                }
                Waiter::Ordered => {
                    in_tx
                        .send(Incoming::Response {
                            id,
                            result: Err(RpcError::closed()),
                        })
                        .ok();
                }
            }
        }
    });

    (peer, in_rx)
}

fn dispatch(
    text: &str,
    out: &mpsc::UnboundedSender<String>,
    pending: &Pending,
    incoming: &mpsc::UnboundedSender<Incoming>,
) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        let error = RpcError::new(RpcError::PARSE_ERROR, "Parse error").to_value();
        out.send(json!({ "jsonrpc": "2.0", "id": null, "error": error }).to_string())
            .ok();
        return;
    };
    // Batches are allowed by JSON-RPC; neither ACP nor MCP peers send them, but handle them.
    if let Value::Array(items) = value {
        for item in items {
            dispatch_one(item, out, pending, incoming);
        }
    } else {
        dispatch_one(value, out, pending, incoming);
    }
}

fn dispatch_one(
    value: Value,
    out: &mpsc::UnboundedSender<String>,
    pending: &Pending,
    incoming: &mpsc::UnboundedSender<Incoming>,
) {
    let method = value.get("method").and_then(Value::as_str);
    let id = value.get("id").filter(|id| !id.is_null()).cloned();
    match (method, id) {
        (Some(method), Some(id)) => {
            let responder = Responder {
                id: Some(id),
                out: out.clone(),
            };
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            incoming
                .send(Incoming::Request {
                    method: method.to_string(),
                    params,
                    responder,
                })
                .ok();
        }
        (Some(method), None) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            incoming
                .send(Incoming::Notification {
                    method: method.to_string(),
                    params,
                })
                .ok();
        }
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else {
                return;
            };
            let Some(waiter) = pending.lock().remove(&id) else {
                return;
            };
            let result = match value.get("error") {
                Some(error) if !error.is_null() => Err(RpcError::from_value(error)),
                _ => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            match waiter {
                Waiter::Future(waiter) => {
                    waiter.send(result).ok();
                }
                Waiter::Ordered => {
                    incoming.send(Incoming::Response { id, result }).ok();
                }
            }
        }
        (None, None) => {}
    }
}

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
    let method = value.get("method").and_then(Value::as_str)?;
    let id = value.get("id").filter(|id| !id.is_null()).cloned();
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
    async fn requests_and_responses_cross_both_ways() {
        let (a, b) = tokio::io::duplex(4096);
        let (a_read, a_write) = tokio::io::split(a);
        let (b_read, b_write) = tokio::io::split(b);
        let (client, _client_in) = connect(a_read, a_write);
        let (_server, mut server_in) = connect(b_read, b_write);

        tokio::spawn(async move {
            while let Some(message) = server_in.recv().await {
                if let Incoming::Request {
                    method,
                    params,
                    responder,
                } = message
                {
                    match method.as_str() {
                        "echo" => responder.ok(params),
                        "fail" => responder.err(RpcError::invalid_params("nope")),
                        _ => {
                            drop(responder);
                            true
                        }
                    };
                }
            }
        });

        let echoed: Value = client.request("echo", json!({"x": 1})).await.unwrap();
        assert_eq!(echoed, json!({"x": 1}));
        let err = client.request_value("fail", Value::Null).await.unwrap_err();
        assert_eq!(err.code, RpcError::INVALID_PARAMS);
        // A dropped responder still answers.
        let err = client
            .request_value("other", Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.code, RpcError::INTERNAL_ERROR);
    }

    #[tokio::test]
    async fn pending_requests_fail_when_the_peer_goes_away() {
        let (a, b) = tokio::io::duplex(4096);
        let (a_read, a_write) = tokio::io::split(a);
        let (client, _in) = connect(a_read, a_write);
        let request = tokio::spawn(async move { client.request_value("x", Value::Null).await });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(b);
        assert_eq!(request.await.unwrap().unwrap_err(), RpcError::closed());
    }

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
    }
}
