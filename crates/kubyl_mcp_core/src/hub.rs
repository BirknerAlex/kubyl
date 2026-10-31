//! [`McpHub`]: Kubyl's own MCP server for outside clients (Claude Code, Cursor, …).
//!
//! On by default, on `127.0.0.1:7421` ([`crate::settings::McpSettings`]). It gives a client every
//! cluster Kubyl is connected to, through the same tools agent threads get, with the same
//! masking and the user's own RBAC. The bearer token is made once and kept in the OS keychain
//! (clients are configured with it, so it must survive restarts); "Regenerate" replaces it.
//! Change tools are off unless `mcp.writes` is `ask`, and then every change waits for the user's
//! click in Kubyl's window ([`McpHub::pending`], [`McpHub::answer`]).

use std::sync::Arc;

use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_kube_core::auth::Credentials;
use secrecy::{ExposeSecret as _, SecretString};
use tokio::sync::{oneshot, watch};

use crate::approval::{ApprovalRequest, Approver, ChannelApprover, Decision, Proposal};
use crate::handler::{Scope, ToolsHandler, Writes};
use crate::server::{self, McpServer, ServeOptions};
use crate::settings::{ExternalWrites, McpSettings};
use crate::tools::ToolContext;

/// Where the token lives in the credential store.
const TOKEN_KEY: &str = "mcp/server-token";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    /// Turned off in settings.
    Off,
    Starting,
    Running {
        port: u16,
    },
    /// It couldn't start; the text says why and what to do.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubEvent {
    Changed,
}

/// A change from an outside client waiting for the user.
pub struct ExternalApproval {
    pub id: u64,
    pub proposal: Proposal,
    reply: Option<oneshot::Sender<Decision>>,
}

impl std::fmt::Debug for ExternalApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalApproval")
            .field("id", &self.id)
            .field("title", &self.proposal.title)
            .finish_non_exhaustive()
    }
}

pub struct McpHub {
    settings: McpSettings,
    credentials: Credentials,
    state: ServerState,
    server: Option<McpServer>,
    token: Option<SecretString>,
    clusters: watch::Sender<Arc<Vec<ToolContext>>>,
    writes: watch::Sender<bool>,
    approver: Arc<ChannelApprover>,
    requests: Option<tokio::sync::mpsc::UnboundedReceiver<ApprovalRequest>>,
    pending: Vec<ExternalApproval>,
    next_id: u64,
    starting: Option<TaskHandle>,
    listening: Option<TaskHandle>,
}

impl Service for McpHub {
    type Event = HubEvent;
    type Effect = ();
}

impl McpHub {
    pub fn new(settings: McpSettings, credentials: Credentials) -> Self {
        let (approver, requests) = ChannelApprover::new();
        Self {
            writes: watch::channel(settings.writes == ExternalWrites::Ask).0,
            settings,
            credentials,
            state: ServerState::Off,
            server: None,
            token: None,
            clusters: watch::channel(Arc::new(Vec::new())).0,
            approver,
            requests: Some(requests),
            pending: Vec::new(),
            next_id: 0,
            starting: None,
            listening: None,
        }
    }

    pub fn settings(&self) -> &McpSettings {
        &self.settings
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// `http://127.0.0.1:<port>/mcp` while the server runs.
    pub fn url(&self) -> Option<String> {
        match self.state {
            ServerState::Running { port } => Some(format!("http://127.0.0.1:{port}/mcp")),
            _ => None,
        }
    }

    /// The bearer token, for the explicit "copy" actions only. Never log it.
    pub fn token(&self) -> Option<&str> {
        self.token.as_ref().map(|t| t.expose_secret())
    }

    /// `claude mcp add …` with the token in it: the user pastes it into a terminal.
    pub fn claude_code_command(&self) -> Option<String> {
        Some(format!(
            "claude mcp add --transport http kubyl {} --header \"Authorization: Bearer {}\"",
            self.url()?,
            self.token()?
        ))
    }

    /// A client configuration in the `mcpServers` JSON form (Cursor, Claude Desktop with a
    /// remote-capable bridge, …), with the token in it.
    pub fn json_config(&self) -> Option<String> {
        let config = serde_json::json!({
            "mcpServers": {
                "kubyl": {
                    "type": "http",
                    "url": self.url()?,
                    "headers": { "Authorization": format!("Bearer {}", self.token()?) },
                }
            }
        });
        serde_json::to_string_pretty(&config).ok()
    }

    pub fn pending(&self) -> &[ExternalApproval] {
        &self.pending
    }

    /// Starts listening for change requests and the server if enabled.
    pub fn start(&mut self, host: &mut dyn Host<Self>) {
        if let Some(requests) = self.requests.take() {
            let stream = futures::stream::unfold(requests, |mut requests| async move {
                requests.recv().await.map(|request| (request, requests))
            });
            self.listening = Some(host.batches(stream, Pace::IMMEDIATE, |this, batch, host| {
                for request in batch {
                    this.requested(request, host);
                }
                Flow::Continue
            }));
        }
        self.restart(host);
    }

    /// The clusters the tools reach. Cheap: only publishes to the running server.
    pub fn set_clusters(&mut self, clusters: Vec<ToolContext>) {
        let output = self.settings.max_tool_output();
        let clusters = clusters
            .into_iter()
            .map(|mut c| {
                c.max_output = output;
                c
            })
            .collect();
        self.clusters.send_replace(Arc::new(clusters));
    }

    pub fn settings_changed(&mut self, settings: McpSettings, host: &mut dyn Host<Self>) {
        if settings == self.settings {
            return;
        }
        let restart = settings.enabled != self.settings.enabled
            || settings.port() != self.settings.port()
            || settings.max_tool_output() != self.settings.max_tool_output();
        self.writes
            .send_replace(settings.writes == ExternalWrites::Ask);
        if settings.writes == ExternalWrites::Off {
            // Nothing may wait for approval once changes are off.
            self.pending.clear();
        }
        self.settings = settings;
        if restart {
            self.restart(host);
        }
        host.notify();
    }

    /// Replaces the token (existing clients stop working until reconfigured).
    pub fn regenerate_token(&mut self, host: &mut dyn Host<Self>) {
        let credentials = self.credentials.clone();
        self.server = None;
        self.token = None;
        self.state = ServerState::Starting;
        host.notify();
        let work = async move {
            tokio::task::spawn_blocking(move || {
                credentials.delete(TOKEN_KEY).ok();
            })
            .await
            .ok();
        };
        self.starting = Some(host.spawn(work, |this, (), host| this.restart(host)));
    }

    fn restart(&mut self, host: &mut dyn Host<Self>) {
        self.server = None;
        self.starting = None;
        if !self.settings.enabled {
            self.state = ServerState::Off;
            host.notify();
            return;
        }
        self.state = ServerState::Starting;
        host.notify();
        let port = self.settings.port();
        let credentials = self.credentials.clone();
        let handler = Arc::new(ToolsHandler::new(
            Scope::Clusters(self.clusters.subscribe()),
            Some(Writes {
                enabled: self.writes.subscribe(),
                approver: self.approver.clone() as Arc<dyn Approver>,
                off_message: "Kubyl's MCP server is read-only. The user can allow changes (each one approved in Kubyl) with \"writes\": \"ask\" under \"mcp\" in Kubyl's settings.json.".into(),
            }),
        ));
        let work = async move {
            let token = tokio::task::spawn_blocking(move || token(&credentials))
                .await
                .map_err(|e| e.to_string())??;
            let server = server::serve(
                handler,
                ServeOptions {
                    port,
                    token: Some(token.expose_secret().to_string()),
                    activity: None,
                },
            )
            .await
            .map_err(|err| start_error(port, &err))?;
            Ok::<_, String>((server, token))
        };
        self.starting = Some(host.spawn(work, |this, result, host| {
            match result {
                Ok((server, token)) => {
                    this.state = ServerState::Running {
                        port: server.addr.port(),
                    };
                    this.server = Some(server);
                    this.token = Some(token);
                }
                Err(message) => this.state = ServerState::Failed(message),
            }
            host.emit(HubEvent::Changed);
            host.notify();
        }));
    }

    fn requested(&mut self, request: ApprovalRequest, host: &mut dyn Host<Self>) {
        self.next_id += 1;
        self.pending.push(ExternalApproval {
            id: self.next_id,
            proposal: request.proposal,
            reply: Some(request.reply),
        });
        host.emit(HubEvent::Changed);
        host.notify();
    }

    /// The user's answer to a waiting change.
    pub fn answer(&mut self, id: u64, decision: Decision, host: &mut dyn Host<Self>) {
        let Some(index) = self.pending.iter().position(|p| p.id == id) else {
            return;
        };
        let mut item = self.pending.remove(index);
        if let Some(reply) = item.reply.take() {
            reply.send(decision).ok();
        }
        host.emit(HubEvent::Changed);
        host.notify();
    }
}

/// The saved token, or a new one saved now.
fn token(credentials: &Credentials) -> Result<SecretString, String> {
    match credentials.get(TOKEN_KEY) {
        Ok(Some(token)) if !token.expose_secret().is_empty() => return Ok(token),
        Ok(_) => {}
        Err(err) => return Err(format!("Kubyl's credential store isn't available: {err}")),
    }
    let token = SecretString::from(server::new_token().map_err(|e| e.to_string())?);
    credentials
        .set(TOKEN_KEY, &token)
        .map_err(|err| format!("The MCP token couldn't be saved: {err}"))?;
    Ok(token)
}

fn start_error(port: u16, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::AddrInUse {
        format!(
            "Port {port} is in use by another program. Set a free port as \"port\" under \"mcp\" in Kubyl's settings.json."
        )
    } else {
        format!("The MCP server couldn't listen on 127.0.0.1:{port}: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_base::ClusterId;
    use kubyl_base::host::TestHost;
    use serde_json::{Value, json};

    /// A credential handle backed by the in-memory store, so tests stay off the keychain.
    fn scoped(scope: &str) -> Credentials {
        static MEMORY: std::sync::Once = std::sync::Once::new();
        MEMORY.call_once(|| {
            // SAFETY: set before the first credential access of this test process, which
            // happens only through this helper.
            unsafe { std::env::set_var("KUBYL_CREDENTIAL_STORE", "memory") };
        });
        Credentials::scoped(scope)
    }

    fn credentials() -> Credentials {
        scoped("mcp-hub-tests")
    }

    async fn post(url: &str, token: &str, body: Value) -> Value {
        use http_body_util::BodyExt as _;
        let authority = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let stream = tokio::net::TcpStream::connect(authority).await.unwrap();
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream))
                .await
                .unwrap();
        tokio::spawn(connection);
        let request = http::Request::post("/mcp")
            .header(http::header::HOST, authority)
            .header(http::header::AUTHORIZATION, format!("Bearer {token}"))
            .body(http_body_util::Full::new(hyper::body::Bytes::from(
                body.to_string(),
            )))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn run_started(hub: &mut McpHub, host: &mut TestHost<McpHub>) {
        host.run_until(hub, |hub, _| !matches!(hub.state(), ServerState::Starting));
    }

    #[test]
    fn it_starts_on_the_configured_port_and_serves_the_clusters() {
        let port = free_port();
        let settings = McpSettings {
            port,
            ..McpSettings::default()
        };
        let mut hub = McpHub::new(settings, credentials());
        let mut host = TestHost::new();
        hub.set_clusters(vec![ToolContext {
            cluster: Some(ClusterId::new("kind-dev")),
            cluster_name: "kind-dev".into(),
            ..ToolContext::default()
        }]);
        hub.start(&mut host);
        run_started(&mut hub, &mut host);
        assert_eq!(hub.state(), &ServerState::Running { port });
        let url = hub.url().unwrap();
        let token = hub.token().unwrap().to_string();
        assert_eq!(url, format!("http://127.0.0.1:{port}/mcp"));
        assert!(hub.claude_code_command().unwrap().contains("--header"));
        assert!(hub.json_config().unwrap().contains("Bearer"));

        let runtime = tokio::runtime::Runtime::new().unwrap();
        let list = runtime.block_on(post(
            &url,
            &token,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_clusters"}}),
        ));
        let text = list["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("kind-dev"), "{list}");
        // The change tools are listed (the setting decides whether they work).
        let tools = runtime.block_on(post(
            &url,
            &token,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        ));
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 17);
        let refused = runtime.block_on(post(
            &url,
            &token,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "delete_resource", "arguments": {"cluster": "kind-dev", "kind": "pod", "name": "x"}}}),
        ));
        assert_eq!(refused["result"]["isError"], true);
        assert!(
            refused["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("read-only")
        );
    }

    #[test]
    fn the_token_survives_a_restart_and_regenerating_replaces_it() {
        let credentials = scoped("mcp-hub-token-test");
        let settings = McpSettings {
            port: free_port(),
            ..McpSettings::default()
        };
        let mut hub = McpHub::new(settings.clone(), credentials.clone());
        let mut host = TestHost::new();
        hub.start(&mut host);
        run_started(&mut hub, &mut host);
        let first = hub.token().unwrap().to_string();

        let mut again = McpHub::new(
            McpSettings {
                port: free_port(),
                ..settings
            },
            credentials,
        );
        let mut host2 = TestHost::new();
        again.start(&mut host2);
        run_started(&mut again, &mut host2);
        assert_eq!(again.token().unwrap(), first);

        again.regenerate_token(&mut host2);
        run_started(&mut again, &mut host2);
        assert_ne!(again.token().unwrap(), first);
        again.credentials.delete(TOKEN_KEY).ok();
    }

    #[test]
    fn a_taken_port_is_reported_and_a_disabled_server_is_off() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let mut hub = McpHub::new(
            McpSettings {
                port,
                ..McpSettings::default()
            },
            scoped("mcp-hub-port-test"),
        );
        let mut host = TestHost::new();
        hub.start(&mut host);
        run_started(&mut hub, &mut host);
        match hub.state() {
            ServerState::Failed(message) => {
                assert!(message.contains(&port.to_string()), "{message}")
            }
            other => panic!("{other:?}"),
        }
        assert!(hub.url().is_none());

        hub.settings_changed(
            McpSettings {
                enabled: false,
                port,
                ..McpSettings::default()
            },
            &mut host,
        );
        assert_eq!(hub.state(), &ServerState::Off);
        hub.credentials.delete(TOKEN_KEY).ok();
    }

    #[test]
    fn outside_changes_wait_for_the_user_and_only_exist_when_enabled() {
        let mut hub = McpHub::new(
            McpSettings {
                writes: ExternalWrites::Ask,
                enabled: false,
                ..McpSettings::default()
            },
            scoped("mcp-hub-approval-test"),
        );
        let mut host = TestHost::new();
        hub.start(&mut host);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let approver = hub.approver.clone();
        let proposal = Proposal {
            cluster: ClusterId::new("kind-dev"),
            cluster_name: "kind-dev".into(),
            production: false,
            title: "Scale deployment shop/web from 2 to 3".into(),
            docs: Vec::new(),
            warnings: Vec::new(),
        };
        let waiting = runtime.spawn(approver.approve(proposal));
        host.run_until(&mut hub, |hub, _| !hub.pending().is_empty());
        let id = hub.pending()[0].id;
        assert_eq!(hub.pending()[0].proposal.cluster_name, "kind-dev");
        hub.answer(id, Decision::Approved, &mut host);
        assert_eq!(runtime.block_on(waiting).unwrap(), Decision::Approved);
        assert!(hub.pending().is_empty());
    }
}
