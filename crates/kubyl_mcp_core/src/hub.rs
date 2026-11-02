//! [`McpHub`]: Kubyl's own MCP server for outside clients (Claude Code, Cursor, …).
//!
//! On by default, on `127.0.0.1:7421` ([`crate::settings::McpSettings`]). It gives a client every
//! cluster Kubyl is connected to, through the same tools agent threads get, with the same
//! masking and the user's own RBAC. The bearer token is made once and kept in the OS keychain
//! (clients are configured with it, so it must survive restarts); "Regenerate" replaces it.
//! Change tools are off unless `mcp.writes` is `ask`, and then every change waits for the user's
//! click in Kubyl's window ([`McpHub::pending`], [`McpHub::answer`]).
//!
//! Several Kubyl instances share the port: the first one serves it, and the others find a Kubyl
//! answering with the same token there and wait on [`ServerState::Standby`], trying to take
//! the port over every few seconds, so one of them carries on when the serving one quits.

use std::sync::Arc;
use std::time::Duration;

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

/// How often an instance on standby tries to take the port over.
const STANDBY_RETRY: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    /// Turned off in settings.
    Off,
    Starting,
    Running {
        port: u16,
    },
    /// Another Kubyl instance serves the port (with the same token); this one takes over when
    /// it quits.
    Standby {
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
    pruning: Option<TaskHandle>,
    /// The next takeover attempt while on standby.
    standby: Option<TaskHandle>,
    /// The next start deletes the stored token first (until one start succeeds).
    replace_token: bool,
    /// Counts starts. The blocking credential step runs under `credential_lock` and does nothing
    /// when a newer start has begun, so an abandoned start can't overwrite the newer token.
    generation: Arc<std::sync::atomic::AtomicU64>,
    credential_lock: Arc<parking_lot::Mutex<()>>,
    /// Where the start lock shared by all Kubyl instances lives ([`start_lock`]).
    lock_dir: std::path::PathBuf,
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
            pruning: None,
            standby: None,
            replace_token: false,
            generation: Arc::default(),
            credential_lock: Arc::default(),
            lock_dir: kubyl_settings_core::config_dir(),
        }
    }

    pub fn settings(&self) -> &McpSettings {
        &self.settings
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// `http://127.0.0.1:<port>/mcp` while this instance or another one serves it.
    pub fn url(&self) -> Option<String> {
        match self.state {
            ServerState::Running { port } | ServerState::Standby { port } => {
                Some(format!("http://127.0.0.1:{port}/mcp"))
            }
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
        // Changes nobody answered in time are rejected by their tool call; take them off the list.
        self.pruning = Some(host.every(Duration::from_secs(5), |this, host| {
            this.prune(host);
            Flow::Continue
        }));
        self.restart(host);
    }

    /// Drops waiting changes whose tool call has given up (the answer timeout).
    fn prune(&mut self, host: &mut dyn Host<Self>) {
        let before = self.pending.len();
        self.pending
            .retain(|p| p.reply.as_ref().is_some_and(|r| !r.is_closed()));
        if self.pending.len() != before {
            host.emit(HubEvent::Changed);
            host.notify();
        }
    }

    /// The clusters the tools reach. Cheap: only publishes to the running server.
    pub fn set_clusters(&mut self, clusters: Vec<ToolContext>) {
        let output = self.settings.max_tool_output();
        let clusters = clusters
            .into_iter()
            .map(|mut c| {
                c.max_output = output;
                c.log_lines = 500;
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

    /// Replaces the token (existing clients stop working until reconfigured). The old one is
    /// deleted in the same step that reads the new one, so a settings change during the
    /// replacement can't read the old token first; if it can't be deleted the server stays
    /// stopped and the state says why, instead of pretending to have a new token.
    pub fn regenerate_token(&mut self, host: &mut dyn Host<Self>) {
        if matches!(self.state, ServerState::Standby { .. }) {
            // The serving instance keeps the old token until it restarts; replace it there.
            return;
        }
        self.replace_token = true;
        self.restart(host);
    }

    fn restart(&mut self, host: &mut dyn Host<Self>) {
        self.start_server(false, host);
    }

    /// Starts the server. `takeover` is a standby's retry: it keeps the standby state (and
    /// token) on screen instead of flashing "Starting" every few seconds.
    fn start_server(&mut self, takeover: bool, host: &mut dyn Host<Self>) {
        // Stopped under the instances' start lock (below), so no other instance can take the
        // port between this listener closing and the new one opening with a replaced token.
        let old = self.server.take();
        self.starting = None;
        self.standby = None;
        if !takeover {
            // Nothing may show a token the running server no longer uses.
            self.token = None;
        }
        if !self.settings.enabled {
            drop(old);
            self.state = ServerState::Off;
            self.token = None;
            host.notify();
            return;
        }
        if !takeover {
            self.state = ServerState::Starting;
            host.notify();
        }
        let port = self.settings.port();
        let credentials = self.credentials.clone();
        let replace = self.replace_token;
        let mine = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let latest = self.generation.clone();
        let lock = self.credential_lock.clone();
        let lock_dir = self.lock_dir.clone();
        let handler = Arc::new(ToolsHandler::new(
            Scope::Clusters(self.clusters.subscribe()),
            Some(Writes {
                enabled: self.writes.subscribe(),
                approver: self.approver.clone() as Arc<dyn Approver>,
                off_message: "Kubyl's MCP server is read-only. The user can allow changes (each one approved in Kubyl) with \"Allow changes\" under View → MCP Server in Kubyl, or \"writes\": \"ask\" under \"mcp\" in Kubyl's settings.json.".into(),
            }),
        ));
        let work = async move {
            let (token, _instances) = tokio::task::spawn_blocking(move || {
                let instances = start_lock(&lock_dir, port);
                drop(old);
                let _serialized = lock.lock();
                if latest.load(std::sync::atomic::Ordering::SeqCst) != mine {
                    // A newer start owns the credential now.
                    return Err("superseded by a newer start".to_string());
                }
                if replace {
                    credentials.delete(TOKEN_KEY).map_err(|err| {
                        format!(
                            "The old MCP token couldn't be removed from the credential store ({err}), so it was not replaced. The server is stopped."
                        )
                    })?;
                }
                Ok((token(&credentials)?, instances))
            })
            .await
            .map_err(|e| e.to_string())??;
            // A server we just stopped may still hold the port for a moment (its listener is
            // closed when the aborted task is dropped), most visibly on Windows: wait for it
            // before calling the port taken.
            let mut tries = 0;
            let server = loop {
                let started = server::serve(
                    handler.clone(),
                    ServeOptions {
                        port,
                        token: Some(token.expose_secret().to_string()),
                        activity: None,
                    },
                )
                .await;
                match started {
                    // A takeover only checks once: the other instance is still there or not.
                    Err(err)
                        if err.kind() == std::io::ErrorKind::AddrInUse
                            && tries < 20
                            && !takeover =>
                    {
                        tries += 1;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                        if server::serves_kubyl(port, token.expose_secret()).await {
                            return Ok(Started::Standby(token));
                        }
                        return Err(start_error(port, &err));
                    }
                    other => break other.map_err(|err| start_error(port, &err))?,
                }
            };
            Ok::<_, String>(Started::Serving(server, token))
        };
        self.starting = Some(host.spawn(work, move |this, result, host| {
            let before = this.state.clone();
            match result {
                Ok(Started::Serving(server, token)) => {
                    this.state = ServerState::Running {
                        port: server.addr.port(),
                    };
                    this.server = Some(server);
                    this.token = Some(token);
                    this.replace_token = false;
                }
                Ok(Started::Standby(token)) => {
                    this.state = ServerState::Standby { port };
                    this.token = Some(token);
                    this.standby = Some(host.after(STANDBY_RETRY, |this, host| {
                        if matches!(this.state, ServerState::Standby { .. }) {
                            this.start_server(true, host);
                        }
                    }));
                }
                Err(message) => {
                    this.state = ServerState::Failed(message);
                    this.token = None;
                }
            }
            if this.state != before || !takeover {
                host.emit(HubEvent::Changed);
                host.notify();
            }
        }));
    }

    fn requested(&mut self, request: ApprovalRequest, host: &mut dyn Host<Self>) {
        if !*self.writes.borrow() {
            // Changes were switched off while the request was on its way.
            request.reply.send(Decision::Rejected).ok();
            return;
        }
        if request.reply.is_closed() {
            // The tool call stopped waiting before the request arrived.
            return;
        }
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

/// Serializes starts across Kubyl instances (and hubs): held from stopping the old listener,
/// through reading or replacing the token, until the new listener is bound, so a standby
/// instance can't take the port over with a token that is being replaced. Released when the
/// file is dropped. Best effort: without it a start goes ahead unlocked.
fn start_lock(dir: &std::path::Path, port: u16) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("mcp-{port}.lock")))
        .ok()?;
    file.lock().ok()?;
    Some(file)
}

/// What a start ended with.
enum Started {
    Serving(McpServer, SecretString),
    /// The port is held by another Kubyl that knows the token.
    Standby(SecretString),
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

    /// A hub whose start lock lives in the temp dir, not in the user's config dir.
    fn test_hub(settings: McpSettings, credentials: Credentials) -> McpHub {
        let mut hub = McpHub::new(settings, credentials);
        hub.lock_dir = std::env::temp_dir().join("kubyl-mcp-hub-tests");
        hub
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
        let mut hub = test_hub(settings, credentials());
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
        let mut hub = test_hub(settings.clone(), credentials.clone());
        let mut host = TestHost::new();
        hub.start(&mut host);
        run_started(&mut hub, &mut host);
        let first = hub.token().unwrap().to_string();

        let mut again = test_hub(
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
        assert!(
            matches!(again.state(), ServerState::Running { .. }),
            "{:?}",
            again.state()
        );
        assert_ne!(again.token().unwrap(), first);
        again.credentials.delete(TOKEN_KEY).ok();
    }

    #[test]
    fn a_second_instance_waits_on_standby_and_takes_over() {
        let credentials = scoped("mcp-hub-standby-test");
        let settings = McpSettings {
            port: free_port(),
            ..McpSettings::default()
        };
        let port = settings.port;
        let mut first = test_hub(settings.clone(), credentials.clone());
        let mut host1 = TestHost::new();
        first.start(&mut host1);
        run_started(&mut first, &mut host1);
        assert_eq!(first.state(), &ServerState::Running { port });

        let mut second = test_hub(settings, credentials);
        let mut host2 = TestHost::new();
        second.start(&mut host2);
        run_started(&mut second, &mut host2);
        assert_eq!(second.state(), &ServerState::Standby { port });
        // Clients can be configured from either instance.
        assert_eq!(second.url(), first.url());
        assert_eq!(second.token(), first.token());
        // The serving instance would keep the old token: standby doesn't replace it.
        second.regenerate_token(&mut host2);
        assert_eq!(second.state(), &ServerState::Standby { port });

        // The serving instance replaces the token: the standby picks the new one up on its next
        // try and keeps waiting (it never serves the old one).
        first.regenerate_token(&mut host1);
        run_started(&mut first, &mut host1);
        assert_eq!(first.state(), &ServerState::Running { port });
        let replaced = first.token().unwrap().to_string();
        host2.run_until(&mut second, |hub, _| hub.token() == Some(replaced.as_str()));
        assert_eq!(second.state(), &ServerState::Standby { port });

        drop(first);
        host2.run_until(&mut second, |hub, _| {
            matches!(hub.state(), ServerState::Running { .. })
        });
        assert_eq!(second.state(), &ServerState::Running { port });
        second.credentials.delete(TOKEN_KEY).ok();
    }

    #[test]
    fn a_taken_port_is_reported_and_a_disabled_server_is_off() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let mut hub = test_hub(
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
        let mut hub = test_hub(
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

    #[test]
    fn answered_late_or_after_writes_went_off_a_change_is_rejected_and_dropped() {
        let mut hub = test_hub(
            McpSettings {
                writes: ExternalWrites::Ask,
                enabled: false,
                ..McpSettings::default()
            },
            scoped("mcp-hub-late-test"),
        );
        let mut host = TestHost::new();
        hub.start(&mut host);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let proposal = || Proposal {
            cluster: ClusterId::new("kind-dev"),
            cluster_name: "kind-dev".into(),
            production: false,
            title: "t".into(),
            docs: Vec::new(),
            warnings: Vec::new(),
        };
        // A request whose tool call gave up (the receiver is gone) leaves the list.
        let (reply, answer) = oneshot::channel();
        hub.requested(
            ApprovalRequest {
                proposal: proposal(),
                reply,
            },
            &mut host,
        );
        assert_eq!(hub.pending().len(), 1);
        drop(answer);
        hub.prune(&mut host);
        assert!(hub.pending().is_empty());

        // A change that waits when writes go off is dropped (and so rejected).
        let (reply, waiting) = oneshot::channel();
        hub.requested(
            ApprovalRequest {
                proposal: proposal(),
                reply,
            },
            &mut host,
        );
        assert_eq!(hub.pending().len(), 1);
        hub.settings_changed(
            McpSettings {
                writes: ExternalWrites::Off,
                enabled: false,
                ..McpSettings::default()
            },
            &mut host,
        );
        assert!(hub.pending().is_empty());
        assert!(runtime.block_on(waiting).is_err(), "dropped, not answered");

        // Back on, then a request that arrives after writes were switched off again.
        hub.settings_changed(
            McpSettings {
                writes: ExternalWrites::Ask,
                enabled: false,
                ..McpSettings::default()
            },
            &mut host,
        );
        // With writes on, a request whose tool call already gave up is not shown.
        let (reply, gone) = oneshot::channel();
        drop(gone);
        hub.requested(
            ApprovalRequest {
                proposal: proposal(),
                reply,
            },
            &mut host,
        );
        assert!(hub.pending().is_empty());
        hub.settings_changed(
            McpSettings {
                writes: ExternalWrites::Off,
                enabled: false,
                ..McpSettings::default()
            },
            &mut host,
        );
        let (reply, answer) = oneshot::channel();
        hub.requested(
            ApprovalRequest {
                proposal: proposal(),
                reply,
            },
            &mut host,
        );
        assert!(hub.pending().is_empty());
        assert_eq!(runtime.block_on(answer).unwrap(), Decision::Rejected);
    }
}
