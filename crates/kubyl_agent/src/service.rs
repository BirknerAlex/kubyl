//! [`AgentService`]: the app's [`AgentCore`] in an entity. It keeps each cluster's tool context
//! current (connection, selection, Prometheus, alerts), saves reopenable threads to state.json
//! and shows a toast when a turn ends while Kubyl isn't focused.

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use kubyl_alerts::AlertsService;
use kubyl_alerts::model::Alert;
use kubyl_core::host::{Hosts, hosted};
use kubyl_core::{ActiveContext, ClusterId, Notification, NotificationCenter};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_metrics::MetricsService;
use kubyl_resources::ResourceSelection;
use kubyl_settings::{Settings, State};

pub use kubyl_agent_core::service::*;
use kubyl_agent_core::settings::{AgentSettings, AgentState, SavedThread};
use kubyl_agent_core::thread::ContextChip;
use kubyl_agent_core::tools::{AlertSummary, ToolContext};

/// How often the tool contexts pick up new alerts and a found Prometheus.
const REFRESH: Duration = Duration::from_secs(5);

pub struct AgentService {
    core: AgentCore,
    _subscriptions: Vec<Subscription>,
    _refresh: Option<Task<()>>,
}

impl Deref for AgentService {
    type Target = AgentCore;

    fn deref(&self) -> &AgentCore {
        &self.core
    }
}

impl EventEmitter<AgentEvent> for AgentService {}

impl Hosts<AgentCore> for AgentService {
    fn service(&mut self) -> &mut AgentCore {
        &mut self.core
    }

    fn apply(&mut self, effect: AgentEffect, cx: &mut Context<Self>) {
        match effect {
            AgentEffect::Remember(thread) => {
                State::update::<AgentState>(cx, |state| state.remember(thread));
            }
            AgentEffect::OpenUrl(url) => cx.open_url(&url),
        }
    }
}

struct GlobalService(Entity<AgentService>);

impl Global for GlobalService {}

/// Where agent files live: `<cache dir>/kubyl/agent` (scratch folders, kubeconfigs).
fn data_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("kubyl")
        .join("agent")
}

impl AgentService {
    /// Creates the service and makes it global. `live` looks up agents and refreshes contexts
    /// (off in GPUI tests, which must not start threads that wake GPUI).
    pub fn install(live: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(|cx| Self::new(live, cx));
        cx.set_global(GlobalService(service.clone()));
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    fn new(live: bool, cx: &mut Context<Self>) -> Self {
        let env = AgentEnv {
            data_dir: data_dir(),
            bridge: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kubyl")),
            home: dirs::home_dir().unwrap_or_else(std::env::temp_dir),
        };
        let mut subscriptions = Vec::new();
        let weak = cx.weak_entity();
        subscriptions.push(Settings::observe::<AgentSettings>(
            cx,
            move |settings, cx| {
                let settings = settings.clone();
                weak.update(cx, |this, cx| {
                    hosted(this, cx, |core, host| core.settings_changed(settings, host));
                })
                .ok();
            },
        ));
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |this, _, event, cx| match event {
                ConnectionEvent::StateChanged(id)
                | ConnectionEvent::DiscoveryChanged(id)
                | ConnectionEvent::NamespacesChanged(id) => this.refresh_context(id, cx),
                ConnectionEvent::ContextsChanged | ConnectionEvent::Rekeyed { .. } => {
                    this.refresh_contexts(cx)
                }
                _ => {}
            }));
        }
        subscriptions
            .push(cx.observe_global::<ResourceSelection>(|this, cx| this.refresh_contexts(cx)));
        subscriptions
            .push(cx.observe_global::<ActiveContext>(|this, cx| this.refresh_contexts(cx)));
        let mut this = Self {
            core: AgentCore::new(Settings::get::<AgentSettings>(cx).clone(), env),
            _subscriptions: subscriptions,
            _refresh: None,
        };
        if live {
            hosted(&mut this, cx, |core, host| core.refresh_agents(host));
            this._refresh = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(REFRESH).await;
                    if this
                        .update(cx, |this, cx| this.refresh_contexts(cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        this
    }

    fn refresh_context(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        if self.core.clusters().contains(cluster) {
            let context = tool_context(cluster, cx);
            self.core.set_context(cluster, context);
        }
    }

    fn refresh_contexts(&mut self, cx: &mut Context<Self>) {
        for cluster in self.core.clusters() {
            let context = tool_context(&cluster, cx);
            self.core.set_context(&cluster, context);
        }
    }

    // ----- Threads -----

    /// Starts a thread about `cluster` with `agent` (default: the configured or last one).
    pub fn new_thread(
        &mut self,
        cluster: &ClusterId,
        agent: Option<String>,
        prompt: Option<(String, Vec<ContextChip>)>,
        cx: &mut Context<Self>,
    ) -> Option<ThreadId> {
        let last = State::get::<AgentState>(cx).last_agent;
        let agent = agent
            .or_else(|| {
                self.core
                    .default_agent(last.as_deref())
                    .map(|a| a.spec.id.clone())
            })
            .or_else(|| self.core.agents().first().map(|a| a.spec.id.clone()))?;
        State::update::<AgentState>(cx, |state| state.last_agent = Some(agent.clone()));
        let (kubeconfig, cluster_name) = match ConnectionManager::try_global(cx) {
            Some(manager) => {
                let manager = manager.read(cx);
                (
                    manager
                        .cli_target(cluster)
                        .map(|t| (t.kubeconfig, t.context)),
                    manager.display_name(cluster).to_string(),
                )
            }
            None => (None, cluster.to_string()),
        };
        let new = NewThread {
            agent,
            cluster: cluster.clone(),
            cluster_name,
            kubeconfig,
            context: tool_context(cluster, cx),
            prompt,
            resume: None,
        };
        Some(hosted(self, cx, |core, host| core.start_thread(new, host)))
    }

    /// Reopens a saved thread from the agent's own storage.
    pub fn reopen(&mut self, saved: SavedThread, cx: &mut Context<Self>) -> ThreadId {
        let (cluster, kubeconfig, cluster_name) = match ConnectionManager::try_global(cx) {
            Some(manager) => {
                let manager = manager.read(cx);
                let cluster = manager.resolve(&saved.cluster);
                (
                    cluster.clone(),
                    manager
                        .cli_target(&cluster)
                        .map(|t| (t.kubeconfig, t.context)),
                    manager.display_name(&cluster).to_string(),
                )
            }
            None => (saved.cluster.clone(), None, saved.cluster.to_string()),
        };
        let new = NewThread {
            agent: saved.agent.clone(),
            cluster: cluster.clone(),
            cluster_name,
            kubeconfig,
            context: tool_context(&cluster, cx),
            prompt: None,
            resume: Some(saved),
        };
        hosted(self, cx, |core, host| core.start_thread(new, host))
    }

    pub fn forget(&mut self, saved: &SavedThread, cx: &mut Context<Self>) {
        State::update::<AgentState>(cx, |state| state.forget(&saved.agent, &saved.session));
        cx.notify();
    }

    pub fn send(
        &mut self,
        thread: ThreadId,
        text: String,
        chips: Vec<ContextChip>,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| core.send(thread, text, chips, host));
    }

    pub fn cancel(&mut self, thread: ThreadId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.cancel(thread, host));
    }

    pub fn close(&mut self, thread: ThreadId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.close_thread(thread, host));
    }

    pub fn retry(&mut self, thread: ThreadId, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.retry(thread, host));
    }

    pub fn answer(
        &mut self,
        thread: ThreadId,
        pending: u64,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.answer(thread, pending, answer, host)
        });
    }

    pub fn set_mode(&mut self, thread: ThreadId, mode: String, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.set_mode(thread, mode, host));
    }

    pub fn set_config(
        &mut self,
        thread: ThreadId,
        config: String,
        value: kubyl_agent_core::thread::ConfigValue,
        cx: &mut Context<Self>,
    ) {
        hosted(self, cx, |core, host| {
            core.set_config(thread, config, value, host)
        });
    }

    pub fn sign_in(&mut self, thread: ThreadId, method: String, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.sign_in(thread, method, host));
    }

    /// The core, for tests that set up agents and pending answers.
    #[cfg(test)]
    pub(crate) fn core_mut(&mut self) -> &mut AgentCore {
        &mut self.core
    }

    pub fn refresh_agents(&mut self, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.refresh_agents(host));
    }
}

/// Pushes the notice for a turn that ended while no Kubyl window is active.
pub(crate) fn notify_turn(title: &str, cx: &mut App) {
    if cx.active_window().is_none() {
        NotificationCenter::push(cx, Notification::from(turn_notice(title)));
    }
}

fn alert_summary(alert: &Alert) -> AlertSummary {
    AlertSummary {
        name: alert.name.clone(),
        severity: alert.severity.label().to_string(),
        state: alert.state.label().to_string(),
        since: alert.since(),
        summary: alert.summary().to_string(),
        labels: alert.labels.clone(),
    }
}

/// What `cluster`'s tools can reach right now.
pub fn tool_context(cluster: &ClusterId, cx: &App) -> ToolContext {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return ToolContext::default();
    };
    let manager = manager.read(cx);
    let selection = ResourceSelection::global(cx)
        .primary()
        .filter(|s| &s.target.cluster == cluster);
    let active = ActiveContext::global(cx);
    let namespace = active
        .cluster
        .as_ref()
        .filter(|c| &c.id == cluster)
        .and_then(|_| active.namespace.as_ref().map(|n| n.to_string()));
    let prometheus = MetricsService::global(cx).and_then(|m| m.read(cx).prometheus(cluster));
    let alerts = AlertsService::global(cx).and_then(|service| {
        let service = service.read(cx);
        let state = service.cluster(cluster, kubyl_alerts::service::Pace::Background)?;
        Some(Arc::new(state.alerts.iter().map(alert_summary).collect()))
    });
    ToolContext {
        cluster: Some(cluster.clone()),
        cluster_name: manager.display_name(cluster).to_string(),
        client: manager.client(cluster),
        discovery: manager.discovery(cluster),
        caps: manager.caps(cluster),
        namespace,
        selection: selection.map(|s| s.target.clone()),
        selection_kind: selection.map(|s| s.kind.clone()),
        prometheus,
        alerts,
        max_output: 0,
        log_lines: 0,
    }
}
