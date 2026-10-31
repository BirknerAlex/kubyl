//! [`McpService`]: the app's [`McpHub`] in an entity. It keeps the clusters the tools reach
//! current (connections, selection, alerts) and runs the server while settings allow it.

use std::ops::Deref;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use kubyl_core::ActiveContext;
use kubyl_core::host::{Hosts, hosted};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_mcp_core::approval::Decision;
use kubyl_mcp_core::hub::{HubEvent, McpHub};
use kubyl_mcp_core::settings::McpSettings;
use kubyl_resources::ResourceSelection;
use kubyl_settings::Settings;

use crate::context::all_tool_contexts;

/// How often the tools pick up new alerts and a found Prometheus.
const REFRESH: Duration = Duration::from_secs(5);

pub struct McpService {
    core: McpHub,
    _subscriptions: Vec<Subscription>,
    _refresh: Option<Task<()>>,
}

impl Deref for McpService {
    type Target = McpHub;

    fn deref(&self) -> &McpHub {
        &self.core
    }
}

impl EventEmitter<HubEvent> for McpService {}

impl Hosts<McpHub> for McpService {
    fn service(&mut self) -> &mut McpHub {
        &mut self.core
    }

    fn apply(&mut self, (): (), _: &mut Context<Self>) {}
}

struct GlobalService(Entity<McpService>);

impl Global for GlobalService {}

impl McpService {
    /// Creates the service and makes it global. `live` starts the server and the refresh (off
    /// in GPUI tests, which must not open sockets or wake GPUI from threads).
    pub fn install(live: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(|cx| Self::new(live, cx));
        cx.set_global(GlobalService(service.clone()));
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    fn new(live: bool, cx: &mut Context<Self>) -> Self {
        let credentials = ConnectionManager::try_global(cx)
            .map(|manager| manager.read(cx).secrets().clone())
            .unwrap_or_default();
        let mut subscriptions = Vec::new();
        let weak = cx.weak_entity();
        subscriptions.push(Settings::observe::<McpSettings>(cx, move |settings, cx| {
            let settings = settings.clone();
            weak.update(cx, |this, cx| {
                hosted(this, cx, |core, host| core.settings_changed(settings, host));
                this.refresh(cx);
            })
            .ok();
        }));
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |this, _, event, cx| {
                if !matches!(event, ConnectionEvent::Rekeyed { .. }) {
                    this.refresh(cx);
                }
            }));
        }
        subscriptions.push(cx.observe_global::<ResourceSelection>(|this, cx| this.refresh(cx)));
        subscriptions.push(cx.observe_global::<ActiveContext>(|this, cx| this.refresh(cx)));
        let mut this = Self {
            core: McpHub::new(Settings::get::<McpSettings>(cx).clone(), credentials),
            _subscriptions: subscriptions,
            _refresh: None,
        };
        if live {
            this.refresh(cx);
            hosted(&mut this, cx, |core, host| core.start(host));
            this._refresh = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(REFRESH).await;
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        this
    }

    /// Hands the server the current state of every cluster (only while it is on).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.core.settings().enabled {
            let contexts = all_tool_contexts(cx);
            self.core.set_clusters(contexts);
        }
    }

    /// The user's answer to a change an outside client proposed.
    pub fn answer(&mut self, id: u64, decision: Decision, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.answer(id, decision, host));
    }

    pub fn regenerate_token(&mut self, cx: &mut Context<Self>) {
        hosted(self, cx, |core, host| core.regenerate_token(host));
    }
}
