//! The ACP client: one agent process, its sessions, and what the agent asks of Kubyl.
//!
//! [`start`] launches the agent, runs `initialize` and returns an [`AgentConnection`] plus a
//! receiver of [`ClientCall`]s: what needs the app (session updates, permission prompts,
//! commands to approve, file access outside the thread's folder). The router answers the rest
//! on Tokio by itself: terminal output, waits, kills and releases, and reads inside the
//! thread's folder.
//!
//! Wire types come from `agent_client_protocol_schema` (ACP v1); requests we send are plain
//! JSON so the schema's non-exhaustive structs don't get in the way.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::v1::{
    AgentCapabilities, AuthMethod, CreateTerminalRequest, Implementation, InitializeResponse,
    NewSessionResponse, PromptResponse, ReadTextFileRequest, RequestPermissionRequest,
    SessionNotification, WriteTextFileRequest,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt as _;
use tokio::sync::mpsc;

use crate::jsonrpc::{self, Incoming, Peer, Responder, RpcError};
use crate::policy;
use crate::terminal::Terminals;

/// How long `initialize` may take (agents started through `npx` download first).
pub const INIT_TIMEOUT: Duration = Duration::from_secs(90);
/// Agent stderr kept for the "Agent output" view and crash messages.
const STDERR_KEEP: usize = 64 * 1024;
/// Largest file the agent reads or writes through Kubyl.
const MAX_FILE: u64 = 8 * 1024 * 1024;

/// How to start an agent.
#[derive(Clone, Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The login shell's `PATH`.
    pub path: Option<OsString>,
    /// `KUBECONFIG` for the agent process (an empty file by default).
    pub kubeconfig: PathBuf,
    pub cwd: PathBuf,
}

/// What `initialize` told us.
#[derive(Clone, Debug)]
pub struct AgentInit {
    pub protocol_version: ProtocolVersion,
    pub capabilities: AgentCapabilities,
    pub auth_methods: Vec<AuthMethod>,
    pub info: Option<Implementation>,
}

impl AgentInit {
    pub fn http_mcp(&self) -> bool {
        self.capabilities.mcp_capabilities.http
    }

    pub fn load_session(&self) -> bool {
        self.capabilities.load_session
    }

    pub fn embedded_context(&self) -> bool {
        self.capabilities.prompt_capabilities.embedded_context
    }
}

/// What a session may touch on this machine.
#[derive(Clone, Debug)]
pub struct SessionScope {
    pub thread: u64,
    pub root: PathBuf,
    pub kubeconfig: PathBuf,
    pub path: Option<OsString>,
}

/// A request or notification the app handles.
#[derive(Debug)]
pub enum ClientCall {
    Update(Box<SessionNotification>),
    Permission {
        request: Box<RequestPermissionRequest>,
        responder: Responder,
    },
    CreateTerminal {
        request: Box<CreateTerminalRequest>,
        responder: Responder,
    },
    /// A read outside the thread's folder.
    ReadFile {
        request: Box<ReadTextFileRequest>,
        responder: Responder,
    },
    /// Every write: `old` is the file's current text, when it exists.
    WriteFile {
        request: Box<WriteTextFileRequest>,
        old: Option<String>,
        responder: Responder,
    },
    /// A question for the user (`elicitation/create`): a form or a URL.
    Elicitation {
        params: Value,
        responder: Responder,
    },
    /// The agent finished the URL question with this id (`elicitation/complete`).
    ElicitationComplete {
        id: String,
    },
    /// A prompt sent with [`AgentConnection::start_prompt`] ended (after all its updates).
    TurnEnded {
        session: String,
        result: Result<PromptResponse, RpcError>,
    },
    /// The agent process ended or closed its stdout.
    Closed {
        stderr_tail: String,
    },
}

#[derive(Default)]
pub(crate) struct Stderr {
    text: String,
}

impl Stderr {
    fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        if self.text.len() > STDERR_KEEP {
            let mut cut = self.text.len() - STDERR_KEEP;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }
}

type Sessions = Arc<Mutex<HashMap<String, SessionScope>>>;
/// Running prompts: request id → session.
type Prompts = Arc<Mutex<HashMap<u64, String>>>;

/// One running agent. Dropping the last handle kills the process and its commands.
pub struct AgentConnection {
    peer: Peer,
    pub init: AgentInit,
    child: Mutex<Option<tokio::process::Child>>,
    stderr: Arc<Mutex<Stderr>>,
    pub terminals: Arc<Terminals>,
    sessions: Sessions,
    prompts: Prompts,
}

impl std::fmt::Debug for AgentConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentConnection")
            .field("protocol_version", &self.init.protocol_version)
            .finish_non_exhaustive()
    }
}

impl Drop for AgentConnection {
    fn drop(&mut self) {
        self.terminals.kill_all();
        if let Some(mut child) = self.child.lock().take() {
            child.start_kill().ok();
        }
    }
}

/// Starts the agent and runs `initialize`. Must run inside a Tokio runtime.
pub async fn start(
    launch: Launch,
) -> Result<(Arc<AgentConnection>, mpsc::UnboundedReceiver<ClientCall>), String> {
    let mut cmd = crate::agents::command(&launch.program);
    cmd.args(&launch.args)
        .current_dir(&launch.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("KUBECONFIG", &launch.kubeconfig);
    if let Some(path) = &launch.path {
        cmd.env("PATH", path);
    }
    for (name, value) in &launch.env {
        cmd.env(name, value);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{} didn't start: {e}", launch.program.display()))?;
    let (Some(stdin), Some(stdout), Some(stderr_pipe)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err("The agent's stdio isn't available.".into());
    };
    let stderr = Arc::new(Mutex::new(Stderr::default()));
    tokio::spawn(pump_stderr(stderr_pipe, stderr.clone()));

    // On failure `child` drops here, which kills it.
    let (connection, calls) = connect(stdout, stdin, stderr.clone()).await?;
    *connection.child.lock() = Some(child);
    Ok((Arc::new(connection), calls))
}

/// Runs `initialize` over an already connected transport (the process, or a test's pipes).
pub(crate) async fn connect(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    writer: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    stderr: Arc<Mutex<Stderr>>,
) -> Result<(AgentConnection, mpsc::UnboundedReceiver<ClientCall>), String> {
    let (peer, incoming) = jsonrpc::connect(reader, writer);
    let terminals = Arc::new(Terminals::default());
    let sessions: Sessions = Arc::default();
    let prompts: Prompts = Arc::default();
    let (calls_tx, calls_rx) = mpsc::unbounded_channel();
    tokio::spawn(route(
        incoming,
        calls_tx,
        terminals.clone(),
        sessions.clone(),
        prompts.clone(),
        stderr.clone(),
    ));

    let params = json!({
        "protocolVersion": ProtocolVersion::V1,
        "clientCapabilities": {
            "fs": { "readTextFile": true, "writeTextFile": true },
            "terminal": true,
            "elicitation": { "form": {}, "url": {} },
            "session": { "configOptions": { "boolean": {} } },
        },
        "clientInfo": { "name": "kubyl", "title": "Kubyl", "version": env!("CARGO_PKG_VERSION") },
    });
    let answer = tokio::time::timeout(
        INIT_TIMEOUT,
        peer.request::<InitializeResponse>("initialize", params),
    )
    .await;
    let response = match answer {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => {
            return Err(with_stderr(
                format!("The agent didn't start a session: {}", err.message),
                &stderr,
            ));
        }
        Err(_) => {
            return Err(with_stderr(
                "The agent didn't answer within 90 seconds.".into(),
                &stderr,
            ));
        }
    };
    if response.protocol_version != ProtocolVersion::V1 {
        return Err(format!(
            "The agent speaks ACP version {:?}; Kubyl speaks version 1. Update the agent or Kubyl.",
            response.protocol_version
        ));
    }
    let init = AgentInit {
        protocol_version: response.protocol_version,
        capabilities: response.agent_capabilities,
        auth_methods: response.auth_methods,
        info: response.agent_info,
    };
    Ok((
        AgentConnection {
            peer,
            init,
            child: Mutex::new(None),
            stderr,
            terminals,
            sessions,
            prompts,
        },
        calls_rx,
    ))
}

fn with_stderr(message: String, stderr: &Mutex<Stderr>) -> String {
    let tail = tail_lines(&stderr.lock().text, 6);
    if tail.is_empty() {
        message
    } else {
        format!("{message}\n{tail}")
    }
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    kubyl_resources_core::redact::scrub_text(&lines[start..].join("\n")).into_owned()
}

async fn pump_stderr(mut pipe: tokio::process::ChildStderr, stderr: Arc<Mutex<Stderr>>) {
    let mut buf = vec![0u8; 4096];
    while let Ok(n) = pipe.read(&mut buf).await {
        if n == 0 {
            break;
        }
        stderr.lock().push(&String::from_utf8_lossy(&buf[..n]));
    }
}

impl AgentConnection {
    /// The agent's stderr so far, scrubbed of token shapes.
    pub fn stderr(&self) -> String {
        kubyl_resources_core::redact::scrub_text(&self.stderr.lock().text).into_owned()
    }

    pub fn is_open(&self) -> bool {
        self.peer.is_open()
    }

    pub async fn authenticate(&self, method: &str) -> Result<(), RpcError> {
        self.peer
            .request_value("authenticate", json!({ "methodId": method }))
            .await
            .map(|_| ())
    }

    /// Starts a session in `scope.root` with Kubyl's MCP server (`mcp`, an ACP `McpServer`).
    pub async fn new_session(
        &self,
        scope: SessionScope,
        mcp: Value,
    ) -> Result<NewSessionResponse, RpcError> {
        let response: NewSessionResponse = self
            .peer
            .request(
                "session/new",
                json!({ "cwd": scope.root, "mcpServers": [mcp] }),
            )
            .await?;
        self.sessions
            .lock()
            .insert(response.session_id.0.to_string(), scope);
        Ok(response)
    }

    /// Reopens a session the agent stored (`loadSession`). Its history arrives as updates.
    pub async fn load_session(
        &self,
        session: &str,
        scope: SessionScope,
        mcp: Value,
    ) -> Result<Value, RpcError> {
        self.sessions
            .lock()
            .insert(session.to_string(), scope.clone());
        let result = self
            .peer
            .request_value(
                "session/load",
                json!({ "sessionId": session, "cwd": scope.root, "mcpServers": [mcp] }),
            )
            .await;
        if result.is_err() {
            self.sessions.lock().remove(session);
        }
        result
    }

    pub async fn prompt(
        &self,
        session: &str,
        prompt: Vec<Value>,
    ) -> Result<PromptResponse, RpcError> {
        self.peer
            .request(
                "session/prompt",
                json!({ "sessionId": session, "prompt": prompt }),
            )
            .await
    }

    /// Sends a prompt; its end arrives as [`ClientCall::TurnEnded`] after its last update.
    pub fn start_prompt(&self, session: &str, prompt: Vec<Value>) -> Result<(), RpcError> {
        let mut prompts = self.prompts.lock();
        let id = self.peer.send_ordered(
            "session/prompt",
            json!({ "sessionId": session, "prompt": prompt }),
        )?;
        prompts.insert(id, session.to_string());
        Ok(())
    }

    pub fn cancel(&self, session: &str) {
        self.peer
            .notify("session/cancel", json!({ "sessionId": session }));
    }

    pub async fn set_mode(&self, session: &str, mode: &str) -> Result<(), RpcError> {
        self.peer
            .request_value(
                "session/set_mode",
                json!({ "sessionId": session, "modeId": mode }),
            )
            .await
            .map(|_| ())
    }

    /// Changes a session setting (`session/set_config_option`). The answer holds all of the
    /// session's settings (`configOptions`).
    pub async fn set_config_option(
        &self,
        session: &str,
        config: &str,
        value: &crate::thread::ConfigValue,
    ) -> Result<Value, RpcError> {
        let mut params = json!({ "sessionId": session, "configId": config });
        match value {
            crate::thread::ConfigValue::Select { current, .. } => params["value"] = json!(current),
            crate::thread::ConfigValue::Bool(on) => {
                params["type"] = json!("boolean");
                params["value"] = json!(on);
            }
        }
        self.peer
            .request_value("session/set_config_option", params)
            .await
    }

    /// Forgets a session's scope (its commands are killed with the thread).
    pub fn close_session(&self, session: &str) {
        self.sessions.lock().remove(session);
    }

    pub fn scope(&self, session: &str) -> Option<SessionScope> {
        self.sessions.lock().get(session).cloned()
    }
}

/// Answers what can be answered on Tokio; forwards the rest.
async fn route(
    mut incoming: mpsc::UnboundedReceiver<Incoming>,
    calls: mpsc::UnboundedSender<ClientCall>,
    terminals: Arc<Terminals>,
    sessions: Sessions,
    prompts: Prompts,
    stderr: Arc<Mutex<Stderr>>,
) {
    while let Some(message) = incoming.recv().await {
        match message {
            Incoming::Response { id, result } => {
                // `start_prompt` holds the lock while sending, so the id is in the map.
                let Some(session) = prompts.lock().remove(&id) else {
                    continue;
                };
                let result = result.and_then(|value| {
                    serde_json::from_value::<PromptResponse>(value)
                        .map_err(|e| RpcError::internal(format!("Unexpected prompt answer: {e}")))
                });
                calls.send(ClientCall::TurnEnded { session, result }).ok();
            }
            Incoming::Notification { method, params } => {
                if method == "elicitation/complete" {
                    let id = params["elicitationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    calls.send(ClientCall::ElicitationComplete { id }).ok();
                } else if method == "session/update"
                    && let Ok(update) = serde_json::from_value::<SessionNotification>(params)
                {
                    calls.send(ClientCall::Update(Box::new(update))).ok();
                }
            }
            Incoming::Request {
                method,
                params,
                responder,
            } => match method.as_str() {
                "session/request_permission" => match serde_json::from_value(params) {
                    Ok(request) => {
                        calls
                            .send(ClientCall::Permission {
                                request: Box::new(request),
                                responder,
                            })
                            .ok();
                    }
                    Err(err) => {
                        responder.err(RpcError::invalid_params(err));
                    }
                },
                "elicitation/create" => {
                    calls
                        .send(ClientCall::Elicitation { params, responder })
                        .ok();
                }
                "terminal/create" => match serde_json::from_value(params) {
                    Ok(request) => {
                        calls
                            .send(ClientCall::CreateTerminal {
                                request: Box::new(request),
                                responder,
                            })
                            .ok();
                    }
                    Err(err) => {
                        responder.err(RpcError::invalid_params(err));
                    }
                },
                "terminal/output" => {
                    let Some(terminal) = terminal_of(&terminals, &params) else {
                        responder.err(no_terminal());
                        continue;
                    };
                    let (output, truncated, exit) = terminal.snapshot();
                    responder.ok(json!({
                        "output": output,
                        "truncated": truncated,
                        "exitStatus": exit.map(|e| json!({ "exitCode": e.code, "signal": e.signal })),
                    }));
                }
                "terminal/wait_for_exit" => {
                    let Some(terminal) = terminal_of(&terminals, &params) else {
                        responder.err(no_terminal());
                        continue;
                    };
                    tokio::spawn(async move {
                        let exit = terminal.wait().await;
                        responder.ok(json!({ "exitCode": exit.code, "signal": exit.signal }));
                    });
                }
                "terminal/kill" => {
                    let Some(terminal) = terminal_of(&terminals, &params) else {
                        responder.err(no_terminal());
                        continue;
                    };
                    terminal.kill();
                    responder.ok(json!({}));
                }
                "terminal/release" => {
                    let id = params["terminalId"].as_str().unwrap_or_default();
                    terminals.release(id);
                    responder.ok(json!({}));
                }
                "fs/read_text_file" => {
                    let request: ReadTextFileRequest = match serde_json::from_value(params) {
                        Ok(request) => request,
                        Err(err) => {
                            responder.err(RpcError::invalid_params(err));
                            continue;
                        }
                    };
                    let scope = sessions.lock().get(request.session_id.0.as_ref()).cloned();
                    match scope {
                        Some(scope) if policy::inside(&scope.root, &request.path) => {
                            tokio::spawn(async move {
                                responder.respond(read_text(&request).await);
                            });
                        }
                        _ => {
                            calls
                                .send(ClientCall::ReadFile {
                                    request: Box::new(request),
                                    responder,
                                })
                                .ok();
                        }
                    }
                }
                "fs/write_text_file" => {
                    let request: WriteTextFileRequest = match serde_json::from_value(params) {
                        Ok(request) => request,
                        Err(err) => {
                            responder.err(RpcError::invalid_params(err));
                            continue;
                        }
                    };
                    if !request.path.is_absolute() || request.content.len() as u64 > MAX_FILE {
                        responder.err(RpcError::invalid_params("absolute path and at most 8 MiB"));
                        continue;
                    }
                    let calls = calls.clone();
                    tokio::spawn(async move {
                        let old = match tokio::fs::metadata(&request.path).await {
                            Ok(meta) if meta.len() <= MAX_FILE => {
                                tokio::fs::read_to_string(&request.path).await.ok()
                            }
                            _ => None,
                        };
                        calls
                            .send(ClientCall::WriteFile {
                                request: Box::new(request),
                                old,
                                responder,
                            })
                            .ok();
                    });
                }
                other => {
                    responder.err(RpcError::method_not_found(other));
                }
            },
        }
    }
    terminals.kill_all();
    let stderr_tail = tail_lines(&stderr.lock().text, 8);
    calls.send(ClientCall::Closed { stderr_tail }).ok();
}

fn terminal_of(
    terminals: &Terminals,
    params: &Value,
) -> Option<Arc<crate::terminal::LocalTerminal>> {
    terminals.get(params["terminalId"].as_str()?)
}

fn no_terminal() -> RpcError {
    RpcError::new(RpcError::RESOURCE_NOT_FOUND, "No such terminal")
}

/// A text file, optionally from `line` (1-based) for `limit` lines.
pub async fn read_text(request: &ReadTextFileRequest) -> Result<Value, RpcError> {
    read_path(&request.path, request.line, request.limit).await
}

pub async fn read_path(
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> Result<Value, RpcError> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| {
        RpcError::new(
            RpcError::RESOURCE_NOT_FOUND,
            format!("{}: {e}", path.display()),
        )
    })?;
    if meta.len() > MAX_FILE {
        return Err(RpcError::invalid_params("The file is larger than 8 MiB"));
    }
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| RpcError::internal(format!("{}: {e}", path.display())))?;
    let content = match (line, limit) {
        (None, None) => text,
        (line, limit) => {
            let skip = line.unwrap_or(1).saturating_sub(1) as usize;
            let take = limit.map(|l| l as usize).unwrap_or(usize::MAX);
            text.split_inclusive('\n').skip(skip).take(take).collect()
        }
    };
    Ok(json!({ "content": content }))
}

/// Writes `content` to `path`, creating its folder.
pub async fn write_text(path: &Path, content: &str) -> Result<Value, RpcError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| RpcError::internal(format!("{}: {e}", parent.display())))?;
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|e| RpcError::internal(format!("{}: {e}", path.display())))?;
    Ok(Value::Null)
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted ACP agent on in-memory pipes, for tests of the client and the service.

    use super::*;

    /// What the fake agent does on each `session/prompt`.
    #[derive(Clone, Debug)]
    pub enum Step {
        /// `session/update` with `agent_message_chunk`.
        Say(String),
        /// A tool call, a permission request for it, then its completion.
        AskPermission { title: String, command: String },
        /// `terminal/create` + `terminal/wait_for_exit` + `terminal/output`; says the output.
        Run(String),
        /// `fs/write_text_file`.
        Write { path: PathBuf, content: String },
        /// `fs/read_text_file`; says the content or the error.
        Read(PathBuf),
        /// `elicitation/create` with these params (the session id is added); says the answer.
        Ask(Value),
        /// `usage_update` with these tokens used of the window size.
        Usage(u64, u64),
        /// Calls a Kubyl MCP tool over HTTP (url and token from the session's MCP entry).
        Tool { name: String, arguments: Value },
    }

    /// Starts a fake agent and connects a client to it.
    pub async fn start(
        script: Vec<Step>,
        http_mcp: bool,
    ) -> (Arc<AgentConnection>, mpsc::UnboundedReceiver<ClientCall>) {
        let (client_side, agent_side) = tokio::io::duplex(1 << 20);
        let (agent_read, agent_write) = tokio::io::split(agent_side);
        let (peer, mut incoming) = jsonrpc::connect(agent_read, agent_write);
        tokio::spawn(async move {
            let mut mcp: HashMap<String, Value> = HashMap::new();
            let mut sessions = 0;
            while let Some(message) = incoming.recv().await {
                let Incoming::Request {
                    method,
                    params,
                    responder,
                } = message
                else {
                    continue;
                };
                match method.as_str() {
                    "initialize" => {
                        responder.ok(json!({
                            "protocolVersion": 1,
                            "agentCapabilities": {
                                "loadSession": true,
                                "mcpCapabilities": { "http": http_mcp },
                                "promptCapabilities": { "embeddedContext": true },
                            },
                            "authMethods": [{ "id": "login", "name": "Log in", "description": "Run `fake login`" }],
                            "agentInfo": { "name": "fake", "version": "1.0.0" },
                        }));
                    }
                    "session/new" => {
                        sessions += 1;
                        let id = format!("s{sessions}");
                        mcp.insert(id.clone(), params["mcpServers"][0].clone());
                        responder.ok(json!({
                            "sessionId": id,
                            "configOptions": fake_config("opus", "medium"),
                            "modes": {
                                "currentModeId": "default",
                                "availableModes": [
                                    { "id": "default", "name": "Ask" },
                                    { "id": "plan", "name": "Plan" },
                                ],
                            },
                        }));
                    }
                    "session/set_config_option" => {
                        let (model, effort) = match params["configId"].as_str() {
                            Some("model") => (params["value"].as_str().unwrap_or("opus"), "medium"),
                            _ => ("opus", params["value"].as_str().unwrap_or("medium")),
                        };
                        responder.ok(json!({ "configOptions": fake_config(model, effort) }));
                    }
                    "session/set_mode" => {
                        let session = params["sessionId"].clone();
                        let mode = params["modeId"].clone();
                        responder.ok(json!({}));
                        peer.notify("session/update", json!({
                            "sessionId": session,
                            "update": { "sessionUpdate": "current_mode_update", "currentModeId": mode },
                        }));
                    }
                    "session/prompt" => {
                        let session = params["sessionId"].as_str().unwrap_or_default().to_string();
                        let entry = mcp.get(&session).cloned().unwrap_or(Value::Null);
                        let peer = peer.clone();
                        let script = script.clone();
                        tokio::spawn(async move {
                            for step in script {
                                run_step(&peer, &session, &entry, step).await;
                            }
                            responder.ok(json!({ "stopReason": "end_turn" }));
                        });
                    }
                    _ => {
                        responder.err(RpcError::method_not_found(&method));
                    }
                }
            }
        });
        let (client_read, client_write) = tokio::io::split(client_side);
        let (connection, calls) = connect(client_read, client_write, Arc::default())
            .await
            .expect("fake agent initializes");
        (Arc::new(connection), calls)
    }

    fn fake_config(model: &str, effort: &str) -> Value {
        json!([
            { "id": "model", "name": "Model", "category": "model", "type": "select",
              "currentValue": model, "options": [
                { "value": "opus", "name": "Opus" }, { "value": "sonnet", "name": "Sonnet" } ] },
            { "id": "effort", "name": "Thinking", "category": "thought_level", "type": "select",
              "currentValue": effort, "options": [
                { "value": "low", "name": "Low" }, { "value": "medium", "name": "Medium" },
                { "value": "high", "name": "High" } ] },
        ])
    }

    fn say(peer: &Peer, session: &str, text: &str) {
        peer.notify(
            "session/update",
            json!({
                "sessionId": session,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text },
                },
            }),
        );
    }

    async fn run_step(peer: &Peer, session: &str, mcp: &Value, step: Step) {
        match step {
            Step::Say(text) => say(peer, session, &text),
            Step::AskPermission { title, command } => {
                peer.notify("session/update", json!({
                    "sessionId": session,
                    "update": {
                        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": title,
                        "kind": "execute", "status": "pending", "rawInput": { "command": command },
                    },
                }));
                let answer = peer
                    .request_value(
                        "session/request_permission",
                        json!({
                            "sessionId": session,
                            "toolCall": { "toolCallId": "t1", "title": title, "kind": "execute", "rawInput": { "command": command } },
                            "options": [
                                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                                { "optionId": "reject", "name": "Reject", "kind": "reject_once" },
                            ],
                        }),
                    )
                    .await;
                let outcome = answer
                    .ok()
                    .and_then(|a| a["outcome"]["optionId"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "cancelled".into());
                say(peer, session, &format!("permission: {outcome}"));
            }
            Step::Run(command) => {
                let created = peer
                    .request_value(
                        "terminal/create",
                        json!({ "sessionId": session, "command": command, "args": [] }),
                    )
                    .await;
                let id = match created {
                    Ok(created) => created["terminalId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    Err(err) => {
                        say(peer, session, &format!("terminal refused: {}", err.message));
                        return;
                    }
                };
                let exit = peer
                    .request_value(
                        "terminal/wait_for_exit",
                        json!({ "sessionId": session, "terminalId": id }),
                    )
                    .await
                    .unwrap_or_default();
                let output = peer
                    .request_value(
                        "terminal/output",
                        json!({ "sessionId": session, "terminalId": id }),
                    )
                    .await
                    .unwrap_or_default();
                peer.request_value(
                    "terminal/release",
                    json!({ "sessionId": session, "terminalId": id }),
                )
                .await
                .ok();
                say(
                    peer,
                    session,
                    &format!(
                        "exit {} output {}",
                        exit["exitCode"],
                        output["output"].as_str().unwrap_or_default().trim()
                    ),
                );
            }
            Step::Write { path, content } => {
                let result = peer
                    .request_value(
                        "fs/write_text_file",
                        json!({ "sessionId": session, "path": path, "content": content }),
                    )
                    .await;
                say(peer, session, &format!("write: {}", result.is_ok()));
            }
            Step::Read(path) => {
                let result = peer
                    .request_value(
                        "fs/read_text_file",
                        json!({ "sessionId": session, "path": path }),
                    )
                    .await;
                match result {
                    Ok(value) => say(
                        peer,
                        session,
                        &format!("read: {}", value["content"].as_str().unwrap_or_default()),
                    ),
                    Err(err) => say(peer, session, &format!("read refused: {}", err.message)),
                }
            }
            Step::Usage(used, size) => {
                peer.notify(
                    "session/update",
                    json!({
                        "sessionId": session,
                        "update": { "sessionUpdate": "usage_update", "used": used, "size": size },
                    }),
                );
            }
            Step::Ask(mut params) => {
                params["sessionId"] = json!(session);
                let answer = peer.request_value("elicitation/create", params).await;
                match answer {
                    Ok(answer) => say(peer, session, &format!("answer: {answer}")),
                    Err(err) => say(peer, session, &format!("ask failed: {}", err.message)),
                }
            }
            Step::Tool { name, arguments } => {
                let text = call_mcp_tool(mcp, &name, arguments).await;
                say(peer, session, &format!("tool {name}: {text}"));
            }
        }
    }

    /// Calls a tool on the MCP server named in `session/new` (HTTP entries only).
    async fn call_mcp_tool(entry: &Value, name: &str, arguments: Value) -> String {
        use http_body_util::{BodyExt as _, Full};
        use hyper::body::Bytes;
        use hyper_util::rt::TokioIo;

        let url = entry["url"].as_str().unwrap_or_default();
        let auth = entry["headers"][0]["value"].as_str().unwrap_or_default();
        let Some((authority, _)) = url.trim_start_matches("http://").split_once('/') else {
            return "no http entry".into();
        };
        let Ok(stream) = tokio::net::TcpStream::connect(authority).await else {
            return "unreachable".into();
        };
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
        let request = http::Request::post("/mcp")
            .header("host", authority)
            .header("authorization", auth)
            .body(Full::new(Bytes::from(body.to_string())))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        value["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{self, Step};
    use super::*;

    fn scope(root: &Path) -> SessionScope {
        SessionScope {
            thread: 1,
            root: root.to_path_buf(),
            kubeconfig: root.join("kubeconfig"),
            path: None,
        }
    }

    #[tokio::test]
    async fn initialize_session_and_prompt() {
        let (agent, mut calls) = fake::start(vec![Step::Say("hello".into())], true).await;
        assert!(agent.init.http_mcp());
        assert!(agent.init.load_session());
        assert_eq!(agent.init.auth_methods.len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let session = agent
            .new_session(scope(dir.path()), json!({}))
            .await
            .unwrap();
        assert_eq!(session.session_id.0.as_ref(), "s1");
        let stop = agent
            .prompt("s1", vec![json!({"type": "text", "text": "hi"})])
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(stop.stop_reason).unwrap(),
            json!("end_turn")
        );
        match calls.recv().await.unwrap() {
            ClientCall::Update(update) => assert_eq!(update.session_id.0.as_ref(), "s1"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn reads_inside_the_folder_are_answered_and_others_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "inside\n").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let (agent, mut calls) = fake::start(
            vec![
                Step::Read(dir.path().join("a.txt")),
                Step::Read(outside.path().to_path_buf()),
            ],
            true,
        )
        .await;
        agent
            .new_session(scope(dir.path()), json!({}))
            .await
            .unwrap();
        let prompt = {
            let agent = agent.clone();
            tokio::spawn(async move { agent.prompt("s1", vec![]).await })
        };
        let mut said = Vec::new();
        while said.len() < 2 {
            match calls.recv().await.unwrap() {
                ClientCall::Update(update) => said.push(format!("{:?}", update.update)),
                ClientCall::ReadFile { responder, .. } => {
                    responder.err(RpcError::internal("declined"));
                }
                other => panic!("{other:?}"),
            }
        }
        prompt.await.unwrap().unwrap();
        assert!(said[0].contains("read: inside"), "{said:?}");
        assert!(said[1].contains("read refused: declined"), "{said:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn approved_commands_run_and_report_output() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, mut calls) =
            fake::start(vec![Step::Run("echo kubyl-$((40+2))".into())], true).await;
        agent
            .new_session(scope(dir.path()), json!({}))
            .await
            .unwrap();
        let prompt = {
            let agent = agent.clone();
            tokio::spawn(async move { agent.prompt("s1", vec![]).await })
        };
        loop {
            match calls.recv().await.unwrap() {
                ClientCall::CreateTerminal { request, responder } => {
                    let scope = agent.scope(request.session_id.0.as_ref()).unwrap();
                    let (id, _) = agent
                        .terminals
                        .spawn(crate::terminal::Launch {
                            command: request.command.clone(),
                            args: request.args.clone(),
                            env: Vec::new(),
                            cwd: scope.root,
                            output_limit: None,
                            path: None,
                            kubeconfig: Some(scope.kubeconfig),
                        })
                        .unwrap();
                    responder.ok(json!({ "terminalId": id }));
                }
                ClientCall::Update(update) => {
                    let text = format!("{:?}", update.update);
                    assert!(text.contains("exit 0 output kubyl-42"), "{text}");
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        prompt.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn closing_the_agent_fails_requests_and_reports() {
        let (client_side, agent_side) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client_side);
        let starting = tokio::spawn(connect(read, write, Arc::default()));
        drop(agent_side);
        assert!(starting.await.unwrap().is_err());
    }
}
