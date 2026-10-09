//! [`AgentCore`]: agents, threads and the user's decisions, on a `kubyl_base::Host`.
//!
//! - One process per agent (`claude`, `codex`, …), shared by its threads; it stops with its
//!   last thread.
//! - One thread per conversation, bound to one cluster. Each thread has its own folder, its own
//!   `KUBECONFIG` for commands, and its own MCP server on loopback whose tools read the cluster
//!   through a per-cluster [`ToolContext`] the app keeps current ([`AgentCore::set_context`]).
//! - What the agent asks that needs the user (permission prompts, commands, reads outside the
//!   folder, every write) waits in the thread's [`Pending`] list until [`AgentCore::answer`].
//!
//! Nothing here logs prompts, tool output or file contents.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_client_protocol_schema::v1::{
    AuthMethod, NewSessionResponse, PermissionOptionKind, RequestPermissionRequest, StopReason,
    ToolKind,
};
use futures::StreamExt as _;
use kubyl_base::host::{Flow, Host, HostExt as _, Pace, Service, TaskHandle};
use kubyl_base::{ClusterId, Notice};
use kubyl_kube_core::cli::CliEnv;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use crate::acp::{self, AgentConnection, ClientCall, SessionScope};
use crate::agents::{self, AgentSpec};
use crate::elicitation::{self, Field};
use crate::jsonrpc::{Responder, RpcError};
use crate::kubeconfig;
use crate::mcp::{self, McpServer};
use crate::policy::{self, ExecuteGrant};
use crate::settings::{AgentSettings, KubectlAccess, SavedThread};
use crate::terminal::{Exit, Launch};
use crate::thread::{ContextChip, Entry, Transcript};
use crate::tools::ToolContext;

/// Identifies a thread in this run of Kubyl.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadId(pub u64);

/// Where Kubyl keeps agent files and finds programs. Filled in by the app.
#[derive(Clone, Debug)]
pub struct AgentEnv {
    /// `<cache dir>/kubyl/agent`: scratch folders and kubeconfigs of threads.
    pub data_dir: PathBuf,
    /// The Kubyl binary, for `kubyl mcp-bridge`.
    pub bridge: PathBuf,
    /// The agent process's working directory.
    pub home: PathBuf,
}

/// An agent and whether it's installed.
#[derive(Clone, Debug)]
pub struct AgentInfo {
    pub spec: AgentSpec,
    /// `None` while not looked up yet or not installed.
    pub program: Option<PathBuf>,
    /// Looked up at least once.
    pub checked: bool,
    pub running: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ThreadStatus {
    /// Starting the agent or the session.
    Starting,
    Idle,
    /// The agent works on a prompt.
    Running,
    /// The agent wants a sign-in first.
    NeedsSignIn {
        /// `(id, name, description)` of methods Kubyl can trigger.
        methods: Vec<(String, String, Option<String>)>,
        hint: Option<String>,
    },
    NotInstalled {
        install: Option<String>,
    },
    Failed(String),
    /// The agent process ended.
    Ended(String),
}

/// A permission option of the agent's prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionChoice {
    pub id: String,
    pub name: String,
    pub allow: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PendingKind {
    /// The agent's own permission prompt for a tool call.
    Permission {
        tool_call: String,
        title: String,
        command: Option<String>,
        execute: bool,
        choices: Vec<PermissionChoice>,
    },
    /// A command the agent wants Kubyl to run.
    Command {
        line: String,
        cwd: PathBuf,
        /// Environment the agent sets for it (values scrubbed for display).
        env: Vec<(String, String)>,
        warnings: Vec<&'static str>,
    },
    /// A read outside the thread's folder.
    Read { path: PathBuf },
    Write {
        path: PathBuf,
        old: Option<String>,
        new: String,
    },
    /// The agent asks the user to fill in a form (`elicitation/create`).
    Form {
        message: String,
        title: Option<String>,
        fields: Vec<Field>,
    },
    /// The agent asks the user to open a web page (a sign-in, a consent page).
    OpenUrl {
        message: String,
        url: String,
        /// The agent's elicitation id, for `elicitation/complete`.
        elicitation: String,
    },
}

/// Something waiting for the user.
pub struct Pending {
    pub id: u64,
    pub kind: PendingKind,
    responder: Option<Responder>,
    /// For commands: what to run once allowed.
    launch: Option<Launch>,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The user's answer to a [`Pending`].
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// A permission option by id.
    Choose(String),
    Allow,
    Deny,
    /// A form's answers (the `content` from [`crate::elicitation::validate`]).
    Submit(Value),
}

/// What a new thread starts with.
#[derive(Clone, Debug)]
pub struct NewThread {
    pub agent: String,
    pub cluster: ClusterId,
    pub cluster_name: String,
    /// The context's kubeconfig file and context name, for `agent.kubectl: context`.
    pub kubeconfig: Option<(PathBuf, String)>,
    pub context: ToolContext,
    /// A first prompt.
    pub prompt: Option<(String, Vec<ContextChip>)>,
    /// Reopen this session of the agent (`session/load`).
    pub resume: Option<SavedThread>,
}

pub struct Thread {
    pub id: ThreadId,
    pub agent: String,
    pub agent_name: String,
    pub cluster: ClusterId,
    pub cluster_name: String,
    pub cwd: PathBuf,
    /// The thread's own folder under the data dir (removed with the thread).
    dir: PathBuf,
    pub status: ThreadStatus,
    pub transcript: Transcript,
    pub session: Option<String>,
    /// `(id, name)` of the agent's modes.
    pub modes: Vec<(String, String)>,
    pub pending: Vec<Pending>,
    /// Prompts waiting for the session or the running turn.
    pub queued: Vec<(String, Vec<ContextChip>)>,
    kubeconfig_source: Option<(PathBuf, String)>,
    resume: Option<SavedThread>,
    first_prompt: bool,
    grant: Option<ExecuteGrant>,
    mcp: Option<McpServer>,
    tasks: Vec<TaskHandle>,
    /// Shown once in the thread (e.g. why `kubectl` reaches nothing).
    pub notes: Vec<String>,
}

impl Thread {
    pub fn title(&self) -> String {
        if let Some(title) = &self.transcript.title {
            return title.clone();
        }
        let first = self.transcript.entries.iter().find_map(|e| match e {
            Entry::User { text, chips } if !text.trim().is_empty() => Some(text.clone()),
            Entry::User { chips, .. } => chips.first().map(|c| c.label.clone()),
            _ => None,
        });
        match first {
            Some(text) => {
                let line = text.lines().next().unwrap_or_default().trim().to_string();
                if line.chars().count() > 60 {
                    format!("{}…", line.chars().take(59).collect::<String>())
                } else {
                    line
                }
            }
            None => "New thread".into(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.status == ThreadStatus::Running
    }

    /// Nothing was asked yet (a thread prepared while the user composes the first message).
    pub fn is_unused(&self) -> bool {
        self.transcript.entries.is_empty() && self.pending.is_empty() && self.queued.is_empty()
    }
}

enum Slot {
    Starting {
        _task: TaskHandle,
    },
    Ready {
        connection: Arc<AgentConnection>,
        _calls: TaskHandle,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    Changed,
    /// A turn ended (for a toast while Kubyl isn't focused).
    TurnEnded {
        thread: ThreadId,
        title: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEffect {
    Remember(SavedThread),
    /// Open a link the user agreed to open (an agent's URL question).
    OpenUrl(String),
}

pub struct AgentCore {
    settings: AgentSettings,
    env: AgentEnv,
    path: Option<OsString>,
    /// What agents and their commands inherit from this process.
    cli_env: CliEnv,
    capabilities: acp::ClientCapabilities,
    session_meta: Option<Value>,
    agents: Vec<AgentInfo>,
    slots: HashMap<String, Slot>,
    threads: Vec<Thread>,
    contexts: HashMap<ClusterId, watch::Sender<ToolContext>>,
    next_thread: u64,
    next_pending: u64,
    _lookup: Option<TaskHandle>,
    ticker: Option<TaskHandle>,
}

impl Service for AgentCore {
    type Event = AgentEvent;
    type Effect = AgentEffect;
}

fn now_seconds() -> i64 {
    jiff::Timestamp::now().as_second()
}

impl AgentCore {
    pub fn new(settings: AgentSettings, env: AgentEnv) -> Self {
        let agents = agents::configured(&settings)
            .into_iter()
            .map(|spec| AgentInfo {
                spec,
                program: None,
                checked: false,
                running: false,
            })
            .collect();
        Self {
            settings,
            env,
            path: None,
            cli_env: CliEnv::default(),
            capabilities: acp::ClientCapabilities::default(),
            session_meta: None,
            agents,
            slots: HashMap::new(),
            threads: Vec::new(),
            contexts: HashMap::new(),
            next_thread: 0,
            next_pending: 0,
            _lookup: None,
            ticker: None,
        }
    }

    /// What agent processes and the commands they run inherit from this process (everything,
    /// by default). A host that keeps secrets in its own environment sets an allow-list. Applies
    /// to agents started after the call.
    pub fn set_cli_env(&mut self, cli_env: CliEnv) {
        self.cli_env = cli_env;
    }

    /// The client capabilities offered to agents in `initialize` (all, by default). A capability
    /// that is off isn't advertised and its requests are refused. Applies to agents started
    /// after the call.
    pub fn set_client_capabilities(&mut self, capabilities: acp::ClientCapabilities) {
        self.capabilities = capabilities;
    }

    /// A JSON object sent as `_meta` of `session/new` and `session/load`, for adapter options
    /// the ACP schema carries there (none, by default). Applies to agents started after the call.
    pub fn set_session_meta(&mut self, meta: Option<Value>) {
        self.session_meta = meta;
    }

    /// Sets where `agent` is installed (`None`: not installed) and marks it as looked up, so
    /// the login-shell search doesn't decide. A host that provides its own adapter binary calls
    /// this. Agents already running keep their process; the next start uses `program`. Does
    /// nothing for an id that isn't in [`Self::agents`]. Notifies observers (the agent list
    /// changed).
    pub fn set_program(
        &mut self,
        agent: &str,
        program: Option<PathBuf>,
        host: &mut dyn Host<Self>,
    ) {
        if self.assign_program(agent, program) {
            host.notify();
        }
    }

    fn assign_program(&mut self, agent: &str, program: Option<PathBuf>) -> bool {
        let Some(info) = self.agents.iter_mut().find(|a| a.spec.id == agent) else {
            return false;
        };
        info.program = program;
        info.checked = true;
        true
    }

    /// Adds a line to the end of `thread`'s transcript, shown as a note (`error`: in the error
    /// style). Nothing is sent to the agent. Notifies observers; does nothing for an unknown
    /// thread.
    pub fn notice(
        &mut self,
        thread: ThreadId,
        text: impl Into<String>,
        error: bool,
        host: &mut dyn Host<Self>,
    ) {
        if self.push_notice(thread, text.into(), error) {
            host.notify();
        }
    }

    fn push_notice(&mut self, thread: ThreadId, text: String, error: bool) -> bool {
        let Some(thread) = self.thread_mut(thread) else {
            return false;
        };
        thread.transcript.notice(text, error);
        true
    }

    pub fn settings(&self) -> &AgentSettings {
        &self.settings
    }

    pub fn agents(&self) -> &[AgentInfo] {
        &self.agents
    }

    pub fn agent(&self, id: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.spec.id == id)
    }

    /// The agent new threads use: the setting, else the first installed one.
    pub fn default_agent(&self, last: Option<&str>) -> Option<&AgentInfo> {
        let installed = |id: &str| self.agent(id).filter(|a| a.program.is_some());
        self.settings
            .default
            .as_deref()
            .and_then(installed)
            .or_else(|| last.and_then(installed))
            .or_else(|| self.agents.iter().find(|a| a.program.is_some()))
    }

    pub fn threads(&self) -> &[Thread] {
        &self.threads
    }

    pub fn thread(&self, id: ThreadId) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }

    fn thread_mut(&mut self, id: ThreadId) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    fn connection(&self, agent: &str) -> Option<Arc<AgentConnection>> {
        match self.slots.get(agent) {
            Some(Slot::Ready { connection, .. }) => Some(connection.clone()),
            _ => None,
        }
    }

    /// What the agent printed on stderr (scrubbed), for "Agent output".
    pub fn agent_output(&self, agent: &str) -> Option<String> {
        self.connection(agent).map(|c| c.stderr())
    }

    /// Output of a command a thread's agent ran through Kubyl: `(command line, output,
    /// truncated, exit)`.
    pub fn terminal(
        &self,
        thread: ThreadId,
        terminal: &str,
    ) -> Option<(String, String, bool, Option<Exit>)> {
        let thread = self.thread(thread)?;
        let terminal = self.connection(&thread.agent)?.terminals.get(terminal)?;
        let (output, truncated, exit) = terminal.snapshot();
        Some((terminal.command_line.clone(), output, truncated, exit))
    }

    // ----- Setup -----

    /// Looks up which agents are installed (the login shell's `PATH`; off the UI thread).
    pub fn refresh_agents(&mut self, host: &mut dyn Host<Self>) {
        let commands: Vec<String> = self.agents.iter().map(|a| a.spec.command.clone()).collect();
        self._lookup = Some(host.background(
            async move {
                let path = kubyl_kube_core::auth::shell_env::path();
                let found: Vec<Option<PathBuf>> = commands
                    .iter()
                    .map(|c| agents::resolve(c, path.as_deref()))
                    .collect();
                (path, found)
            },
            |this, (path, found), host| {
                this.path = path;
                for (agent, program) in this.agents.iter_mut().zip(found) {
                    agent.program = program;
                    agent.checked = true;
                }
                host.notify();
            },
        ));
    }

    pub fn settings_changed(&mut self, settings: AgentSettings, host: &mut dyn Host<Self>) {
        if settings == self.settings {
            return;
        }
        let specs = agents::configured(&settings);
        self.settings = settings;
        self.agents = specs
            .into_iter()
            .map(|spec| {
                let running = self.slots.contains_key(&spec.id);
                AgentInfo {
                    spec,
                    program: None,
                    checked: false,
                    running,
                }
            })
            .collect();
        self.refresh_agents(host);
    }

    /// The tool context of `cluster`'s threads (connection, selection, Prometheus, alerts).
    pub fn set_context(&mut self, cluster: &ClusterId, mut context: ToolContext) {
        context.max_output = self.settings.max_tool_output();
        context.log_lines = self.settings.log_lines();
        if let Some(sender) = self.contexts.get(cluster) {
            sender.send_replace(context);
        }
    }

    /// Clusters with threads, for the app to keep their contexts current.
    pub fn clusters(&self) -> Vec<ClusterId> {
        self.contexts.keys().cloned().collect()
    }

    // ----- Threads -----

    pub fn start_thread(&mut self, new: NewThread, host: &mut dyn Host<Self>) -> ThreadId {
        self.next_thread += 1;
        let id = ThreadId(self.next_thread);
        let agent_name = self
            .agent(&new.agent)
            .map(|a| a.spec.name.clone())
            .unwrap_or_else(|| new.agent.clone());
        let key = format!("{}-{}", now_seconds(), id.0);
        let dir = self.env.data_dir.join("threads").join(key);
        let cwd = match (&new.resume, &self.settings.cwd) {
            (Some(saved), _) if PathBuf::from(&saved.cwd).is_dir() => PathBuf::from(&saved.cwd),
            (_, Some(cwd)) if !cwd.trim().is_empty() => expand(cwd),
            _ => dir.join("work"),
        };
        let mut context = new.context;
        context.max_output = self.settings.max_tool_output();
        context.log_lines = self.settings.log_lines();
        self.contexts
            .entry(new.cluster.clone())
            .and_modify(|sender| {
                sender.send_replace(context.clone());
            })
            .or_insert_with(|| watch::channel(context).0);
        let mut transcript = Transcript::default();
        let mut queued = Vec::new();
        if let Some((text, chips)) = new.prompt {
            transcript.push_user(text.clone(), chips.clone());
            queued.push((text, chips));
        }
        if let Some(saved) = &new.resume {
            transcript.title = Some(saved.title.clone());
        }
        self.threads.push(Thread {
            id,
            agent: new.agent.clone(),
            agent_name,
            cluster: new.cluster,
            cluster_name: new.cluster_name,
            cwd,
            dir,
            status: ThreadStatus::Starting,
            transcript,
            session: None,
            modes: Vec::new(),
            pending: Vec::new(),
            queued,
            kubeconfig_source: new.kubeconfig,
            resume: new.resume,
            first_prompt: true,
            grant: None,
            mcp: None,
            tasks: Vec::new(),
            notes: Vec::new(),
        });
        self.ensure_agent(&new.agent, host);
        host.notify();
        id
    }

    /// Starts the thread's agent and session again (after a failure or a sign-in).
    pub fn retry(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        if thread.session.is_some() {
            return;
        }
        thread.status = ThreadStatus::Starting;
        let agent = thread.agent.clone();
        self.ensure_agent(&agent, host);
        host.notify();
    }

    pub fn close_thread(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let Some(index) = self.threads.iter().position(|t| t.id == id) else {
            return;
        };
        self.cancel(id, host);
        let thread = self.threads.remove(index);
        if let (Some(session), Some(connection)) = (&thread.session, self.connection(&thread.agent))
        {
            connection.close_session(session);
        }
        // The folder: only the scratch one is ours to remove.
        let dir = thread.dir.clone();
        host.background(
            async move {
                std::fs::remove_dir_all(&dir).ok();
            },
            |_, _, _| {},
        )
        .detach();
        if !self.threads.iter().any(|t| t.agent == thread.agent) {
            self.slots.remove(&thread.agent);
            self.set_running(&thread.agent, false);
        }
        if !self.threads.iter().any(|t| t.cluster == thread.cluster) {
            self.contexts.remove(&thread.cluster);
        }
        drop(thread);
        host.notify();
    }

    /// Sends a prompt (queued while the session starts or a turn runs).
    pub fn send(
        &mut self,
        id: ThreadId,
        text: String,
        chips: Vec<ContextChip>,
        host: &mut dyn Host<Self>,
    ) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        if text.trim().is_empty() && chips.is_empty() {
            return;
        }
        thread.transcript.push_user(text.clone(), chips.clone());
        thread.queued.push((text, chips));
        match thread.status.clone() {
            ThreadStatus::Idle => self.next_prompt(id, host),
            ThreadStatus::Failed(_)
            | ThreadStatus::Ended(_)
            | ThreadStatus::NotInstalled { .. } => self.retry(id, host),
            _ => {}
        }
        host.notify();
    }

    pub fn cancel(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        thread.queued.clear();
        for pending in thread.pending.drain(..) {
            answer_cancelled(pending);
        }
        let running = thread.is_running();
        let (agent, session) = (thread.agent.clone(), thread.session.clone());
        if running && let (Some(connection), Some(session)) = (self.connection(&agent), session) {
            connection.cancel(&session);
        }
        host.notify();
    }

    pub fn set_mode(&mut self, id: ThreadId, mode: String, host: &mut dyn Host<Self>) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        let (Some(session), agent) = (thread.session.clone(), thread.agent.clone()) else {
            return;
        };
        thread.transcript.mode = Some(mode.clone());
        let Some(connection) = self.connection(&agent) else {
            return;
        };
        host.spawn(
            async move { connection.set_mode(&session, &mode).await },
            move |this, result, host| {
                if let Err(err) = result
                    && let Some(thread) = this.thread_mut(id)
                {
                    thread
                        .transcript
                        .notice(format!("The mode didn't change: {}", err.message), true);
                    host.notify();
                }
            },
        )
        .detach();
        host.notify();
    }

    /// Changes a session setting (model, thinking level, mode…). Shown right away; the agent's
    /// answer replaces the settings.
    pub fn set_config(
        &mut self,
        id: ThreadId,
        config: String,
        value: crate::thread::ConfigValue,
        host: &mut dyn Host<Self>,
    ) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        let (Some(session), agent) = (thread.session.clone(), thread.agent.clone()) else {
            return;
        };
        thread.transcript.set_config(&config, &value);
        let Some(connection) = self.connection(&agent) else {
            return;
        };
        host.spawn(
            async move {
                connection
                    .set_config_option(&session, &config, &value)
                    .await
            },
            move |this, result, host| {
                let Some(thread) = this.thread_mut(id) else {
                    return;
                };
                match result {
                    Ok(answer) => {
                        if let Ok(options) =
                            serde_json::from_value::<
                                Vec<agent_client_protocol_schema::v1::SessionConfigOption>,
                            >(answer["configOptions"].clone())
                        {
                            thread.transcript.config = crate::thread::config_options(&options);
                        }
                    }
                    Err(err) => thread
                        .transcript
                        .notice(format!("The setting didn't change: {}", err.message), true),
                }
                host.notify();
            },
        )
        .detach();
        host.notify();
    }

    pub fn sign_in(&mut self, id: ThreadId, method: String, host: &mut dyn Host<Self>) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        let agent = thread.agent.clone();
        thread.status = ThreadStatus::Starting;
        let Some(connection) = self.connection(&agent) else {
            self.retry(id, host);
            return;
        };
        host.spawn(
            async move { connection.authenticate(&method).await },
            move |this, result, host| {
                match result {
                    Ok(()) => this.open_session(id, host),
                    Err(err) => {
                        if let Some(thread) = this.thread_mut(id) {
                            thread.status =
                                ThreadStatus::Failed(format!("Sign-in failed: {}", err.message));
                        }
                    }
                }
                host.notify();
            },
        )
        .detach();
        host.notify();
    }

    // ----- Agents -----

    fn set_running(&mut self, agent: &str, running: bool) {
        if let Some(info) = self.agents.iter_mut().find(|a| a.spec.id == agent) {
            info.running = running;
        }
    }

    fn ensure_agent(&mut self, agent: &str, host: &mut dyn Host<Self>) {
        match self.slots.get(agent) {
            Some(Slot::Ready { .. }) => {
                self.open_waiting(agent, host);
                return;
            }
            Some(Slot::Starting { .. }) => return,
            None => {}
        }
        let info = self.agent(agent).cloned();
        let program = info.as_ref().and_then(|i| i.program.clone());
        let checked = info.as_ref().is_some_and(|i| i.checked);
        let Some(program) = program else {
            if !checked && info.is_some() {
                // Not looked up yet: look up, then try again.
                let commands: Vec<String> =
                    self.agents.iter().map(|a| a.spec.command.clone()).collect();
                let agent = agent.to_string();
                self._lookup = Some(host.background(
                    async move {
                        let path = kubyl_kube_core::auth::shell_env::path();
                        let found: Vec<Option<PathBuf>> = commands
                            .iter()
                            .map(|c| agents::resolve(c, path.as_deref()))
                            .collect();
                        (path, found)
                    },
                    move |this, (path, found), host| {
                        this.path = path;
                        for (info, program) in this.agents.iter_mut().zip(found) {
                            info.program = program;
                            info.checked = true;
                        }
                        this.ensure_agent(&agent, host);
                        host.notify();
                    },
                ));
                return;
            }
            let install = info.and_then(|i| i.spec.install);
            for thread in self.threads.iter_mut().filter(|t| t.agent == agent) {
                thread.status = ThreadStatus::NotInstalled {
                    install: install.clone(),
                };
            }
            host.notify();
            return;
        };
        let spec = info.map(|i| i.spec).unwrap_or_else(|| AgentSpec {
            id: agent.to_string(),
            name: agent.to_string(),
            command: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            install: None,
            sign_in: None,
            custom: true,
        });
        let launch = acp::Launch {
            program,
            args: spec.args.clone(),
            env: spec.env.clone(),
            path: self.path.clone(),
            kubeconfig: self.env.data_dir.join("empty-kubeconfig"),
            cwd: self.env.home.clone(),
            cli_env: self.cli_env.clone(),
            capabilities: self.capabilities,
            session_meta: self.session_meta.clone(),
        };
        let agent_id = agent.to_string();
        let task = host.spawn(
            async move {
                if let Some(parent) = launch.kubeconfig.parent() {
                    tokio::fs::create_dir_all(parent).await.ok();
                }
                tokio::fs::write(&launch.kubeconfig, kubeconfig::EMPTY)
                    .await
                    .ok();
                acp::start(launch).await
            },
            move |this, result, host| {
                this.agent_started(&agent_id, result, host);
            },
        );
        self.slots
            .insert(agent.to_string(), Slot::Starting { _task: task });
        self.set_running(agent, true);
    }

    fn agent_started(
        &mut self,
        agent: &str,
        result: Result<(Arc<AgentConnection>, mpsc::UnboundedReceiver<ClientCall>), String>,
        host: &mut dyn Host<Self>,
    ) {
        match result {
            Ok((connection, calls)) => self.attach(agent, connection, calls, host),
            Err(message) => {
                self.slots.remove(agent);
                self.set_running(agent, false);
                for thread in self.threads.iter_mut().filter(|t| t.agent == agent) {
                    if thread.session.is_none() {
                        thread.status = ThreadStatus::Failed(message.clone());
                    }
                }
                host.notify();
            }
        }
    }

    /// Uses a connected agent (also how tests plug in a fake one).
    pub fn attach(
        &mut self,
        agent: &str,
        connection: Arc<AgentConnection>,
        calls: mpsc::UnboundedReceiver<ClientCall>,
        host: &mut dyn Host<Self>,
    ) {
        let stream = futures::stream::unfold(calls, |mut calls| async move {
            calls.recv().await.map(|call| (call, calls))
        })
        .boxed();
        let agent_id = agent.to_string();
        let task = host.batches(stream, Pace::IMMEDIATE, move |this, batch, host| {
            for call in batch {
                this.handle_call(&agent_id, call, host);
            }
            host.notify();
            Flow::Continue
        });
        self.slots.insert(
            agent.to_string(),
            Slot::Ready {
                connection,
                _calls: task,
            },
        );
        self.set_running(agent, true);
        self.open_waiting(agent, host);
    }

    fn open_waiting(&mut self, agent: &str, host: &mut dyn Host<Self>) {
        let waiting: Vec<ThreadId> = self
            .threads
            .iter()
            .filter(|t| t.agent == agent && t.session.is_none())
            .filter(|t| {
                matches!(
                    t.status,
                    ThreadStatus::Starting
                        | ThreadStatus::NotInstalled { .. }
                        | ThreadStatus::Failed(_)
                        | ThreadStatus::Ended(_)
                )
            })
            .map(|t| t.id)
            .collect();
        for id in waiting {
            if let Some(thread) = self.thread_mut(id) {
                thread.status = ThreadStatus::Starting;
            }
            self.open_session(id, host);
        }
    }

    fn open_session(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let Some(thread) = self.thread(id) else {
            return;
        };
        let Some(connection) = self.connection(&thread.agent) else {
            return;
        };
        let Some(context) = self.contexts.get(&thread.cluster).map(|s| s.subscribe()) else {
            return;
        };
        let scratch = thread.cwd.starts_with(&thread.dir);
        let cwd = thread.cwd.clone();
        let dir = thread.dir.clone();
        let access = self.settings.kubectl;
        let source = thread.kubeconfig_source.clone();
        let resume = thread.resume.as_ref().map(|s| s.session.clone());
        let bridge = self.env.bridge.clone();
        let path = self.path.clone();
        let work = async move {
            if scratch {
                tokio::fs::create_dir_all(&cwd)
                    .await
                    .map_err(|e| format!("{}: {e}", cwd.display()))?;
            }
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| format!("{}: {e}", dir.display()))?;
            let mut note = None;
            let content = match (access, source) {
                (KubectlAccess::Context, Some((file, context))) => {
                    match kubeconfig::context_kubeconfig(&file, &context) {
                        Ok(content) => content,
                        Err(err) => {
                            note = Some(format!(
                                "`kubectl` in the agent's commands reaches nothing: {err}."
                            ));
                            kubeconfig::EMPTY.to_string()
                        }
                    }
                }
                _ => kubeconfig::EMPTY.to_string(),
            };
            let kubeconfig_path = dir.join("kubeconfig");
            tokio::fs::write(&kubeconfig_path, content)
                .await
                .map_err(|e| format!("{}: {e}", kubeconfig_path.display()))?;
            let server = mcp::serve(context, None)
                .await
                .map_err(|e| format!("Kubyl's tools didn't start: {e}"))?;
            let entry = server.acp_entry(connection.init.http_mcp(), &bridge);
            let scope = SessionScope {
                thread: id.0,
                root: cwd.clone(),
                kubeconfig: kubeconfig_path,
                path,
            };
            let response = match resume {
                Some(session) if connection.init.load_session() => connection
                    .load_session(&session, scope, entry)
                    .await
                    .map(|value| (session, value)),
                _ => connection.new_session(scope, entry).await.map(|r| {
                    let value = serde_json::to_value(&r).unwrap_or_default();
                    (r.session_id.0.to_string(), value)
                }),
            };
            Ok::<_, String>((server, response, note))
        };
        let task = host.spawn(work, move |this, result, host| {
            this.session_opened(id, result, host);
        });
        if let Some(thread) = self.thread_mut(id) {
            thread.tasks.push(task);
        }
    }

    #[allow(clippy::type_complexity)]
    fn session_opened(
        &mut self,
        id: ThreadId,
        result: Result<(McpServer, Result<(String, Value), RpcError>, Option<String>), String>,
        host: &mut dyn Host<Self>,
    ) {
        let auth = self.thread(id).and_then(|t| {
            let connection = self.connection(&t.agent)?;
            let hint = self.agent(&t.agent).and_then(|a| a.spec.sign_in.clone());
            Some((
                connection.init.auth_methods.clone(),
                hint,
                connection.init.load_session(),
            ))
        });
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        let (server, response, note) = match result {
            Ok(parts) => parts,
            Err(message) => {
                thread.status = ThreadStatus::Failed(message);
                host.notify();
                return;
            }
        };
        if let Some(note) = note {
            thread.notes.push(note);
        }
        let (session, value) = match response {
            Ok(ok) => ok,
            Err(err) if err.code == RpcError::AUTH_REQUIRED => {
                let (methods, hint, _) = auth.unwrap_or_default();
                thread.status = ThreadStatus::NeedsSignIn {
                    methods: methods
                        .iter()
                        .filter_map(|m| match m {
                            AuthMethod::Agent(m) => {
                                Some((m.id.0.to_string(), m.name.clone(), m.description.clone()))
                            }
                            _ => None,
                        })
                        .collect(),
                    hint,
                };
                host.notify();
                return;
            }
            Err(err) => {
                thread.status = ThreadStatus::Failed(err.message);
                host.notify();
                return;
            }
        };
        if let Ok(response) = serde_json::from_value::<NewSessionResponse>(value.clone())
            && let Some(modes) = response.modes
        {
            thread.transcript.mode = Some(modes.current_mode_id.0.to_string());
            thread.modes = modes
                .available_modes
                .iter()
                .map(|m| (m.id.0.to_string(), m.name.clone()))
                .collect();
        } else if let Some(modes) = value.get("modes")
            && let Ok(modes) = serde_json::from_value::<
                agent_client_protocol_schema::v1::SessionModeState,
            >(modes.clone())
        {
            thread.transcript.mode = Some(modes.current_mode_id.0.to_string());
            thread.modes = modes
                .available_modes
                .iter()
                .map(|m| (m.id.0.to_string(), m.name.clone()))
                .collect();
        }
        if let Ok(options) = serde_json::from_value::<
            Vec<agent_client_protocol_schema::v1::SessionConfigOption>,
        >(value["configOptions"].clone())
        {
            thread.transcript.config = crate::thread::config_options(&options);
        }
        thread.mcp = Some(server);
        thread.session = Some(session);
        if thread.resume.take().is_some() {
            thread.first_prompt = false;
        }
        thread.status = ThreadStatus::Idle;
        // A thread prepared before its first message isn't worth reopening yet.
        let used = thread
            .transcript
            .entries
            .iter()
            .any(|e| matches!(e, Entry::User { .. }));
        let remember = used && auth.is_some_and(|(_, _, load)| load);
        if remember {
            self.remember(id, host);
        }
        self.next_prompt(id, host);
        host.notify();
    }

    fn remember(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let Some(thread) = self.thread(id) else {
            return;
        };
        let Some(session) = &thread.session else {
            return;
        };
        host.effect(AgentEffect::Remember(SavedThread {
            agent: thread.agent.clone(),
            cluster: thread.cluster.clone(),
            // state.json keeps no conversation secrets.
            title: kubyl_resources_core::redact::scrub_text(&thread.title()).into_owned(),
            cwd: thread.cwd.display().to_string(),
            session: session.clone(),
            updated: now_seconds(),
        }));
    }

    fn next_prompt(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let embedded = self
            .thread(id)
            .and_then(|t| self.connection(&t.agent))
            .is_some_and(|c| c.init.embedded_context());
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        if thread.status != ThreadStatus::Idle || thread.queued.is_empty() {
            return;
        }
        let Some(session) = thread.session.clone() else {
            return;
        };
        let (text, chips) = thread.queued.remove(0);
        let first = std::mem::replace(&mut thread.first_prompt, false);
        let blocks = prompt_blocks(&thread.cluster_name, first, &text, &chips, embedded);
        thread.status = ThreadStatus::Running;
        let agent = thread.agent.clone();
        let Some(connection) = self.connection(&agent) else {
            return;
        };
        if let Err(err) = connection.start_prompt(&session, blocks) {
            self.turn_ended(id, Err(err), host);
        }
    }

    fn turn_ended(
        &mut self,
        id: ThreadId,
        result: Result<agent_client_protocol_schema::v1::PromptResponse, RpcError>,
        host: &mut dyn Host<Self>,
    ) {
        let hint = self
            .thread(id)
            .and_then(|t| self.agent(&t.agent))
            .and_then(|a| a.spec.sign_in.clone());
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        thread.transcript.settle();
        for pending in thread.pending.drain(..) {
            answer_cancelled(pending);
        }
        thread.status = ThreadStatus::Idle;
        match result {
            Ok(response) => match response.stop_reason {
                StopReason::EndTurn => {}
                StopReason::Cancelled => thread.transcript.notice("Stopped.", false),
                StopReason::MaxTokens => thread
                    .transcript
                    .notice("The agent stopped: it reached its token limit.", true),
                StopReason::MaxTurnRequests => thread
                    .transcript
                    .notice("The agent stopped: it reached its limit of steps.", true),
                StopReason::Refusal => thread
                    .transcript
                    .notice("The agent declined to continue.", true),
                _ => {}
            },
            Err(err) if err.code == RpcError::AUTH_REQUIRED => {
                thread.status = ThreadStatus::NeedsSignIn {
                    methods: Vec::new(),
                    hint,
                };
            }
            Err(err) => thread.transcript.notice(err.message, true),
        }
        let title = thread.title();
        // Finished turns keep only the newest task handles.
        thread.tasks.clear();
        host.emit(AgentEvent::TurnEnded { thread: id, title });
        self.remember_if_loadable(id, host);
        self.next_prompt(id, host);
        host.notify();
    }

    fn remember_if_loadable(&mut self, id: ThreadId, host: &mut dyn Host<Self>) {
        let loadable = self
            .thread(id)
            .and_then(|t| self.connection(&t.agent))
            .is_some_and(|c| c.init.load_session());
        if loadable {
            self.remember(id, host);
        }
    }

    // ----- What the agent asks -----

    fn thread_of_session(&mut self, agent: &str, session: &str) -> Option<&mut Thread> {
        self.threads
            .iter_mut()
            .find(|t| t.agent == agent && t.session.as_deref() == Some(session))
    }

    fn handle_call(&mut self, agent: &str, call: ClientCall, host: &mut dyn Host<Self>) {
        match call {
            ClientCall::Update(notification) => {
                let session = notification.session_id.0.to_string();
                if let Some(thread) = self.thread_of_session(agent, &session) {
                    thread.transcript.apply(notification.update);
                } else if let Some(thread) = self.threads.iter_mut().find(|t| {
                    t.agent == agent && t.resume.as_ref().is_some_and(|r| r.session == session)
                }) {
                    // History of a session being reopened.
                    thread.transcript.apply(notification.update);
                }
            }
            ClientCall::Permission { request, responder } => {
                self.permission(agent, *request, responder);
            }
            ClientCall::CreateTerminal { request, responder } => {
                let session = request.session_id.0.to_string();
                let connection = self.connection(agent);
                let path = self.path.clone();
                let cli_env = self.cli_env.clone();
                let next = self.next_pending + 1;
                let Some(thread) = self.thread_of_session(agent, &session) else {
                    responder.err(RpcError::internal("The thread was closed"));
                    return;
                };
                let scope = connection.as_ref().and_then(|c| c.scope(&session));
                let line = policy::command_line(&request.command, &request.args);
                let mut warnings = policy::command_warnings(&line);
                let cwd = request.cwd.clone().unwrap_or_else(|| thread.cwd.clone());
                // The thread's kubeconfig always wins; the agent can't swap in another one.
                let env: Vec<(String, String)> = request
                    .env
                    .iter()
                    .filter(|e| !e.name.eq_ignore_ascii_case("KUBECONFIG"))
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect();
                if env.len() != request.env.len() {
                    warnings.push("Tried to set KUBECONFIG; Kubyl keeps the thread's.");
                }
                let launch = Launch {
                    command: request.command.clone(),
                    args: request.args.clone(),
                    env: env.clone(),
                    cwd: cwd.clone(),
                    output_limit: request.output_byte_limit,
                    path,
                    kubeconfig: scope.map(|s| s.kubeconfig),
                    cli_env,
                };
                let inner = policy::shell_inner(&request.command, &request.args);
                let mut lines = vec![line.as_str()];
                lines.extend(inner.as_deref());
                // A grant covers exactly the command the user saw, with no extra environment.
                let granted = warnings.is_empty()
                    && env.is_empty()
                    && thread
                        .grant
                        .as_ref()
                        .is_some_and(|g| g.covers(&lines, Instant::now()));
                if granted {
                    thread.grant = None;
                    self.run_command(agent, launch, responder, host);
                } else {
                    thread.pending.push(Pending {
                        id: next,
                        kind: PendingKind::Command {
                            line,
                            cwd,
                            env: env
                                .iter()
                                .map(|(name, value)| {
                                    (
                                        name.clone(),
                                        kubyl_resources_core::redact::scrub_text(value)
                                            .into_owned(),
                                    )
                                })
                                .collect(),
                            warnings,
                        },
                        responder: Some(responder),
                        launch: Some(launch),
                    });
                    self.next_pending = next;
                }
            }
            ClientCall::ReadFile { request, responder } => {
                let next = self.next_pending + 1;
                let session = request.session_id.0.to_string();
                let Some(thread) = self.thread_of_session(agent, &session) else {
                    responder.err(RpcError::internal("The thread was closed"));
                    return;
                };
                thread.pending.push(Pending {
                    id: next,
                    kind: PendingKind::Read {
                        path: request.path.clone(),
                    },
                    responder: Some(responder),
                    launch: None,
                });
                self.next_pending = next;
            }
            ClientCall::WriteFile {
                request,
                old,
                responder,
            } => {
                let next = self.next_pending + 1;
                let session = request.session_id.0.to_string();
                let Some(thread) = self.thread_of_session(agent, &session) else {
                    responder.err(RpcError::internal("The thread was closed"));
                    return;
                };
                thread.pending.push(Pending {
                    id: next,
                    kind: PendingKind::Write {
                        path: request.path.clone(),
                        old,
                        new: request.content.clone(),
                    },
                    responder: Some(responder),
                    launch: None,
                });
                self.next_pending = next;
            }
            ClientCall::Elicitation { params, responder } => {
                self.elicitation(agent, params, responder);
            }
            ClientCall::ElicitationComplete { id } => {
                for thread in self.threads.iter_mut().filter(|t| t.agent == agent) {
                    let done: Vec<usize> = thread
                        .pending
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| matches!(&p.kind, PendingKind::OpenUrl { elicitation, .. } if *elicitation == id))
                        .map(|(i, _)| i)
                        .collect();
                    for i in done.into_iter().rev() {
                        let mut pending = thread.pending.remove(i);
                        if let Some(responder) = pending.responder.take() {
                            responder.ok(elicitation::accept(json!({})));
                        }
                    }
                }
            }
            ClientCall::TurnEnded { session, result } => {
                if let Some(id) = self.thread_of_session(agent, &session).map(|t| t.id) {
                    self.turn_ended(id, result, host);
                }
            }
            ClientCall::Closed { stderr_tail } => {
                self.slots.remove(agent);
                self.set_running(agent, false);
                let message = if stderr_tail.is_empty() {
                    "The agent stopped.".to_string()
                } else {
                    format!("The agent stopped:\n{stderr_tail}")
                };
                for thread in self.threads.iter_mut().filter(|t| t.agent == agent) {
                    thread.transcript.settle();
                    for pending in thread.pending.drain(..) {
                        answer_cancelled(pending);
                    }
                    thread.session = None;
                    thread.mcp = None;
                    thread.tasks.clear();
                    thread.status = ThreadStatus::Ended(message.clone());
                }
            }
        }
    }

    fn permission(&mut self, agent: &str, request: RequestPermissionRequest, responder: Responder) {
        let session = request.session_id.0.to_string();
        let next = self.next_pending + 1;
        let Some(thread) = self.thread_of_session(agent, &session) else {
            responder.ok(json!({ "outcome": { "outcome": "cancelled" } }));
            return;
        };
        let tool_id = request.tool_call.tool_call_id.0.to_string();
        let known = thread.transcript.tool(&tool_id).cloned();
        let fields = &request.tool_call.fields;
        // Kubyl's own tools are read-only and masked: allow them without a prompt ("always",
        // so the agent stops asking for that tool).
        let kubyl = [fields.name.as_deref(), fields.title.as_deref()]
            .into_iter()
            .chain(known.iter().flat_map(|t| [Some(t.title.as_str())]))
            .flatten()
            .any(crate::thread::is_kubyl_tool);
        if kubyl {
            let option = [
                PermissionOptionKind::AllowAlways,
                PermissionOptionKind::AllowOnce,
            ]
            .iter()
            .find_map(|kind| request.options.iter().find(|o| o.kind == *kind));
            // Without an allow option the user decides.
            if let Some(option) = option {
                responder.ok(json!({ "outcome": { "outcome": "selected", "optionId": option.option_id.0.as_ref() } }));
                return;
            }
        }
        let title = fields
            .title
            .clone()
            .or_else(|| known.as_ref().map(|t| t.title.clone()))
            .unwrap_or_else(|| "A tool call".into());
        let execute = matches!(
            fields.kind.or(known.as_ref().and_then(|t| t.kind)),
            Some(ToolKind::Execute)
        );
        let command = fields
            .raw_input
            .as_ref()
            .and_then(|input| {
                crate::thread::ToolEntry {
                    id: String::new(),
                    title: String::new(),
                    kind: None,
                    status: crate::thread::ToolStatus::Pending,
                    content: Vec::new(),
                    raw_input: Some(input.clone()),
                    kubyl: None,
                }
                .command()
            })
            .or_else(|| known.as_ref().and_then(|t| t.command()));
        let choices = request
            .options
            .iter()
            .map(|o| PermissionChoice {
                id: o.option_id.0.to_string(),
                name: o.name.clone(),
                allow: matches!(
                    o.kind,
                    PermissionOptionKind::AllowOnce | PermissionOptionKind::AllowAlways
                ),
            })
            .collect();
        thread.pending.push(Pending {
            id: next,
            kind: PendingKind::Permission {
                tool_call: tool_id,
                title,
                command,
                execute,
                choices,
            },
            responder: Some(responder),
            launch: None,
        });
        self.next_pending = next;
    }

    /// An agent question: shown in the thread of its session, else the agent's running one.
    fn elicitation(&mut self, agent: &str, params: Value, responder: Responder) {
        let request = match elicitation::parse(&params) {
            Ok(request) => request,
            Err(message) => {
                responder.err(RpcError::invalid_params(message));
                return;
            }
        };
        let next = self.next_pending + 1;
        let session = elicitation::session_of(&params);
        let index = self
            .threads
            .iter()
            .position(|t| t.agent == agent && session.is_some() && t.session == session)
            .or_else(|| {
                self.threads
                    .iter()
                    .position(|t| t.agent == agent && t.is_running())
            })
            .or_else(|| self.threads.iter().position(|t| t.agent == agent));
        let Some(index) = index else {
            responder.ok(elicitation::cancel());
            return;
        };
        let kind = match request {
            elicitation::Request::Form {
                message,
                title,
                fields,
            } => PendingKind::Form {
                message,
                title,
                fields,
            },
            elicitation::Request::Url { message, url, id } => PendingKind::OpenUrl {
                message,
                url,
                elicitation: id,
            },
        };
        self.threads[index].pending.push(Pending {
            id: next,
            kind,
            responder: Some(responder),
            launch: None,
        });
        self.next_pending = next;
    }

    /// The user's answer to `pending` of `thread`.
    pub fn answer(
        &mut self,
        id: ThreadId,
        pending: u64,
        answer: Answer,
        host: &mut dyn Host<Self>,
    ) {
        let Some(thread) = self.thread_mut(id) else {
            return;
        };
        let Some(index) = thread.pending.iter().position(|p| p.id == pending) else {
            return;
        };
        let mut item = thread.pending.remove(index);
        let Some(responder) = item.responder.take() else {
            return;
        };
        let agent = thread.agent.clone();
        match (item.kind, answer) {
            (
                PendingKind::Permission {
                    command,
                    execute,
                    choices,
                    ..
                },
                answer,
            ) => {
                let chosen = match answer {
                    Answer::Choose(id) => choices.iter().find(|c| c.id == id).cloned(),
                    Answer::Allow => choices.iter().find(|c| c.allow).cloned(),
                    Answer::Deny | Answer::Submit(_) => choices.iter().find(|c| !c.allow).cloned(),
                };
                match chosen {
                    Some(choice) => {
                        if choice.allow && execute {
                            thread.grant = Some(ExecuteGrant {
                                at: Instant::now(),
                                command,
                            });
                        }
                        responder.ok(
                            json!({ "outcome": { "outcome": "selected", "optionId": choice.id } }),
                        );
                    }
                    None => {
                        responder.ok(json!({ "outcome": { "outcome": "cancelled" } }));
                    }
                }
            }
            (PendingKind::Command { .. }, Answer::Allow | Answer::Choose(_)) => {
                if let Some(launch) = item.launch.take() {
                    self.run_command(&agent, launch, responder, host);
                }
            }
            (PendingKind::Read { path }, Answer::Allow | Answer::Choose(_)) => {
                host.spawn(
                    async move { acp::read_path(&path, None, None).await },
                    move |_, result, _| {
                        responder.respond(result);
                    },
                )
                .detach();
            }
            (PendingKind::Write { path, new, .. }, Answer::Allow | Answer::Choose(_)) => {
                host.spawn(
                    async move { acp::write_text(&path, &new).await },
                    move |_, result, _| {
                        responder.respond(result);
                    },
                )
                .detach();
            }
            (PendingKind::Form { .. }, Answer::Submit(content)) => {
                responder.ok(elicitation::accept(content));
            }
            (PendingKind::OpenUrl { url, .. }, Answer::Allow | Answer::Choose(_)) => {
                host.effect(AgentEffect::OpenUrl(url));
                responder.ok(elicitation::accept(json!({})));
            }
            (PendingKind::Form { .. } | PendingKind::OpenUrl { .. }, _) => {
                responder.ok(elicitation::decline());
            }
            (_, Answer::Deny | Answer::Submit(_)) => {
                responder.err(RpcError::internal("The user declined this."));
            }
        }
        host.notify();
    }

    fn run_command(
        &mut self,
        agent: &str,
        launch: Launch,
        responder: Responder,
        host: &mut dyn Host<Self>,
    ) {
        let Some(connection) = self.connection(agent) else {
            responder.err(RpcError::internal("The agent stopped"));
            return;
        };
        let terminals = connection.terminals.clone();
        host.spawn(
            async move { terminals.spawn(launch).map(|(id, _)| id) },
            move |this, result, host| match result {
                Ok(id) => {
                    responder.ok(json!({ "terminalId": id }));
                    this.watch_terminals(host);
                }
                Err(err) => {
                    responder.err(RpcError::internal(format!(
                        "The command didn't start: {err}"
                    )));
                }
            },
        )
        .detach();
    }

    /// Re-renders twice a second while a command runs, so its output shows live.
    fn watch_terminals(&mut self, host: &mut dyn Host<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(host.every(Duration::from_millis(500), |this, host| {
            host.notify();
            if this.any_terminal_running() {
                Flow::Continue
            } else {
                this.ticker = None;
                Flow::Stop
            }
        }));
    }

    fn any_terminal_running(&self) -> bool {
        self.threads.iter().any(|t| {
            let Some(connection) = self.connection(&t.agent) else {
                return false;
            };
            t.transcript.entries.iter().any(|e| match e {
                Entry::Tool(tool) => tool.content.iter().any(|c| match c {
                    crate::thread::ToolContent::Terminal(id) => connection
                        .terminals
                        .get(id)
                        .is_some_and(|term| term.exit().is_none()),
                    _ => false,
                }),
                _ => false,
            })
        })
    }
}

/// Hooks for the panel's GPUI tests (`kubyl_agent`), which don't start agents.
#[cfg(any(test, feature = "test-support"))]
impl AgentCore {
    /// Marks `agent` as looked up: installed at `program`, or not installed.
    pub fn set_program_for_tests(&mut self, agent: &str, program: Option<PathBuf>) {
        self.assign_program(agent, program);
    }

    /// A thread whose session is open, without an agent process (prompts go nowhere).
    pub fn insert_ready_thread_for_tests(&mut self, agent: &str, cluster: ClusterId) -> ThreadId {
        self.next_thread += 1;
        let id = ThreadId(self.next_thread);
        self.contexts
            .entry(cluster.clone())
            .or_insert_with(|| watch::channel(ToolContext::default()).0);
        let dir = self
            .env
            .data_dir
            .join("threads")
            .join(format!("test-{}", id.0));
        self.threads.push(Thread {
            id,
            agent: agent.to_string(),
            agent_name: agent.to_string(),
            cluster: cluster.clone(),
            cluster_name: cluster.to_string(),
            cwd: dir.join("work"),
            dir,
            status: ThreadStatus::Idle,
            transcript: Transcript::default(),
            session: Some(format!("test-{}", id.0)),
            modes: Vec::new(),
            pending: Vec::new(),
            queued: Vec::new(),
            kubeconfig_source: None,
            resume: None,
            first_prompt: true,
            grant: None,
            mcp: None,
            tasks: Vec::new(),
            notes: Vec::new(),
        });
        id
    }

    pub fn set_status_for_tests(&mut self, thread: ThreadId, status: ThreadStatus) {
        if let Some(thread) = self.thread_mut(thread) {
            thread.status = status;
        }
    }

    /// The thread's transcript, to set what an agent would report (settings, usage).
    pub fn transcript_for_tests(&mut self, thread: ThreadId) -> Option<&mut Transcript> {
        self.thread_mut(thread).map(|t| &mut t.transcript)
    }

    /// Adds something waiting for the user to `thread` (nobody waits for the answer).
    pub fn push_pending_for_tests(&mut self, thread: ThreadId, kind: PendingKind) -> u64 {
        self.next_pending += 1;
        let id = self.next_pending;
        if let Some(thread) = self.thread_mut(thread) {
            thread.pending.push(Pending {
                id,
                kind,
                responder: None,
                launch: None,
            });
        }
        id
    }
}

fn answer_cancelled(mut pending: Pending) {
    let Some(responder) = pending.responder.take() else {
        return;
    };
    match pending.kind {
        PendingKind::Permission { .. } => {
            responder.ok(json!({ "outcome": { "outcome": "cancelled" } }));
        }
        PendingKind::Form { .. } | PendingKind::OpenUrl { .. } => {
            responder.ok(elicitation::cancel());
        }
        _ => {
            responder.err(RpcError::internal("Cancelled."));
        }
    }
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// The blocks of one prompt: Kubyl's context note (first prompt only), the attached chips
/// (embedded when the agent takes embedded context, else as links with their text inline), and
/// the user's text.
pub fn prompt_blocks(
    cluster: &str,
    first: bool,
    text: &str,
    chips: &[ContextChip],
    embedded: bool,
) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut inline = String::new();
    if first {
        let note = format!(
            "Kubyl context: this conversation is about the Kubernetes cluster \"{cluster}\". Read it with the `kubyl` MCP tools (start with cluster_info); they use the user's own access, are read-only, and mask Secret values and tokens. Don't try to reach the cluster another way, and propose changes as manifests or commands for the user to review."
        );
        if embedded {
            blocks.push(json!({
                "type": "resource",
                "resource": { "uri": "kubyl://context", "mimeType": "text/plain", "text": note },
            }));
        } else {
            inline.push_str(&note);
            inline.push_str("\n\n");
        }
    }
    for chip in chips {
        let masked = kubyl_resources_core::redact::scrub_text(&chip.text).into_owned();
        if embedded {
            blocks.push(json!({
                "type": "resource",
                "resource": { "uri": chip.uri, "mimeType": "text/plain", "text": masked },
            }));
        } else {
            inline.push_str(&format!(
                "<context name=\"{}\" uri=\"{}\">\n{masked}\n</context>\n\n",
                chip.label, chip.uri
            ));
        }
    }
    let text = if text.trim().is_empty() {
        "Look at the attached context and tell me what's wrong, if anything."
    } else {
        text
    };
    blocks.push(json!({ "type": "text", "text": format!("{inline}{text}") }));
    blocks
}

/// A short notice for a turn that ended while Kubyl wasn't focused.
pub fn turn_notice(title: &str) -> Notice {
    Notice::info(format!("The agent finished: {title}"))
}

#[cfg(test)]
mod tests {
    use kubyl_base::host::TestHost;

    use super::*;
    use crate::acp::fake::{self, Step};

    fn env(dir: &std::path::Path) -> AgentEnv {
        AgentEnv {
            data_dir: dir.join("data"),
            bridge: PathBuf::from("/usr/bin/false"),
            home: dir.to_path_buf(),
        }
    }

    struct Harness {
        core: AgentCore,
        host: TestHost<AgentCore>,
        // Keeps the fake agent's tasks alive.
        _runtime: tokio::runtime::Runtime,
        _dir: tempfile::TempDir,
    }

    fn harness(script: Vec<Step>) -> (Harness, ThreadId) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (connection, calls) = runtime.block_on(fake::start(script, true));
        let mut core = AgentCore::new(AgentSettings::default(), env(dir.path()));
        let mut host = TestHost::new();
        let id = core.start_thread(
            NewThread {
                agent: "fake".into(),
                cluster: ClusterId::new("kind-dev"),
                cluster_name: "kind-dev".into(),
                kubeconfig: None,
                context: ToolContext {
                    cluster_name: "kind-dev".into(),
                    ..ToolContext::default()
                },
                prompt: None,
                resume: None,
            },
            &mut host,
        );
        // The fake isn't in the agent list: attach it instead of starting a process.
        core.slots.clear();
        core.attach("fake", connection, calls, &mut host);
        host.run_until(&mut core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        (
            Harness {
                core,
                host,
                _runtime: runtime,
                _dir: dir,
            },
            id,
        )
    }

    fn last_agent_text(core: &AgentCore, id: ThreadId) -> String {
        core.thread(id)
            .unwrap()
            .transcript
            .entries
            .iter()
            .rev()
            .find_map(|e| match e {
                Entry::Agent { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn a_host_can_add_a_notice_to_a_transcript() {
        let (mut h, id) = harness(vec![]);
        let before = h.host.notified;
        h.core.notice(id, "Heads up", false, &mut h.host);
        h.core.notice(id, "Broke", true, &mut h.host);
        assert_eq!(h.host.notified, before + 2);
        let entries = &h.core.thread(id).unwrap().transcript.entries;
        assert!(matches!(
            &entries[entries.len() - 2..],
            [Entry::Notice { text: a, error: false }, Entry::Notice { text: b, error: true }]
                if a == "Heads up" && b == "Broke"
        ));
        // An unknown thread changes nothing and tells nobody.
        let before = h.host.notified;
        h.core.notice(ThreadId(999), "lost", false, &mut h.host);
        assert_eq!(h.host.notified, before);
    }

    #[test]
    fn a_host_can_set_where_an_agent_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let mut core = AgentCore::new(AgentSettings::default(), env(dir.path()));
        let mut host = TestHost::new();
        let program = PathBuf::from("/opt/host/claude-agent-acp");
        core.set_program("claude", Some(program.clone()), &mut host);
        let info = core.agent("claude").unwrap();
        assert_eq!(info.program.as_deref(), Some(program.as_path()));
        assert!(info.checked);
        assert_eq!(host.notified, 1);
        core.set_program("claude", None, &mut host);
        assert_eq!(core.agent("claude").unwrap().program, None);
        // An id that isn't an agent changes nothing.
        core.set_program("nope", Some(program), &mut host);
        assert_eq!(host.notified, 2);
        assert!(core.agent("nope").is_none());
    }

    #[test]
    fn a_thread_starts_prompts_and_reaches_kubyls_tools() {
        let (mut h, id) = harness(vec![
            Step::Say("Looking. ".into()),
            Step::Tool {
                name: "cluster_info".into(),
                arguments: json!({}),
            },
        ]);
        let thread = h.core.thread(id).unwrap();
        assert_eq!(thread.session.as_deref(), Some("s1"));
        assert_eq!(thread.modes.len(), 2);
        assert!(thread.cwd.is_dir(), "scratch folder exists");

        h.core
            .send(id, "what's up?".into(), Vec::new(), &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
                && last_agent_text(core, id).contains("tool")
        });
        let text = last_agent_text(&h.core, id);
        assert!(text.contains("Cluster: kind-dev"), "{text}");
        assert!(
            h.host
                .events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnded { .. }))
        );
        assert!(matches!(
            h.host.effects.last(),
            Some(AgentEffect::Remember(_))
        ));
        assert_eq!(h.core.thread(id).unwrap().title(), "what's up?");
    }

    #[test]
    fn permissions_wait_for_the_user_and_grant_the_command() {
        let (mut h, id) = harness(vec![
            Step::AskPermission {
                title: "Run tests".into(),
                command: "echo granted".into(),
            },
            Step::Run("echo granted".into()),
        ]);
        h.core.send(id, "run it".into(), Vec::new(), &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id).is_some_and(|t| !t.pending.is_empty())
        });
        let pending = &h.core.thread(id).unwrap().pending[0];
        let PendingKind::Permission {
            execute, command, ..
        } = &pending.kind
        else {
            panic!("{:?}", pending.kind);
        };
        assert!(*execute);
        assert_eq!(command.as_deref(), Some("echo granted"));
        let pending = pending.id;
        h.core
            .answer(id, pending, Answer::Choose("allow".into()), &mut h.host);
        // The command was covered by the grant: no second prompt.
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        let text = last_agent_text(&h.core, id);
        if cfg!(unix) {
            assert!(text.contains("output granted"), "{text}");
        }
    }

    #[test]
    fn agent_questions_wait_for_the_users_answers() {
        let (mut h, id) = harness(vec![
            Step::Ask(json!({
                "mode": "form", "message": "Which namespace?",
                "requestedSchema": {"type": "object", "properties": {
                    "namespace": {"type": "string", "minLength": 1}}, "required": ["namespace"]},
            })),
            Step::Ask(json!({
                "mode": "url", "message": "Sign in", "url": "https://example.com/login",
                "elicitationId": "e1",
            })),
        ]);
        h.core.send(id, "go".into(), Vec::new(), &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id).is_some_and(|t| !t.pending.is_empty())
        });
        let pending = &h.core.thread(id).unwrap().pending[0];
        let PendingKind::Form { fields, .. } = &pending.kind else {
            panic!("{:?}", pending.kind);
        };
        let mut inputs = std::collections::BTreeMap::new();
        inputs.insert(
            "namespace".to_string(),
            elicitation::Input::Text("shop".into()),
        );
        let content = elicitation::validate(fields, &inputs).unwrap();
        let pending = pending.id;
        h.core
            .answer(id, pending, Answer::Submit(content), &mut h.host);

        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id).is_some_and(|t| !t.pending.is_empty())
        });
        let pending = &h.core.thread(id).unwrap().pending[0];
        assert!(
            matches!(&pending.kind, PendingKind::OpenUrl { url, .. } if url == "https://example.com/login")
        );
        let pending = pending.id;
        h.core.answer(id, pending, Answer::Allow, &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        assert!(
            h.host
                .effects
                .contains(&AgentEffect::OpenUrl("https://example.com/login".into()))
        );
        let said: Vec<String> = h
            .core
            .thread(id)
            .unwrap()
            .transcript
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Agent { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        let said = said.join(" ");
        assert!(
            said.contains(r#""action":"accept""#) && said.contains(r#""namespace":"shop""#),
            "{said}"
        );
    }

    #[test]
    fn settings_and_context_use_come_from_the_agent() {
        let (mut h, id) = harness(vec![Step::Usage(150_000, 200_000), Step::Say("ok".into())]);
        let config = &h.core.thread(id).unwrap().transcript.config;
        assert_eq!(config.len(), 2);
        assert_eq!(config[0].current_label(), "Opus");
        assert_eq!(config[1].current_label(), "Medium");

        h.core.set_config(
            id,
            "effort".into(),
            crate::thread::ConfigValue::Select {
                current: "high".into(),
                choices: Vec::new(),
            },
            &mut h.host,
        );
        // Shown right away, then confirmed by the agent's answer.
        assert_eq!(
            h.core.thread(id).unwrap().transcript.config[1].current_label(),
            "High"
        );
        h.core.send(id, "go".into(), Vec::new(), &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        let transcript = &h.core.thread(id).unwrap().transcript;
        assert_eq!(transcript.config[1].current_label(), "High");
        assert_eq!(transcript.config[0].current_label(), "Opus");
        assert_eq!(transcript.usage_percent(), Some(75));
    }

    #[test]
    fn kubyls_own_tools_are_allowed_without_asking() {
        let (mut h, id) = harness(vec![Step::AskPermission {
            title: "mcp__kubyl__cluster_info".into(),
            command: String::new(),
        }]);
        h.core.send(id, "go".into(), Vec::new(), &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        assert!(h.core.thread(id).unwrap().pending.is_empty());
        assert!(last_agent_text(&h.core, id).contains("permission: allow"));
    }

    #[test]
    fn unapproved_commands_and_writes_ask_and_can_be_denied() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("out.yaml");
        let (mut h, id) = harness(vec![
            Step::Run("cat ~/.kube/config".into()),
            Step::Write {
                path: target.clone(),
                content: "kind: Pod\n".into(),
            },
        ]);
        h.core.send(id, "go".into(), Vec::new(), &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id).is_some_and(|t| !t.pending.is_empty())
        });
        let pending = &h.core.thread(id).unwrap().pending[0];
        let PendingKind::Command { warnings, .. } = &pending.kind else {
            panic!("{:?}", pending.kind);
        };
        assert!(!warnings.is_empty());
        let pending = pending.id;
        h.core.answer(id, pending, Answer::Deny, &mut h.host);

        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id).is_some_and(|t| !t.pending.is_empty())
        });
        let pending = &h.core.thread(id).unwrap().pending[0];
        assert!(matches!(
            &pending.kind,
            PendingKind::Write { old: None, .. }
        ));
        let pending = pending.id;
        h.core.answer(id, pending, Answer::Allow, &mut h.host);
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "kind: Pod\n");
        let entries = &h.core.thread(id).unwrap().transcript.entries;
        let said: Vec<String> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Agent { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(said.join(" ").contains("terminal refused"), "{said:?}");
        assert!(said.join(" ").contains("write: true"), "{said:?}");
    }

    #[test]
    fn prepared_threads_are_remembered_only_once_used() {
        let (mut h, id) = harness(vec![Step::Say("hi".into())]);
        assert!(h.core.thread(id).unwrap().is_unused());
        assert!(
            !h.host
                .effects
                .iter()
                .any(|e| matches!(e, AgentEffect::Remember(_)))
        );
        h.core.send(id, "first".into(), Vec::new(), &mut h.host);
        assert!(!h.core.thread(id).unwrap().is_unused());
        h.host.run_until(&mut h.core, |core, _| {
            core.thread(id)
                .is_some_and(|t| t.status == ThreadStatus::Idle)
        });
        assert!(
            h.host
                .effects
                .iter()
                .any(|e| matches!(e, AgentEffect::Remember(_)))
        );
    }

    #[test]
    fn closing_a_thread_removes_its_folder_and_stops_the_agent() {
        let (mut h, id) = harness(vec![Step::Say("hi".into())]);
        let dir = h.core.thread(id).unwrap().dir.clone();
        assert!(dir.is_dir());
        h.core.close_thread(id, &mut h.host);
        h.host.run_until_idle(&mut h.core);
        assert!(!dir.exists());
        assert!(h.core.threads().is_empty());
        assert!(h.core.slots.is_empty());
    }

    #[test]
    fn prompts_carry_the_context_note_once_and_scrub_chips() {
        let chip = ContextChip {
            label: "Pod shop/web-0".into(),
            uri: "kubyl://kind/pods/shop/web-0".into(),
            text: "env TOKEN=Bearer abcdefghijklmnop".into(),
        };
        let first = prompt_blocks("kind-dev", true, "why?", std::slice::from_ref(&chip), true);
        assert_eq!(first.len(), 3);
        assert_eq!(first[0]["resource"]["uri"], "kubyl://context");
        assert!(
            !first[1]["resource"]["text"]
                .as_str()
                .unwrap()
                .contains("abcdefghijklmnop")
        );
        let later = prompt_blocks("kind-dev", false, "", &[chip], false);
        assert_eq!(later.len(), 1);
        let text = later[0]["text"].as_str().unwrap();
        assert!(text.contains("<context name=\"Pod shop/web-0\""));
        assert!(text.contains("tell me what's wrong"));
    }
}
