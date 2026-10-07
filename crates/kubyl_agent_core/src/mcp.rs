//! Kubyl's MCP server: the cluster tools of one thread, for its agent.
//!
//! One server per thread listens on `127.0.0.1` (a random port) and speaks MCP's streamable
//! HTTP transport in its simplest form: every `POST /mcp` carries one JSON-RPC message and gets
//! a JSON answer (no event streams, no sessions). Each request needs the thread's random bearer
//! token (memory only, compared in constant time); requests from browsers (an `Origin` header)
//! are refused. Dropping [`McpServer`] stops it.
//!
//! Agents that can't reach an HTTP MCP server get [`bridge`]: the Kubyl binary started as
//! `kubyl mcp-bridge`, which relays stdio to the same endpoint.

use std::net::SocketAddr;
use std::sync::Arc;

use base64::Engine as _;
use http::{Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use crate::jsonrpc::{self, RpcError};
use crate::tools::{self, ToolContext};

/// The MCP server's name in the agent's tool list (`mcp__kubyl__…`).
pub const SERVER_NAME: &str = "kubyl";
/// Environment of `kubyl mcp-bridge`.
pub const BRIDGE_URL_ENV: &str = "KUBYL_MCP_URL";
pub const BRIDGE_TOKEN_ENV: &str = "KUBYL_MCP_TOKEN";
/// The subcommand of the Kubyl binary that runs [`bridge`].
pub const BRIDGE_ARG: &str = "mcp-bridge";

/// MCP versions this server speaks, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// Largest request body.
const MAX_BODY: usize = 1024 * 1024;

const INSTRUCTIONS: &str = "Kubyl's read-only view of one Kubernetes cluster, through the user's own account. \
Start with cluster_info. Secret values, private keys and token-like strings are masked; \
Kubyl never reveals them, so don't try to work around the masking. \
These tools can't change the cluster; when a change would help, show the user the manifest or command.";

/// A tool call, for the thread's transcript.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolActivity {
    pub tool: String,
    pub arguments: Value,
    pub is_error: bool,
}

/// A running server. Dropping it stops listening.
pub struct McpServer {
    pub addr: SocketAddr,
    token: String,
    task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl McpServer {
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// The bearer token. Goes only to the agent (in `session/new`), never to logs or state.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The server as ACP's `session/new` names it: HTTP when the agent supports it, else the
    /// stdio bridge (`bridge_program` is the Kubyl binary).
    pub fn acp_entry(&self, http: bool, bridge_program: &std::path::Path) -> Value {
        if http {
            json!({
                "type": "http",
                "name": SERVER_NAME,
                "url": self.url(),
                "headers": [{ "name": "Authorization", "value": format!("Bearer {}", self.token) }],
            })
        } else {
            json!({
                "name": SERVER_NAME,
                "command": bridge_program,
                "args": [BRIDGE_ARG],
                "env": [
                    { "name": BRIDGE_URL_ENV, "value": self.url() },
                    { "name": BRIDGE_TOKEN_ENV, "value": self.token },
                ],
            })
        }
    }
}

/// A random token (256 bits, base64url).
pub fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

struct Shared {
    token: String,
    context: watch::Receiver<ToolContext>,
    activity: Option<mpsc::UnboundedSender<ToolActivity>>,
}

/// Starts a server for one thread. Must run inside a Tokio runtime.
pub async fn serve(
    context: watch::Receiver<ToolContext>,
    activity: Option<mpsc::UnboundedSender<ToolActivity>>,
) -> std::io::Result<McpServer> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let token = new_token()?;
    let shared = Arc::new(Shared {
        token: token.clone(),
        context,
        activity,
    });
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // A lasting error (out of file descriptors) mustn't spin the runtime.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            };
            let shared = shared.clone();
            connections.spawn(async move {
                let service = hyper::service::service_fn(move |request| {
                    let shared = shared.clone();
                    async move { Ok::<_, std::convert::Infallible>(handle(&shared, request).await) }
                });
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                    .ok();
            });
            // Reap finished connections so the set doesn't grow.
            while connections.try_join_next().is_some() {}
        }
    });
    Ok(McpServer { addr, token, task })
}

fn reply(status: StatusCode, body: impl Into<Bytes>, json: bool) -> Response<Full<Bytes>> {
    let mut response = Response::builder().status(status);
    if json {
        response = response.header(header::CONTENT_TYPE, "application/json");
    }
    response
        .body(Full::new(body.into()))
        .unwrap_or_else(|_| Response::new(Full::new(Bytes::new())))
}

async fn handle(shared: &Shared, request: Request<Incoming>) -> Response<Full<Bytes>> {
    if request.uri().path() != "/mcp" {
        return reply(StatusCode::NOT_FOUND, "", false);
    }
    // Browsers send an Origin; agents don't. A page must never reach these tools.
    if request.headers().contains_key(header::ORIGIN) {
        return reply(StatusCode::FORBIDDEN, "", false);
    }
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| constant_time_eq(t.trim().as_bytes(), shared.token.as_bytes()));
    if !authorized {
        return reply(StatusCode::UNAUTHORIZED, "", false);
    }
    match *request.method() {
        Method::POST => {}
        // No server-initiated stream and no sessions to end.
        Method::GET => return reply(StatusCode::METHOD_NOT_ALLOWED, "", false),
        Method::DELETE => return reply(StatusCode::OK, "", false),
        _ => return reply(StatusCode::METHOD_NOT_ALLOWED, "", false),
    }
    let body = match Limited::new(request.into_body(), MAX_BODY).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, "", false),
    };
    let text = String::from_utf8_lossy(&body);
    match handle_message(shared, &text).await {
        Some(answer) => reply(StatusCode::OK, answer.to_string(), true),
        None => reply(StatusCode::ACCEPTED, "", false),
    }
}

/// One JSON-RPC message in, its answer out (`None` for notifications).
async fn handle_message(shared: &Shared, text: &str) -> Option<Value> {
    jsonrpc::handle_single(text, |method, params| async move {
        match method.as_str() {
            "initialize" => Some(Ok(initialize(&params))),
            "ping" => Some(Ok(json!({}))),
            "tools/list" => Some(Ok(json!({ "tools": tools::definitions() }))),
            "tools/call" => Some(call_tool(shared, &params).await),
            m if m.starts_with("notifications/") => Some(Ok(Value::Null)),
            _ => None,
        }
    })
    .await
}

fn initialize(params: &Value) -> Value {
    let requested = params["protocolVersion"].as_str().unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == requested)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "title": "Kubyl", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

async fn call_tool(shared: &Shared, params: &Value) -> Result<Value, RpcError> {
    let name = params["name"]
        .as_str()
        .ok_or_else(|| RpcError::invalid_params("`name` is missing"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .filter(|a| !a.is_null())
        .unwrap_or_else(|| json!({}));
    let context = shared.context.borrow().clone();
    let output = tools::call(&context, name, &arguments).await;
    if let Some(activity) = &shared.activity {
        activity
            .send(ToolActivity {
                tool: name.to_string(),
                arguments,
                is_error: output.is_error,
            })
            .ok();
    }
    Ok(json!({
        "content": [{ "type": "text", "text": output.text }],
        "isError": output.is_error,
    }))
}

/// `kubyl mcp-bridge`: relays newline-delimited JSON-RPC from stdin to the thread's server and
/// its answers to stdout, until stdin closes. Returns the process exit code.
pub fn bridge() -> i32 {
    let (Ok(url), Ok(token)) = (
        std::env::var(BRIDGE_URL_ENV),
        std::env::var(BRIDGE_TOKEN_ENV),
    ) else {
        eprintln!(
            "kubyl mcp-bridge: started without {BRIDGE_URL_ENV} and {BRIDGE_TOKEN_ENV}; Kubyl starts it for agents."
        );
        return 2;
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("kubyl mcp-bridge: {err}");
            return 1;
        }
    };
    runtime.block_on(relay(url, token, tokio::io::stdin(), tokio::io::stdout()))
}

async fn relay(
    url: String,
    token: String,
    input: impl tokio::io::AsyncRead + Unpin,
    mut output: impl tokio::io::AsyncWrite + Unpin,
) -> i32 {
    use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

    let Some(target) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(authority, path)| (authority.to_string(), format!("/{path}")))
    else {
        eprintln!("kubyl mcp-bridge: unexpected URL");
        return 2;
    };
    let mut lines = tokio::io::BufReader::new(input).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match post(&target.0, &target.1, &token, line.clone()).await {
            Ok(Some(answer)) => {
                if output.write_all(answer.as_bytes()).await.is_err()
                    || output.write_all(b"\n").await.is_err()
                    || output.flush().await.is_err()
                {
                    return 1;
                }
            }
            Ok(None) => {}
            Err(err) => {
                // Answer requests so the agent doesn't wait; Kubyl may have closed the thread.
                if let Ok(message) = serde_json::from_str::<Value>(&line)
                    && let Some(id) = message.get("id").filter(|id| !id.is_null())
                {
                    let answer = json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": { "code": RpcError::INTERNAL_ERROR, "message": format!("Kubyl isn't reachable: {err}") },
                    });
                    output.write_all(answer.to_string().as_bytes()).await.ok();
                    output.write_all(b"\n").await.ok();
                    output.flush().await.ok();
                }
            }
        }
    }
    0
}

/// One POST; the answer body for 200, `None` for 202.
async fn post(
    authority: &str,
    path: &str,
    token: &str,
    body: String,
) -> Result<Option<String>, String> {
    let stream = tokio::net::TcpStream::connect(authority)
        .await
        .map_err(|e| e.to_string())?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        connection.await.ok();
    });
    let request = Request::post(path)
        .header(header::HOST, authority)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| e.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes();
    match status {
        StatusCode::OK if !bytes.is_empty() => {
            Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
        }
        StatusCode::OK | StatusCode::ACCEPTED => Ok(None),
        other => Err(format!("HTTP {}", other.as_u16())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ToolContext {
        ToolContext {
            cluster_name: "kind-dev".into(),
            max_output: 8192,
            ..ToolContext::default()
        }
    }

    async fn raw_post(
        server: &McpServer,
        token: Option<&str>,
        origin: bool,
        body: &str,
    ) -> (StatusCode, String) {
        let stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let mut request = Request::post("/mcp").header(header::HOST, server.addr.to_string());
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if origin {
            request = request.header(header::ORIGIN, "https://evil.example.com");
        }
        let response = sender
            .send_request(
                request
                    .body(Full::new(Bytes::from(body.to_string())))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn speaks_mcp_with_the_token_only() {
        let (_tx, rx) = watch::channel(context());
        let (activity_tx, mut activity) = mpsc::unbounded_channel();
        let server = serve(rx, Some(activity_tx)).await.unwrap();
        let token = server.token().to_string();

        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
        assert_eq!(
            raw_post(&server, None, false, init).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            raw_post(&server, Some("wrong"), false, init).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            raw_post(&server, Some(&token), true, init).await.0,
            StatusCode::FORBIDDEN
        );

        let (status, body) = raw_post(&server, Some(&token), false, init).await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(body["result"]["serverInfo"]["name"], "kubyl");

        let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(
            raw_post(&server, Some(&token), false, note).await.0,
            StatusCode::ACCEPTED
        );

        let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        let body: Value =
            serde_json::from_str(&raw_post(&server, Some(&token), false, list).await.1).unwrap();
        assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 11);

        let call = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cluster_info","arguments":{}}}"#;
        let body: Value =
            serde_json::from_str(&raw_post(&server, Some(&token), false, call).await.1).unwrap();
        assert!(
            body["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("kind-dev")
        );
        assert_eq!(activity.recv().await.unwrap().tool, "cluster_info");
    }

    #[tokio::test]
    async fn the_bridge_relays_stdio() {
        let (_tx, rx) = watch::channel(context());
        let server = serve(rx, None).await.unwrap();
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n"
        );
        let mut output = Vec::new();
        let code = relay(
            server.url(),
            server.token().to_string(),
            input.as_bytes(),
            &mut output,
        )
        .await;
        assert_eq!(code, 0);
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 11);

        // A stopped server still answers requests with an error.
        let url = server.url();
        let token = server.token().to_string();
        drop(server);
        let mut output = Vec::new();
        relay(
            url,
            token,
            r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#.as_bytes(),
            &mut output,
        )
        .await;
        let answer: Value = serde_json::from_slice(output.trim_ascii()).unwrap();
        assert_eq!(answer["id"], 9);
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Kubyl isn't reachable")
        );
    }

    #[test]
    fn tokens_are_random_and_compared_exactly() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(constant_time_eq(a.as_bytes(), a.as_bytes()));
        assert!(!constant_time_eq(a.as_bytes(), b.as_bytes()));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
