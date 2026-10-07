//! The Agent panel of the right dock (board 19): threads with the user's agent about one
//! cluster, what it asks for, and the composer.

use std::collections::{BTreeMap, HashMap, HashSet};

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle,
    Focusable, Global, Hsla, IntoElement, Render, ScrollHandle, SharedString, Subscription,
    WeakEntity, Window, WindowId, div, prelude::*, px,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::text::TextView;
use kubyl_agent_core::elicitation::{self, Field, FieldKind};
use kubyl_agent_core::mentions;
use kubyl_agent_core::service::{AgentInfo, Answer, PendingKind, Thread, ThreadId, ThreadStatus};
use kubyl_agent_core::settings::AgentState;
use kubyl_agent_core::thread::{ConfigCategory, ConfigOption, ConfigValue};
use kubyl_agent_core::thread::{ContextChip, Entry, ToolContent, ToolEntry, ToolStatus};
use kubyl_core::actions::ActivateDockPanel;
use kubyl_core::{
    ActiveContext, ClusterId, DockPanel, DockPosition, Notification, NotificationCenter, TabHandle,
    TabView,
};
use kubyl_kube::ConnectionManager;
use kubyl_settings::State;
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, DockHeader, Icon, IconButton, IconName, h_flex, u, v_flex,
};

use crate::service::AgentService;

/// [`DockPanel::id`] of the panel, for [`ActivateDockPanel`].
pub const PANEL_ID: &str = "agent";
/// Lines of a tool's text output shown before "Show all".
const TOOL_LINES: usize = 12;

/// The panel of each window.
#[derive(Default)]
struct Panels(HashMap<WindowId, WeakEntity<AgentPanel>>);

impl Global for Panels {}

pub struct AgentDockPanel;

impl DockPanel for AgentDockPanel {
    fn id(&self) -> &'static str {
        PANEL_ID
    }

    fn position(&self) -> DockPosition {
        DockPosition::Right
    }

    fn order(&self) -> i32 {
        30
    }

    fn build(&self, window: &mut Window, cx: &mut App) -> Box<dyn TabHandle> {
        let panel = cx.new(|cx| AgentPanel::new(window, cx));
        let id = window.window_handle().window_id();
        cx.default_global::<Panels>()
            .0
            .insert(id, panel.downgrade());
        Box::new(panel)
    }
}

/// The panel of `window`, if the window has one.
pub fn panel_for(window: AnyWindowHandle, cx: &App) -> Option<Entity<AgentPanel>> {
    cx.try_global::<Panels>()?
        .0
        .get(&window.window_id())?
        .upgrade()
}

/// Shows the panel of `window` and focuses its composer.
pub fn show(window: &mut Window, cx: &mut App) -> Option<Entity<AgentPanel>> {
    let panel = panel_for(window.window_handle(), cx)?;
    window.dispatch_action(Box::new(ActivateDockPanel(PANEL_ID.into())), cx);
    panel.update(cx, |panel, cx| panel.focus_input(window, cx));
    Some(panel)
}

pub struct AgentPanel {
    focus: FocusHandle,
    input: Entity<TextareaState>,
    /// The thread shown.
    thread: Option<ThreadId>,
    /// Attachments for the next prompt.
    chips: Vec<ContextChip>,
    /// The cluster a new thread is about (when no thread is shown).
    draft_cluster: Option<ClusterId>,
    /// The agent picked for a new thread.
    draft_agent: Option<String>,
    show_threads: bool,
    scroll: ScrollHandle,
    /// Entries with their full text shown (thoughts, long tool output).
    expanded: HashSet<(u64, usize)>,
    entry_count: usize,
    /// Inputs of agent questions (forms), by pending id.
    forms: HashMap<u64, FormState>,
    /// Objects offered for the `@` mention being typed.
    mentions: Vec<Mention>,
    /// A thread started for the new-thread composer, before its first message, so the agent's
    /// settings (model, thinking level, mode) can be picked up front. Reused on send.
    draft: Option<ThreadId>,
    /// The panel was shown: preparing a thread may start an agent.
    drafting: bool,
    _subscriptions: Vec<Subscription>,
}

/// Whether the composer can send.
#[derive(Clone, Debug, PartialEq)]
enum Readiness {
    Ready,
    /// The agent or its session is starting (shown above the input).
    Starting(String),
    /// No cluster, no installed agent, or the agent failed (the panel says why).
    Blocked,
}

/// An object offered for an `@` mention.
#[derive(Clone)]
struct Mention {
    candidate: mentions::Candidate,
    target: kubyl_core::ResourceRef,
    object: Option<std::sync::Arc<serde_json::Value>>,
}

/// Mentions offered at most.
const MENTIONS: usize = 8;

/// What the user entered in one agent question so far.
#[derive(Default)]
struct FormState {
    inputs: HashMap<String, Entity<InputState>>,
    selected: HashMap<String, Vec<String>>,
    bools: HashMap<String, bool>,
    errors: BTreeMap<String, String>,
}

impl AgentPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 10)
                .submit_on_enter(true)
                .placeholder("Ask about this cluster…  (@ mentions an object, Enter sends)")
        });
        let mut subscriptions =
            vec![
                cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { shift: false, .. } => this.submit(window, cx),
                        InputEvent::Change => this.update_mentions(cx),
                        InputEvent::Focus => {
                            this.drafting = true;
                            this.ensure_draft(cx);
                        }
                        _ => {}
                    }
                }),
            ];
        if let Some(service) = AgentService::global(cx) {
            subscriptions.push(cx.observe(&service, |this, _, cx| this.service_changed(cx)));
            subscriptions
                .push(cx.observe_global::<ActiveContext>(|this, cx| this.ensure_draft(cx)));
        }
        Self {
            focus: cx.focus_handle(),
            input,
            thread: None,
            chips: Vec::new(),
            draft_cluster: None,
            draft_agent: None,
            show_threads: false,
            scroll: ScrollHandle::new(),
            expanded: HashSet::new(),
            entry_count: 0,
            forms: HashMap::new(),
            mentions: Vec::new(),
            draft: None,
            drafting: false,
            _subscriptions: subscriptions,
        }
    }

    fn service_changed(&mut self, cx: &mut Context<Self>) {
        let Some(service) = AgentService::global(cx) else {
            return;
        };
        let service = service.read(cx);
        if let Some(id) = self.thread
            && service.thread(id).is_none()
        {
            self.thread = None;
        }
        // Follow new entries when the view sits at (or near) the bottom.
        let count = self
            .thread
            .and_then(|id| service.thread(id))
            .map(|t| t.transcript.entries.len() + t.pending.len())
            .unwrap_or(0);
        let offset = self.scroll.offset().y;
        let max = self.scroll.max_offset().y;
        let near_bottom = (max + offset).abs() < px(80.0);
        if count != self.entry_count || near_bottom {
            self.scroll.scroll_to_bottom();
        }
        self.entry_count = count;
        if self.draft.is_some_and(|id| service.thread(id).is_none()) {
            self.draft = None;
        }
        self.ensure_draft(cx);
        cx.notify();
    }

    /// The agent a new thread would use: the picked one, else the default (installed) one.
    fn draft_agent_id(&self, cx: &App) -> Option<String> {
        let service = AgentService::global(cx)?;
        let service = service.read(cx);
        match &self.draft_agent {
            Some(id) => service
                .agent(id)
                .filter(|a| a.program.is_some())
                .map(|a| a.spec.id.clone()),
            None => {
                let last = State::get::<AgentState>(cx).last_agent;
                service
                    .default_agent(last.as_deref())
                    .map(|a| a.spec.id.clone())
            }
        }
    }

    /// Keeps a thread prepared for the new-thread composer (agent and session started, no
    /// prompt sent, so nothing is spent), matching the picked agent and cluster.
    fn ensure_draft(&mut self, cx: &mut Context<Self>) {
        if !self.drafting || self.thread.is_some() {
            return;
        }
        let (Some(cluster), Some(agent), Some(service)) = (
            self.draft_cluster(cx),
            self.draft_agent_id(cx),
            AgentService::global(cx),
        ) else {
            self.drop_draft(cx);
            return;
        };
        let matches = self
            .draft
            .and_then(|id| service.read(cx).thread(id))
            .is_some_and(|t| t.cluster == cluster && t.agent == agent && t.is_unused());
        if matches {
            return;
        }
        self.drop_draft(cx);
        self.draft = service.update(cx, |s, cx| s.new_thread(&cluster, Some(agent), None, cx));
    }

    /// Whether the composer can send, and what to say while it can't.
    fn readiness(&self, cx: &App) -> Readiness {
        let Some(service) = AgentService::global(cx) else {
            return Readiness::Blocked;
        };
        let service = service.read(cx);
        if self.thread.is_some() {
            return Readiness::Ready;
        }
        if self.draft_cluster(cx).is_none() {
            return Readiness::Blocked;
        }
        if let Some(draft) = self.draft.and_then(|id| service.thread(id)) {
            return match draft.status {
                ThreadStatus::Idle | ThreadStatus::Running => Readiness::Ready,
                ThreadStatus::Starting => {
                    Readiness::Starting(format!("Starting {}…", draft.agent_name))
                }
                _ => Readiness::Blocked,
            };
        }
        if service.agents().iter().any(|a| !a.checked) {
            return Readiness::Starting("Looking for agents…".into());
        }
        match self
            .draft_agent_id(cx)
            .and_then(|id| service.agent(&id).map(|a| a.spec.name.clone()))
        {
            Some(name) => Readiness::Starting(format!("Starting {name}…")),
            None => Readiness::Blocked,
        }
    }

    /// Closes the prepared thread unless something was sent in it.
    fn drop_draft(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.draft.take() else {
            return;
        };
        let Some(service) = AgentService::global(cx) else {
            return;
        };
        if service.read(cx).thread(id).is_some_and(|t| t.is_unused()) {
            service.update(cx, |s, cx| s.close(id, cx));
        }
    }

    pub fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_threads = false;
        self.drafting = true;
        self.ensure_draft(cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Attaches `chip` for `cluster`: to the shown thread when it's about that cluster, else to
    /// a new thread.
    pub fn attach(&mut self, cluster: ClusterId, chip: ContextChip, cx: &mut Context<Self>) {
        let same_cluster = self
            .thread
            .and_then(|id| {
                AgentService::global(cx)?
                    .read(cx)
                    .thread(id)
                    .map(|t| t.cluster.clone())
            })
            .is_some_and(|c| c == cluster);
        if !same_cluster {
            self.thread = None;
            self.chips.clear();
            self.draft_cluster = Some(cluster);
            self.drafting = true;
            self.ensure_draft(cx);
        }
        if !self
            .chips
            .iter()
            .any(|c| c.uri == chip.uri && c.text == chip.text)
        {
            self.chips.push(chip);
        }
        cx.notify();
    }

    /// Shows `thread` scrolled to its end, where waiting prompts are.
    pub fn show_thread(&mut self, thread: ThreadId, cx: &mut Context<Self>) {
        if self.thread != Some(thread) {
            self.chips.clear();
            self.mentions.clear();
        }
        self.thread = Some(thread);
        self.show_threads = false;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// Starts a new thread composer for `cluster` (the active one when `None`).
    pub fn new_thread(&mut self, cluster: Option<ClusterId>, cx: &mut Context<Self>) {
        self.thread = None;
        self.chips.clear();
        self.draft_cluster = cluster;
        self.show_threads = false;
        self.drafting = true;
        self.ensure_draft(cx);
        cx.notify();
    }

    fn draft_cluster(&self, cx: &App) -> Option<ClusterId> {
        self.draft_cluster.clone().or_else(|| {
            ActiveContext::global(cx)
                .cluster
                .as_ref()
                .map(|c| c.id.clone())
        })
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Enter while mentions are offered picks the first one.
        if !self.mentions.is_empty() {
            self.pick_mention(0, window, cx);
            return;
        }
        if self.readiness(cx) != Readiness::Ready {
            return;
        }
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() && self.chips.is_empty() {
            return;
        }
        let Some(service) = AgentService::global(cx) else {
            return;
        };
        let chips = std::mem::take(&mut self.chips);
        match self.thread {
            Some(thread) => {
                service.update(cx, |s, cx| s.send(thread, text, chips, cx));
            }
            None => {
                let Some(cluster) = self.draft_cluster(cx) else {
                    NotificationCenter::push(
                        cx,
                        Notification::warning(
                            "Pick a cluster first: the agent works on one cluster.",
                        ),
                    );
                    self.chips = chips;
                    return;
                };
                let agent = self.draft_agent_id(cx).or_else(|| self.draft_agent.clone());
                let prepared = self.draft.filter(|id| {
                    service.read(cx).thread(*id).is_some_and(|t| {
                        t.cluster == cluster && Some(&t.agent) == agent.as_ref() && t.is_unused()
                    })
                });
                self.thread = match prepared {
                    Some(id) => {
                        self.draft = None;
                        service.update(cx, |s, cx| s.send(id, text, chips, cx));
                        Some(id)
                    }
                    None => service.update(cx, |s, cx| {
                        s.new_thread(&cluster, agent, Some((text, chips)), cx)
                    }),
                };
                self.draft_cluster = None;
            }
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn colors(cx: &App) -> Colors {
        cx.colors().clone()
    }
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for AgentPanel {
    fn tab_title(&self, _: &App) -> SharedString {
        "Agent".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Zap.path())
    }
}

fn cluster_color(cluster: &ClusterId, cx: &App) -> Hsla {
    ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).color(cluster, cx))
        .unwrap_or_else(|| cx.colors().accent)
}

fn small(text: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .text_size(u(11.5))
        .text_color(color)
        .child(text.into())
}

fn mono_block(text: String, colors: &Colors) -> impl IntoElement {
    div()
        .w_full()
        .px(u(8.0))
        .py(u(6.0))
        .rounded(u(4.0))
        .bg(colors.background)
        .border_1()
        .border_color(colors.border_variant)
        .font_family(kubyl_ui::fonts::MONO)
        .text_size(u(11.5))
        .text_color(colors.text_muted)
        .whitespace_normal()
        .children(
            text.lines()
                .map(|l| div().child(l.to_string()))
                .collect::<Vec<_>>(),
        )
}

fn copy_button(id: impl Into<gpui::ElementId>, text: String) -> impl IntoElement {
    IconButton::new(id, IconName::Copy)
        .icon_size(12.0)
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            NotificationCenter::push(cx, Notification::info("Copied."));
        })
}

impl AgentPanel {
    fn render_header(&self, weak: &WeakEntity<Self>) -> impl IntoElement {
        let weak = weak.clone();
        let weak2 = weak.clone();
        let weak3 = weak.clone();
        let has_thread = self.thread.is_some();
        DockHeader::new("Agent")
            .end_child(
                IconButton::new("agent-threads", IconName::History)
                    .toggled(self.show_threads)
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.show_threads = !this.show_threads;
                            cx.notify();
                        })
                        .ok();
                    }),
            )
            .end_child(
                IconButton::new("agent-new", IconName::Plus).on_click(move |_, _, cx| {
                    weak2.update(cx, |this, cx| this.new_thread(None, cx)).ok();
                }),
            )
            .end_child(
                IconButton::new("agent-close", IconName::Trash).on_click(move |_, _, cx| {
                    if !has_thread {
                        return;
                    }
                    weak3
                        .update(cx, |this, cx| {
                            if let (Some(id), Some(service)) =
                                (this.thread.take(), AgentService::global(cx))
                            {
                                service.update(cx, |s, cx| s.close(id, cx));
                            }
                            cx.notify();
                        })
                        .ok();
                }),
            )
    }

    fn render_threads(&self, weak: &WeakEntity<Self>, cx: &App) -> AnyElement {
        let colors = Self::colors(cx);
        let Some(service) = AgentService::global(cx) else {
            return div().into_any_element();
        };
        let service = service.read(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(section_title("Open threads", &colors));
        if service.threads().is_empty() {
            rows.push(
                div()
                    .px(u(14.0))
                    .py(u(6.0))
                    .child(small("None yet.", colors.text_dim))
                    .into_any_element(),
            );
        }
        for thread in service.threads().iter().rev().filter(|t| !t.is_unused()) {
            let id = thread.id;
            let weak = weak.clone();
            let selected = self.thread == Some(id);
            let dot = cluster_color(&thread.cluster, cx);
            rows.push(
                h_flex()
                    .id(("agent-thread", id.0))
                    .gap(u(8.0))
                    .px(u(14.0))
                    .py(u(6.0))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(colors.selection))
                    .hover(|this| this.bg(colors.hover))
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.thread = Some(id);
                            this.show_threads = false;
                            this.chips.clear();
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(div().size(u(7.0)).rounded_full().bg(dot).flex_none())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(u(12.5)).truncate().child(thread.title()))
                            .child(small(
                                format!(
                                    "{} · {} · {}",
                                    thread.agent_name,
                                    thread.cluster_name,
                                    status_label(&thread.status)
                                ),
                                colors.text_dim,
                            )),
                    )
                    .when(!thread.pending.is_empty(), |this| {
                        this.child(
                            Icon::new(IconName::TriangleAlert)
                                .size(12.0)
                                .color(colors.yellow),
                        )
                    })
                    .into_any_element(),
            );
        }
        let saved = State::get::<AgentState>(cx).threads;
        let open_sessions: HashSet<String> = service
            .threads()
            .iter()
            .filter_map(|t| t.session.clone())
            .collect();
        let saved: Vec<_> = saved
            .into_iter()
            .filter(|s| !open_sessions.contains(&s.session))
            .collect();
        if !saved.is_empty() {
            rows.push(section_title(
                "Earlier (reopened from the agent's own history)",
                &colors,
            ));
            for (i, thread) in saved.into_iter().enumerate() {
                let weak = weak.clone();
                let forget = thread.clone();
                let agent_name = service
                    .agent(&thread.agent)
                    .map(|a| a.spec.name.clone())
                    .unwrap_or_else(|| thread.agent.clone());
                let when = jiff::Timestamp::from_second(thread.updated)
                    .map(|t| {
                        kubyl_resources::format::human_duration(
                            kubyl_resources::format::seconds_since(t, jiff::Timestamp::now()),
                        )
                    })
                    .unwrap_or_default();
                let cluster_name = ConnectionManager::try_global(cx)
                    .map(|m| m.read(cx).display_name(&thread.cluster).to_string())
                    .unwrap_or_else(|| thread.cluster.to_string());
                rows.push(
                    h_flex()
                        .id(("agent-saved", i))
                        .gap(u(8.0))
                        .px(u(14.0))
                        .py(u(6.0))
                        .cursor_pointer()
                        .hover(|this| this.bg(colors.hover))
                        .on_click(move |_, _, cx| {
                            let saved = thread.clone();
                            weak.update(cx, |this, cx| {
                                if let Some(service) = AgentService::global(cx) {
                                    this.thread =
                                        Some(service.update(cx, |s, cx| s.reopen(saved, cx)));
                                }
                                this.show_threads = false;
                                cx.notify();
                            })
                            .ok();
                        })
                        .child(
                            Icon::new(IconName::History)
                                .size(12.0)
                                .color(colors.text_dim),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(u(12.5))
                                        .truncate()
                                        .child(forget.title.clone()),
                                )
                                .child(small(
                                    format!("{agent_name} · {cluster_name} · {when} ago"),
                                    colors.text_dim,
                                )),
                        )
                        .child(
                            IconButton::new(("agent-forget", i), IconName::X)
                                .icon_size(11.0)
                                .on_click(move |_, _, cx| {
                                    if let Some(service) = AgentService::global(cx) {
                                        let forget = forget.clone();
                                        service.update(cx, |s, cx| s.forget(&forget, cx));
                                    }
                                }),
                        )
                        .into_any_element(),
                );
            }
        }
        rows.push(section_title("Agents", &colors));
        for (i, agent) in service.agents().iter().enumerate() {
            rows.push(agent_row(
                i,
                agent,
                self.draft_agent.as_deref(),
                false,
                weak,
                cx,
            ));
        }
        div()
            .id("agent-thread-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(u(4.0))
            .children(rows)
            .into_any_element()
    }

    fn render_welcome(&self, weak: &WeakEntity<Self>, cx: &App) -> AnyElement {
        let colors = Self::colors(cx);
        let Some(service) = AgentService::global(cx) else {
            return div().into_any_element();
        };
        let service = service.read(cx);
        let state = State::get::<AgentState>(cx);
        let cluster = self.draft_cluster(cx);
        let mut body: Vec<AnyElement> = Vec::new();
        if !state.note_dismissed {
            body.push(first_run_note(&colors));
        }
        body.push(section_title("Cluster", &colors));
        body.push(match &cluster {
            Some(cluster) => {
                let name = ConnectionManager::try_global(cx)
                    .map(|m| m.read(cx).display_name(cluster).to_string())
                    .unwrap_or_else(|| cluster.to_string());
                h_flex()
                    .px(u(14.0))
                    .py(u(4.0))
                    .gap(u(6.0))
                    .child(Chip::new(name).dot(cluster_color(cluster, cx)))
                    .child(small(
                        "The agent reads it through Kubyl's tools, with your access.",
                        colors.text_dim,
                    ))
                    .into_any_element()
            }
            None => div()
                .px(u(14.0))
                .py(u(4.0))
                .child(small(
                    "Select a cluster in the sidebar first.",
                    colors.text_dim,
                ))
                .into_any_element(),
        });
        body.push(section_title("Agent", &colors));
        let picked = self.draft_agent.clone().or_else(|| {
            service
                .default_agent(state.last_agent.as_deref())
                .map(|a| a.spec.id.clone())
        });
        for (i, agent) in service.agents().iter().enumerate() {
            body.push(agent_row(i, agent, picked.as_deref(), true, weak, cx));
        }
        if let Some(draft) = self.draft.and_then(|id| service.thread(id)) {
            body.push(
                div()
                    .px(u(14.0))
                    .py(u(6.0))
                    .child(match &draft.status {
                        // The composer shows "Starting …".
                        ThreadStatus::Starting => div().into_any_element(),
                        ThreadStatus::Idle => small(
                            format!(
                                "{} is ready: pick a model and thinking level below, then ask.",
                                draft.agent_name
                            ),
                            colors.text_dim,
                        )
                        .into_any_element(),
                        ThreadStatus::NeedsSignIn { methods, hint } => {
                            sign_in(draft.id, &draft.agent_name, methods, hint.clone(), &colors)
                        }
                        ThreadStatus::Failed(message) | ThreadStatus::Ended(message) => {
                            failed(draft.id, &draft.agent, message, &colors)
                        }
                        _ => div().into_any_element(),
                    })
                    .into_any_element(),
            );
        }
        if service
            .agents()
            .iter()
            .all(|a| a.checked && a.program.is_none())
        {
            body.push(
                div()
                    .px(u(14.0))
                    .py(u(6.0))
                    .child(small(
                        "No agent is installed. Install one with the command next to it, then click Refresh.",
                        colors.text_dim,
                    ))
                    .into_any_element(),
            );
        }
        div()
            .id("agent-welcome")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py(u(4.0))
            .children(body)
            .into_any_element()
    }

    fn render_thread(&self, thread: &Thread, weak: &WeakEntity<Self>, cx: &App) -> AnyElement {
        let colors = Self::colors(cx);
        let mut items: Vec<AnyElement> = Vec::new();
        for note in &thread.notes {
            items.push(notice(note, false, &colors));
        }
        for (ix, entry) in thread.transcript.entries.iter().enumerate() {
            items.push(self.render_entry(thread, ix, entry, weak, cx));
        }
        if !thread.transcript.plan.is_empty() {
            items.push(plan(thread, &colors));
        }
        match &thread.status {
            ThreadStatus::Starting => items.push(working("Starting the agent…", &colors)),
            ThreadStatus::Running if thread.pending.is_empty() => {
                items.push(working("Working…", &colors))
            }
            ThreadStatus::NotInstalled { install } => {
                items.push(not_installed(&thread.agent_name, install.clone(), &colors))
            }
            ThreadStatus::NeedsSignIn { methods, hint } => items.push(sign_in(
                thread.id,
                &thread.agent_name,
                methods,
                hint.clone(),
                &colors,
            )),
            ThreadStatus::Failed(message) | ThreadStatus::Ended(message) => {
                items.push(failed(thread.id, &thread.agent, message, &colors))
            }
            _ => {}
        }
        for pending in &thread.pending {
            items.push(match &pending.kind {
                PendingKind::Form {
                    message,
                    title,
                    fields,
                } => form_card(
                    thread.id,
                    pending.id,
                    message,
                    title.as_deref(),
                    fields,
                    self.forms.get(&pending.id),
                    weak,
                    &colors,
                ),
                kind => pending_card(thread.id, pending.id, kind, &colors),
            });
        }
        div()
            .id(("agent-transcript", thread.id.0))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                v_flex()
                    .gap(u(10.0))
                    .px(u(14.0))
                    .py(u(12.0))
                    .children(items),
            )
            .into_any_element()
    }

    fn render_entry(
        &self,
        thread: &Thread,
        ix: usize,
        entry: &Entry,
        weak: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let colors = Self::colors(cx);
        let key = (thread.id.0, ix);
        match entry {
            Entry::User { text, chips } => v_flex()
                .gap(u(6.0))
                .px(u(10.0))
                .py(u(8.0))
                .rounded(u(6.0))
                .bg(colors.elevated)
                .border_1()
                .border_color(colors.border_variant)
                .when(!chips.is_empty(), |this| {
                    this.child(
                        h_flex().flex_wrap().gap(u(4.0)).children(
                            chips
                                .iter()
                                .map(|c| Chip::new(c.label.clone()).icon(IconName::Link)),
                        ),
                    )
                })
                .when(!text.trim().is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(u(13.0))
                            .whitespace_normal()
                            .child(text.clone()),
                    )
                })
                .into_any_element(),
            Entry::Agent { text, .. } => div()
                .text_size(u(13.0))
                .child(
                    TextView::markdown(
                        SharedString::from(format!("agent-md-{}-{ix}", thread.id.0)),
                        text.clone(),
                    )
                    .selectable(true),
                )
                .into_any_element(),
            Entry::Thought { text } => {
                let open = self.expanded.contains(&key);
                let weak = weak.clone();
                v_flex()
                    .gap(u(4.0))
                    .child(
                        h_flex()
                            .id(("agent-thought", ix))
                            .gap(u(4.0))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| toggle(&weak, key, cx))
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(11.0)
                                .color(colors.text_faint),
                            )
                            .child(small("Thinking", colors.text_dim)),
                    )
                    .when(open, |this| {
                        this.child(
                            div()
                                .pl(u(15.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .whitespace_normal()
                                .child(text.clone()),
                        )
                    })
                    .into_any_element()
            }
            Entry::Tool(tool) => self.tool_card(thread, ix, tool, weak, cx),
            Entry::Notice { text, error } => notice(text, *error, &colors),
        }
    }

    fn tool_card(
        &self,
        thread: &Thread,
        ix: usize,
        tool: &ToolEntry,
        weak: &WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
        let colors = Self::colors(cx);
        let key = (thread.id.0, ix);
        let open = self.expanded.contains(&key);
        let (icon, title) = match &tool.kubyl {
            Some(call) => {
                let summary = call.summary();
                let title = if summary.is_empty() {
                    format!("Kubyl · {}", call.tool.replace('_', " "))
                } else {
                    format!("Kubyl · {} {summary}", call.tool.replace('_', " "))
                };
                (IconName::Layers, title)
            }
            None => (
                tool_icon(tool),
                if tool.title.is_empty() {
                    "Tool".into()
                } else {
                    tool.title.clone()
                },
            ),
        };
        let (status_icon, status_color) = match tool.status {
            ToolStatus::Pending => (IconName::Clock, colors.text_faint),
            ToolStatus::Running => (IconName::Clock, colors.yellow),
            ToolStatus::Done => (IconName::Check, colors.green),
            ToolStatus::Failed => (IconName::CircleX, colors.red),
        };
        let weak = weak.clone();
        let mut body: Vec<AnyElement> = Vec::new();
        if open {
            for (ci, content) in tool.content.iter().enumerate() {
                body.push(match content {
                    ToolContent::Text(text) => tool_text(text, (thread.id.0, ix, ci), &colors),
                    ToolContent::Diff { path, old, new } => {
                        diff_block(path, old.as_deref(), new, &colors)
                    }
                    ToolContent::Terminal(id) => terminal_block(thread.id, id, (ix, ci), cx),
                });
            }
            if tool.content.is_empty()
                && let Some(input) = &tool.raw_input
            {
                body.push(
                    mono_block(
                        serde_json::to_string_pretty(input).unwrap_or_default(),
                        &colors,
                    )
                    .into_any_element(),
                );
            }
        } else if let Some(ToolContent::Terminal(id)) = tool
            .content
            .iter()
            .find(|c| matches!(c, ToolContent::Terminal(_)))
        {
            // Commands show their output even folded: the user should see what ran.
            body.push(terminal_block(thread.id, id, (ix, 0), cx));
        } else if let Some(ToolContent::Text(text)) = tool.content.first() {
            let lines: Vec<&str> = unfence(text).lines().take(3).collect();
            if !lines.is_empty() && tool.kubyl.is_none() {
                body.push(mono_block(lines.join("\n"), &colors).into_any_element());
            }
        }
        let long = tool.content.iter().any(|c| match c {
            ToolContent::Text(t) => t.lines().count() > TOOL_LINES,
            _ => false,
        });
        v_flex()
            .gap(u(6.0))
            .px(u(10.0))
            .py(u(6.0))
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .id(("agent-tool", ix))
                    .gap(u(6.0))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| toggle(&weak, key, cx))
                    .child(Icon::new(icon).size(13.0).color(colors.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(u(12.5))
                            .child(title),
                    )
                    .when(long && !open, |this| {
                        this.child(small("details", colors.text_faint))
                    })
                    .child(Icon::new(status_icon).size(12.0).color(status_color)),
            )
            .children(body)
            .into_any_element()
    }
}

fn toggle(weak: &WeakEntity<AgentPanel>, key: (u64, usize), cx: &mut App) {
    weak.update(cx, |this, cx| {
        if !this.expanded.remove(&key) {
            this.expanded.insert(key);
        }
        cx.notify();
    })
    .ok();
}

fn tool_icon(tool: &ToolEntry) -> IconName {
    use agent_client_protocol_schema::v1::ToolKind;
    match tool.kind {
        Some(ToolKind::Execute) => IconName::Terminal,
        Some(ToolKind::Read) => IconName::File,
        Some(ToolKind::Edit) => IconName::Diff,
        Some(ToolKind::Delete) => IconName::Trash,
        Some(ToolKind::Search) => IconName::Search,
        Some(ToolKind::Fetch) => IconName::Globe,
        Some(ToolKind::Think) => IconName::Info,
        _ => IconName::Zap,
    }
}

fn status_label(status: &ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Starting => "starting",
        ThreadStatus::Idle => "ready",
        ThreadStatus::Running => "working",
        ThreadStatus::NeedsSignIn { .. } => "needs sign-in",
        ThreadStatus::NotInstalled { .. } => "not installed",
        ThreadStatus::Failed(_) => "failed",
        ThreadStatus::Ended(_) => "stopped",
    }
}

fn section_title(title: &str, colors: &Colors) -> AnyElement {
    div()
        .px(u(14.0))
        .pt(u(10.0))
        .pb(u(4.0))
        .text_size(u(11.0))
        .text_color(colors.text_faint)
        .child(title.to_uppercase())
        .into_any_element()
}

fn notice(text: &str, error: bool, colors: &Colors) -> AnyElement {
    h_flex()
        .gap(u(6.0))
        .items_start()
        .child(
            Icon::new(if error {
                IconName::TriangleAlert
            } else {
                IconName::Info
            })
            .size(12.0)
            .color(if error { colors.red } else { colors.text_faint }),
        )
        .child(
            div()
                .flex_1()
                .text_size(u(12.0))
                .text_color(if error { colors.red } else { colors.text_dim })
                .whitespace_normal()
                .child(text.to_string()),
        )
        .into_any_element()
}

fn working(text: &str, colors: &Colors) -> AnyElement {
    h_flex()
        .gap(u(6.0))
        .child(Icon::new(IconName::Clock).size(12.0).color(colors.yellow))
        .child(small(text.to_string(), colors.text_dim))
        .into_any_element()
}

fn first_run_note(colors: &Colors) -> AnyElement {
    v_flex()
        .mx(u(14.0))
        .mt(u(8.0))
        .p(u(10.0))
        .gap(u(6.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.elevated)
        .child(
            h_flex()
                .gap(u(6.0))
                .child(Icon::new(IconName::Shield).size(13.0).color(colors.accent))
                .child(div().text_size(u(12.5)).child("What the agent sees")),
        )
        .child(small(
            "Your own agent (Claude, Codex, Gemini…) runs on this machine with its own sign-in. It reads one cluster through Kubyl's read-only tools, with your access. Kubyl masks Secret values, private keys and token-like text, and asks before every command and file change.",
            colors.text_muted,
        ))
        .child(small(
            "What it reads goes to the agent's provider, logs included. The agent isn't sandboxed: its own tools can still read files on this machine, such as ~/.kube/config.",
            colors.text_muted,
        ))
        .child(
            h_flex().child(Button::new("agent-note-ok").label("Got it").on_click(|_, _, cx| {
                State::update::<AgentState>(cx, |state| state.note_dismissed = true);
                if let Some(service) = AgentService::global(cx) {
                    service.update(cx, |_, cx| cx.notify());
                }
            })),
        )
        .into_any_element()
}

fn agent_row(
    i: usize,
    agent: &AgentInfo,
    picked: Option<&str>,
    pickable: bool,
    weak: &WeakEntity<AgentPanel>,
    cx: &App,
) -> AnyElement {
    let colors = AgentPanel::colors(cx);
    let installed = agent.program.is_some();
    let selected = pickable && picked == Some(agent.spec.id.as_str());
    let id = agent.spec.id.clone();
    let weak = weak.clone();
    let status = if !agent.checked {
        "looking…".to_string()
    } else if agent.running {
        "running".to_string()
    } else if installed {
        "installed".to_string()
    } else {
        "not installed".to_string()
    };
    h_flex()
        .id(("agent-row", i))
        .gap(u(8.0))
        .px(u(14.0))
        .py(u(5.0))
        .when(selected, |this| this.bg(colors.selection))
        .when(pickable && installed, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(colors.hover))
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    weak.update(cx, |this, cx| {
                        this.draft_agent = Some(id);
                        this.drafting = true;
                        this.ensure_draft(cx);
                        cx.notify();
                    })
                    .ok();
                })
        })
        .child(
            div()
                .size(u(7.0))
                .rounded_full()
                .flex_none()
                .bg(if installed {
                    colors.green
                } else {
                    colors.text_faint
                }),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_size(u(12.5)).child(agent.spec.name.clone()))
                .child(small(
                    match (&agent.program, &agent.spec.install) {
                        (Some(path), _) => format!("{status} · {}", path.display()),
                        (None, Some(install)) if agent.checked => install.clone(),
                        _ => status.clone(),
                    },
                    colors.text_dim,
                )),
        )
        .when(!installed && agent.checked, |this| {
            match agent.spec.install.clone() {
                Some(install) if install.starts_with("npm ") => {
                    this.child(copy_button(("agent-install", i), install))
                }
                _ => this,
            }
        })
        .into_any_element()
}

fn not_installed(name: &str, install: Option<String>, colors: &Colors) -> AnyElement {
    v_flex()
        .gap(u(6.0))
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border)
        .child(
            div()
                .text_size(u(12.5))
                .child(format!("{name} isn't installed")),
        )
        .child(small(
            "Kubyl doesn't install agents. Install it in a terminal, then retry:",
            colors.text_dim,
        ))
        .when_some(install, |this, install| {
            this.child(
                h_flex()
                    .gap(u(6.0))
                    .child(mono_block(install.clone(), colors))
                    .child(copy_button("agent-install-cmd", install)),
            )
        })
        .child(
            h_flex().child(
                Button::new("agent-refresh")
                    .label("Refresh")
                    .on_click(|_, _, cx| {
                        if let Some(service) = AgentService::global(cx) {
                            service.update(cx, |s, cx| s.refresh_agents(cx));
                        }
                    }),
            ),
        )
        .into_any_element()
}

fn sign_in(
    thread: ThreadId,
    name: &str,
    methods: &[(String, String, Option<String>)],
    hint: Option<String>,
    colors: &Colors,
) -> AnyElement {
    v_flex()
        .gap(u(6.0))
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.yellow)
        .child(
            h_flex()
                .gap(u(6.0))
                .child(Icon::new(IconName::Key).size(13.0).color(colors.yellow))
                .child(
                    div()
                        .text_size(u(12.5))
                        .child(format!("{name} needs a sign-in")),
                ),
        )
        .when_some(hint, |this, hint| {
            this.child(small(hint, colors.text_muted))
        })
        .child(
            h_flex()
                .flex_wrap()
                .gap(u(6.0))
                .children(methods.iter().enumerate().map(|(i, (id, label, _))| {
                    let id = id.clone();
                    Button::new(("agent-sign-in", i))
                        .label(label.clone())
                        .on_click(move |_, _, cx| {
                            if let Some(service) = AgentService::global(cx) {
                                let id = id.clone();
                                service.update(cx, |s, cx| s.sign_in(thread, id, cx));
                            }
                        })
                }))
                .child(
                    Button::new("agent-sign-in-retry")
                        .label("Retry")
                        .ghost()
                        .on_click(move |_, _, cx| {
                            if let Some(service) = AgentService::global(cx) {
                                service.update(cx, |s, cx| s.retry(thread, cx));
                            }
                        }),
                ),
        )
        .into_any_element()
}

fn failed(thread: ThreadId, agent: &str, message: &str, colors: &Colors) -> AnyElement {
    let agent = agent.to_string();
    v_flex()
        .gap(u(6.0))
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.red)
        .child(notice(message, true, colors))
        .child(
            h_flex()
                .gap(u(6.0))
                .child(
                    Button::new("agent-retry")
                        .label("Retry")
                        .icon(IconName::RotateCcw)
                        .on_click(move |_, _, cx| {
                            if let Some(service) = AgentService::global(cx) {
                                service.update(cx, |s, cx| s.retry(thread, cx));
                            }
                        }),
                )
                .child(
                    Button::new("agent-output")
                        .label("Agent output")
                        .ghost()
                        .on_click(move |_, window, cx| {
                            crate::show_output(&agent, window, cx);
                        }),
                ),
        )
        .into_any_element()
}

fn plan(thread: &Thread, colors: &Colors) -> AnyElement {
    v_flex()
        .gap(u(3.0))
        .p(u(8.0))
        .rounded(u(6.0))
        .bg(colors.elevated)
        .child(
            h_flex()
                .gap(u(6.0))
                .child(
                    Icon::new(IconName::ListChecks)
                        .size(12.0)
                        .color(colors.text_muted),
                )
                .child(small("Plan", colors.text_muted)),
        )
        .children(thread.transcript.plan.iter().map(|item| {
            h_flex()
                .gap(u(6.0))
                .child(
                    Icon::new(if item.done {
                        IconName::CircleCheck
                    } else if item.active {
                        IconName::ChevronRight
                    } else {
                        IconName::Square
                    })
                    .size(11.0)
                    .color(if item.done {
                        colors.green
                    } else if item.active {
                        colors.accent
                    } else {
                        colors.text_faint
                    }),
                )
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(if item.done {
                            colors.text_dim
                        } else {
                            colors.text
                        })
                        .child(item.text.clone()),
                )
        }))
        .into_any_element()
}

fn diff_block(path: &str, old: Option<&str>, new: &str, colors: &Colors) -> AnyElement {
    let diff = similar::TextDiff::from_lines(old.unwrap_or(""), new);
    let mut lines: Vec<AnyElement> = Vec::new();
    for change in diff.iter_all_changes().take(400) {
        let (sign, color, bg) = match change.tag() {
            similar::ChangeTag::Delete => ("-", colors.red, Some(colors.red.opacity(0.08))),
            similar::ChangeTag::Insert => ("+", colors.green, Some(colors.green.opacity(0.08))),
            similar::ChangeTag::Equal => (" ", colors.text_dim, None),
        };
        let text = format!("{sign} {}", change.value().trim_end_matches('\n'));
        lines.push(
            div()
                .px(u(6.0))
                .when_some(bg, |this, bg| this.bg(bg))
                .text_color(color)
                .child(text)
                .into_any_element(),
        );
    }
    v_flex()
        .gap(u(2.0))
        .child(small(path.to_string(), colors.text_dim))
        .child(
            v_flex()
                .rounded(u(4.0))
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.background)
                .py(u(4.0))
                .font_family(kubyl_ui::fonts::MONO)
                .text_size(u(11.5))
                .children(lines),
        )
        .into_any_element()
}

fn terminal_block(thread: ThreadId, terminal: &str, key: (usize, usize), cx: &App) -> AnyElement {
    let colors = cx.colors().clone();
    let Some(service) = AgentService::global(cx) else {
        return div().into_any_element();
    };
    let Some((line, output, truncated, exit)) = service.read(cx).terminal(thread, terminal) else {
        return small("The command's output is gone.", colors.text_faint).into_any_element();
    };
    let status = match &exit {
        None => ("running".to_string(), colors.yellow),
        Some(exit) => match (exit.code, &exit.signal) {
            (Some(0), _) => ("exit 0".to_string(), colors.green),
            (Some(code), _) => (format!("exit {code}"), colors.red),
            (None, Some(signal)) => (signal.clone(), colors.red),
            _ => ("ended".to_string(), colors.text_dim),
        },
    };
    let tail: Vec<&str> = output.lines().collect();
    let shown = tail[tail.len().saturating_sub(40)..].join("\n");
    v_flex()
        .id(("agent-term", key.0 * 100 + key.1))
        .gap(u(4.0))
        .child(
            h_flex()
                .gap(u(6.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(kubyl_ui::fonts::MONO)
                        .text_size(u(11.5))
                        .truncate()
                        .child(format!("$ {line}")),
                )
                .child(small(status.0, status.1)),
        )
        .when(!shown.is_empty(), |this| {
            this.child(mono_block(
                if truncated || tail.len() > 40 {
                    format!("…\n{shown}")
                } else {
                    shown
                },
                &colors,
            ))
        })
        .into_any_element()
}

fn pending_card(thread: ThreadId, pending: u64, kind: &PendingKind, colors: &Colors) -> AnyElement {
    let answer = move |answer: Answer| {
        move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
            if let Some(service) = AgentService::global(cx) {
                let answer = answer.clone();
                service.update(cx, |s, cx| s.answer(thread, pending, answer, cx));
            }
        }
    };
    let card = v_flex()
        .gap(u(8.0))
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.yellow)
        .bg(colors.elevated);
    let header = |icon: IconName, title: String| {
        h_flex()
            .gap(u(6.0))
            .child(Icon::new(icon).size(13.0).color(colors.yellow))
            // One line: long titles (paths, commands) can't wrap; the details follow below.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(u(12.5))
                    .child(title),
            )
    };
    match kind {
        PendingKind::Permission {
            title,
            command,
            choices,
            ..
        } => card
            .child(header(
                IconName::Lock,
                permission_title(title, command.as_deref()),
            ))
            .when_some(command.clone(), |this, command| {
                let warnings = kubyl_agent_core::policy::command_warnings(&command);
                this.child(mono_block(format!("$ {command}"), colors))
                    .children(warnings.into_iter().map(|w| notice(w, true, colors)))
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(u(6.0))
                    .children(choices.iter().enumerate().map(|(i, choice)| {
                        let button = Button::new(("agent-perm", pending as usize * 16 + i))
                            .label(choice.name.clone())
                            .on_click(answer(Answer::Choose(choice.id.clone())));
                        if choice.allow {
                            button.primary()
                        } else {
                            button.ghost()
                        }
                    })),
            )
            .into_any_element(),
        PendingKind::Command {
            line,
            cwd,
            env,
            warnings,
        } => card
            .child(header(IconName::Terminal, "Run this command?".into()))
            .child(mono_block(format!("$ {line}"), colors))
            .child(small(format!("in {}", cwd.display()), colors.text_dim))
            .when(!env.is_empty(), |this| {
                let vars: Vec<String> = env.iter().map(|(n, v)| format!("{n}={v}")).collect();
                this.child(small("with this environment:", colors.text_dim))
                    .child(mono_block(vars.join("\n"), colors))
            })
            .children(warnings.iter().map(|w| notice(w, true, colors)))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        Button::new(("agent-run", pending as usize))
                            .label("Run")
                            .primary()
                            .on_click(answer(Answer::Allow)),
                    )
                    .child(
                        Button::new(("agent-skip", pending as usize))
                            .label("Don't run")
                            .ghost()
                            .on_click(answer(Answer::Deny)),
                    ),
            )
            .into_any_element(),
        PendingKind::Read { path } => card
            .child(header(
                IconName::File,
                "Read a file outside the thread's folder?".into(),
            ))
            .child(mono_block(path.display().to_string(), colors))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        Button::new(("agent-read", pending as usize))
                            .label("Allow")
                            .primary()
                            .on_click(answer(Answer::Allow)),
                    )
                    .child(
                        Button::new(("agent-noread", pending as usize))
                            .label("Deny")
                            .ghost()
                            .on_click(answer(Answer::Deny)),
                    ),
            )
            .into_any_element(),
        PendingKind::OpenUrl { message, url, .. } => card
            .child(header(
                IconName::Globe,
                "The agent asks you to open a page".into(),
            ))
            .when(!message.is_empty(), |this| {
                this.child(
                    div()
                        .text_size(u(12.0))
                        .whitespace_normal()
                        .child(message.clone()),
                )
            })
            .child(mono_block(url.clone(), colors))
            .child(small(
                "It opens in your browser. Check the address before you sign in anywhere.",
                colors.text_dim,
            ))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        Button::new(("agent-url", pending as usize))
                            .label("Open in browser")
                            .icon(IconName::ExternalLink)
                            .primary()
                            .on_click(answer(Answer::Allow)),
                    )
                    .child(
                        Button::new(("agent-nourl", pending as usize))
                            .label("Decline")
                            .ghost()
                            .on_click(answer(Answer::Deny)),
                    ),
            )
            .into_any_element(),
        // Forms render through `form_card`, which keeps their inputs.
        PendingKind::Form { .. } => card.into_any_element(),
        PendingKind::Write { path, old, new } => card
            .child(header(
                IconName::Diff,
                if old.is_some() {
                    "Change this file?".into()
                } else {
                    "Create this file?".into()
                },
            ))
            .child(diff_block(
                &path.display().to_string(),
                old.as_deref(),
                new,
                colors,
            ))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        Button::new(("agent-write", pending as usize))
                            .label("Write")
                            .primary()
                            .on_click(answer(Answer::Allow)),
                    )
                    .child(
                        Button::new(("agent-nowrite", pending as usize))
                            .label("Don't write")
                            .ghost()
                            .on_click(answer(Answer::Deny)),
                    ),
            )
            .into_any_element(),
    }
}

impl AgentPanel {
    fn render_composer(
        &self,
        thread: Option<&Thread>,
        weak: &WeakEntity<Self>,
        cx: &App,
    ) -> impl IntoElement {
        let colors = Self::colors(cx);
        let running = thread.is_some_and(|t| t.is_running());
        let weak = weak.clone();
        let chips = self.chips.iter().enumerate().map(|(i, chip)| {
            let weak = weak.clone();
            div()
                .id(("agent-chip", i))
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| {
                        if i < this.chips.len() {
                            this.chips.remove(i);
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .child(
                    Chip::new(chip.label.clone())
                        .icon(IconName::Link)
                        .removable(),
                )
        });
        let readiness = self.readiness(cx);
        let ready = readiness == Readiness::Ready;
        // A new thread shows the settings of the thread prepared for it.
        let settings = match thread {
            Some(t) => Some(session_settings(t, &colors)),
            None => AgentService::global(cx)
                .zip(self.draft)
                .and_then(|(s, id)| s.read(cx).thread(id).map(|t| session_settings(t, &colors))),
        };
        let weak2 = weak.clone();
        let thread_id = thread.map(|t| t.id);
        let label = match thread {
            Some(thread) => format!("{} · {}", thread.agent_name, thread.cluster_name),
            None => String::new(),
        };
        v_flex()
            .flex_none()
            .gap(u(6.0))
            .px(u(12.0))
            .py(u(10.0))
            .border_t_1()
            .border_color(colors.border_variant)
            .when(!self.chips.is_empty(), |this| {
                this.child(h_flex().flex_wrap().gap(u(4.0)).children(chips))
            })
            .when(!self.mentions.is_empty(), |this| {
                this.child(mention_list(&self.mentions, &weak, &colors))
            })
            .when_some(settings.flatten(), |this, settings| this.child(settings))
            .when_some(
                match &readiness {
                    Readiness::Starting(text) => Some(text.clone()),
                    _ => None,
                },
                |this, text| this.child(working(&text, &colors)),
            )
            .child(Textarea::new(&self.input).appearance(true).disabled(!ready))
            .child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(small(label, colors.text_faint)),
                    )
                    .child(if running {
                        Button::new("agent-stop")
                            .label("Stop")
                            .icon(IconName::Square)
                            .on_click(move |_, _, cx| {
                                if let (Some(id), Some(service)) =
                                    (thread_id, AgentService::global(cx))
                                {
                                    service.update(cx, |s, cx| s.cancel(id, cx));
                                }
                            })
                            .into_any_element()
                    } else {
                        Button::new("agent-send")
                            .label("Send")
                            .icon(IconName::ArrowUp)
                            .primary()
                            .disabled(!ready)
                            .on_click(move |_, window, cx| {
                                weak2.update(cx, |this, cx| this.submit(window, cx)).ok();
                            })
                            .into_any_element()
                    }),
            )
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_forms(window, cx);
        // Rendered means shown: prepare the new thread's agent right away.
        if !self.drafting {
            self.drafting = true;
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| this.ensure_draft(cx)).ok();
            });
        }
        let weak = cx.weak_entity();
        let cx: &App = cx;
        let colors = Self::colors(cx);
        let service = AgentService::global(cx);
        let service = service.as_ref().map(|s| s.read(cx));
        let thread = service
            .zip(self.thread)
            .and_then(|(s, id)| s.thread(id))
            .filter(|_| !self.show_threads);
        let (subheader, body, composer): (Option<AnyElement>, AnyElement, AnyElement) =
            match (service, thread) {
                (Some(_), Some(thread)) => (
                    Some(thread_header(thread, &colors, cx)),
                    self.render_thread(thread, &weak, cx),
                    self.render_composer(Some(thread), &weak, cx)
                        .into_any_element(),
                ),
                (Some(_), None) if self.show_threads => (
                    None,
                    self.render_threads(&weak, cx),
                    div().into_any_element(),
                ),
                (Some(_), None) => (
                    None,
                    self.render_welcome(&weak, cx),
                    self.render_composer(None, &weak, cx).into_any_element(),
                ),
                (None, _) => (
                    None,
                    div()
                        .p(u(14.0))
                        .child(small("Agents aren't available.", colors.text_dim))
                        .into_any_element(),
                    div().into_any_element(),
                ),
            };
        v_flex()
            .track_focus(&self.focus)
            .key_context("AgentPanel")
            .size_full()
            .text_color(colors.text)
            .child(self.render_header(&weak))
            .children(subheader)
            .child(body)
            .child(composer)
    }
}

fn thread_header(thread: &Thread, colors: &Colors, cx: &App) -> AnyElement {
    let production = ConnectionManager::try_global(cx)
        .is_some_and(|m| m.read(cx).caps(&thread.cluster).production);
    h_flex()
        .flex_none()
        .gap(u(6.0))
        .px(u(14.0))
        .py(u(6.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(
            div()
                .size(u(7.0))
                .rounded_full()
                .bg(cluster_color(&thread.cluster, cx))
                .flex_none(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(u(12.5))
                .child(thread.title()),
        )
        .when(production, |this| this.child(kubyl_ui::ProdBadge))
        .children(context_use(thread, colors))
        .child(small(status_label(&thread.status), colors.text_faint))
        .into_any_element()
}

impl AgentPanel {
    /// Creates inputs for new agent questions of the shown thread and drops finished ones.
    fn sync_forms(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(service) = AgentService::global(cx) else {
            return;
        };
        let forms: Vec<(u64, Vec<Field>)> = service
            .read(cx)
            .threads()
            .iter()
            .flat_map(|t| t.pending.iter())
            .filter_map(|p| match &p.kind {
                PendingKind::Form { fields, .. } => Some((p.id, fields.clone())),
                _ => None,
            })
            .collect();
        self.forms
            .retain(|id, _| forms.iter().any(|(pending, _)| pending == id));
        for (id, fields) in forms {
            if self.forms.contains_key(&id) {
                continue;
            }
            let mut state = FormState::default();
            for field in &fields {
                match &field.kind {
                    FieldKind::Text { default, .. } => {
                        let default = default.clone().unwrap_or_default();
                        state.inputs.insert(
                            field.key.clone(),
                            cx.new(|cx| InputState::new(window, cx).default_value(default)),
                        );
                    }
                    FieldKind::Number { default, .. } => {
                        let default = default.map(|d| d.to_string()).unwrap_or_default();
                        state.inputs.insert(
                            field.key.clone(),
                            cx.new(|cx| InputState::new(window, cx).default_value(default)),
                        );
                    }
                    FieldKind::Choice { default, .. } => {
                        state
                            .selected
                            .insert(field.key.clone(), default.iter().cloned().collect());
                    }
                    FieldKind::Multi { default, .. } => {
                        state.selected.insert(field.key.clone(), default.clone());
                    }
                    FieldKind::Bool { default } => {
                        state.bools.insert(field.key.clone(), *default);
                    }
                }
            }
            self.forms.insert(id, state);
        }
    }

    /// Checks a form's answers; sends them, or shows what's wrong.
    fn submit_form(
        &mut self,
        thread: ThreadId,
        pending: u64,
        fields: &[Field],
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.forms.get_mut(&pending) else {
            return;
        };
        let mut inputs = BTreeMap::new();
        for field in fields {
            let input = match &field.kind {
                FieldKind::Text { .. } | FieldKind::Number { .. } => state
                    .inputs
                    .get(&field.key)
                    .map(|i| elicitation::Input::Text(i.read(cx).value().to_string())),
                FieldKind::Choice { .. } | FieldKind::Multi { .. } => state
                    .selected
                    .get(&field.key)
                    .map(|s| elicitation::Input::Selected(s.clone())),
                FieldKind::Bool { .. } => state
                    .bools
                    .get(&field.key)
                    .map(|b| elicitation::Input::Bool(*b)),
            };
            if let Some(input) = input {
                inputs.insert(field.key.clone(), input);
            }
        }
        match elicitation::validate(fields, &inputs) {
            Ok(content) => {
                if let Some(service) = AgentService::global(cx) {
                    service.update(cx, |s, cx| {
                        s.answer(thread, pending, Answer::Submit(content), cx)
                    });
                }
            }
            Err(errors) => state.errors = errors,
        }
        cx.notify();
    }
}

#[allow(clippy::too_many_arguments)]
fn form_card(
    thread: ThreadId,
    pending: u64,
    message: &str,
    title: Option<&str>,
    fields: &[Field],
    state: Option<&FormState>,
    weak: &WeakEntity<AgentPanel>,
    colors: &Colors,
) -> AnyElement {
    let Some(state) = state else {
        return div().into_any_element();
    };
    let mut rows: Vec<AnyElement> = Vec::new();
    for (fi, field) in fields.iter().enumerate() {
        let label = if field.required {
            format!("{} *", field.label)
        } else {
            field.label.clone()
        };
        let control: AnyElement = match &field.kind {
            FieldKind::Text { .. } | FieldKind::Number { .. } => match state.inputs.get(&field.key)
            {
                Some(input) => Input::new(input).into_any_element(),
                None => div().into_any_element(),
            },
            FieldKind::Bool { .. } => {
                let on = state.bools.get(&field.key).copied().unwrap_or(false);
                let key = field.key.clone();
                let weak = weak.clone();
                div()
                    .id(("agent-form-bool", pending as usize * 64 + fi))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let key = key.clone();
                        weak.update(cx, |this, cx| {
                            if let Some(state) = this.forms.get_mut(&pending) {
                                let value = state.bools.entry(key).or_default();
                                *value = !*value;
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .child(Chip::new(if on { "Yes" } else { "No" }).selected(on))
                    .into_any_element()
            }
            FieldKind::Choice { options, .. } | FieldKind::Multi { options, .. } => {
                let multi = matches!(field.kind, FieldKind::Multi { .. });
                let selected = state.selected.get(&field.key).cloned().unwrap_or_default();
                h_flex()
                    .flex_wrap()
                    .gap(u(4.0))
                    .children(options.iter().enumerate().map(|(oi, (value, title))| {
                        let on = selected.contains(value);
                        let key = field.key.clone();
                        let value = value.clone();
                        let weak = weak.clone();
                        div()
                            .id(("agent-form-opt", (pending as usize * 64 + fi) * 64 + oi))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                let (key, value) = (key.clone(), value.clone());
                                weak.update(cx, |this, cx| {
                                    if let Some(state) = this.forms.get_mut(&pending) {
                                        let chosen = state.selected.entry(key).or_default();
                                        if multi {
                                            if let Some(i) = chosen.iter().position(|v| *v == value)
                                            {
                                                chosen.remove(i);
                                            } else {
                                                chosen.push(value);
                                            }
                                        } else {
                                            *chosen = vec![value];
                                        }
                                    }
                                    cx.notify();
                                })
                                .ok();
                            })
                            .child(Chip::new(title.clone()).selected(on))
                    }))
                    .into_any_element()
            }
        };
        rows.push(
            v_flex()
                .gap(u(3.0))
                .child(small(label, colors.text_muted))
                .when_some(field.description.clone(), |this, d| {
                    this.child(small(d, colors.text_faint))
                })
                .child(control)
                .when_some(state.errors.get(&field.key).cloned(), |this, e| {
                    this.child(small(e, colors.red))
                })
                .into_any_element(),
        );
    }
    let submit_fields = fields.to_vec();
    let weak_submit = weak.clone();
    let decline = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
        if let Some(service) = AgentService::global(cx) {
            service.update(cx, |s, cx| s.answer(thread, pending, Answer::Deny, cx));
        }
    };
    v_flex()
        .gap(u(8.0))
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.yellow)
        .bg(colors.elevated)
        .child(
            h_flex()
                .gap(u(6.0))
                .child(Icon::new(IconName::Info).size(13.0).color(colors.yellow))
                .child(
                    div().flex_1().text_size(u(12.5)).whitespace_normal().child(
                        title
                            .map(str::to_string)
                            .unwrap_or_else(|| "The agent asks".into()),
                    ),
                ),
        )
        .when(!message.is_empty(), |this| {
            this.child(
                div()
                    .text_size(u(12.0))
                    .whitespace_normal()
                    .child(message.to_string()),
            )
        })
        .children(rows)
        .child(small(
            "Your answer goes to the agent. Don't enter passwords or tokens here.",
            colors.text_dim,
        ))
        .child(
            h_flex()
                .gap(u(6.0))
                .child(
                    Button::new(("agent-form-send", pending as usize))
                        .label("Answer")
                        .primary()
                        .on_click(move |_, _, cx| {
                            let fields = submit_fields.clone();
                            weak_submit
                                .update(cx, |this, cx| {
                                    this.submit_form(thread, pending, &fields, cx)
                                })
                                .ok();
                        }),
                )
                .child(
                    Button::new(("agent-form-decline", pending as usize))
                        .label("Decline")
                        .ghost()
                        .on_click(decline),
                ),
        )
        .into_any_element()
}

/// Tool output as agents send it: a lone fenced block (```` ```console ````) shows as plain
/// output without the fence; text with fences among prose renders as markdown; anything else
/// stays monospace, as it came.
fn tool_text(text: &str, key: (u64, usize, usize), colors: &Colors) -> AnyElement {
    let inner = unfence(text);
    if inner.len() != text.len() || !text.contains("```") {
        return mono_block(inner.to_string(), colors).into_any_element();
    }
    div()
        .text_size(u(12.5))
        .child(
            TextView::markdown(
                SharedString::from(format!("agent-tool-md-{}-{}-{}", key.0, key.1, key.2)),
                text.to_string(),
            )
            .selectable(true),
        )
        .into_any_element()
}

/// The content of `text` when it's one fenced code block, else `text`.
fn unfence(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return text;
    };
    let Some(body) = rest.strip_suffix("```") else {
        return text;
    };
    // Drop the info string (`console`, `yaml`) on the opening line.
    let body = match body.split_once('\n') {
        Some((_, body)) => body,
        None => return text,
    };
    if body.contains("\n```") {
        // More than one block: not a lone fence.
        return text;
    }
    body.trim_end_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::unfence;

    #[test]
    fn permission_titles_dont_repeat_the_command() {
        use super::permission_title;
        assert_eq!(
            permission_title(
                "f=/tmp/x; head -c 3000 $f",
                Some("f=/tmp/x; head -c 3000 $f; wc -l $f")
            ),
            "The agent wants to run a command"
        );
        assert_eq!(
            permission_title("Edit gateway.yaml", None),
            "The agent asks: Edit gateway.yaml"
        );
    }

    #[test]
    fn lone_fences_are_removed() {
        assert_eq!(unfence("```console\n$ ls\nfile\n```"), "$ ls\nfile");
        assert_eq!(unfence("plain output"), "plain output");
        let two = "```a\nx\n```\ntext\n```b\ny\n```";
        assert_eq!(unfence(two), two);
    }
}

impl AgentPanel {
    /// The cluster mentions come from: the shown thread's, else the new thread's.
    fn composer_cluster(&self, cx: &App) -> Option<ClusterId> {
        match self.thread {
            Some(id) => AgentService::global(cx)?
                .read(cx)
                .thread(id)
                .map(|t| t.cluster.clone()),
            None => self.draft_cluster(cx),
        }
    }

    /// Offers objects of the composer's cluster for the `@` mention being typed: what Kubyl's
    /// watch caches hold, like the palette's object search.
    fn update_mentions(&mut self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let (Some(query), Some(cluster)) = (mentions::query(&text), self.composer_cluster(cx))
        else {
            if !self.mentions.is_empty() {
                self.mentions.clear();
                cx.notify();
            }
            return;
        };
        let all = mention_candidates(&cluster, cx);
        let candidates: Vec<mentions::Candidate> =
            all.iter().map(|m| m.candidate.clone()).collect();
        self.mentions = mentions::rank(&candidates, query, MENTIONS)
            .into_iter()
            .map(|i| all[i].clone())
            .collect();
        cx.notify();
    }

    fn pick_mention(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mention) = self.mentions.get(index).cloned() else {
            return;
        };
        self.mentions.clear();
        let text = mentions::complete(&self.input.read(cx).value(), &mention.candidate.name);
        self.input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        if let Some(chip) = crate::object_chip(
            &mention.candidate.kind,
            &mention.target,
            mention.object.as_deref(),
            "mentioned",
        ) && !self.chips.iter().any(|c| c.uri == chip.uri)
        {
            self.chips.push(chip);
        }
        cx.notify();
    }
}

/// The objects in Kubyl's watch caches of `cluster`, one per object.
fn mention_candidates(cluster: &ClusterId, cx: &App) -> Vec<Mention> {
    let discovery = ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(cluster));
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for store in kubyl_resources::ResourceStores::all(cx) {
        let store = store.read(cx);
        let key = store.key();
        if &key.cluster != cluster {
            continue;
        }
        let kind = discovery
            .as_ref()
            .and_then(|d| d.by_gvr(&key.gvr).map(|r| r.gvk.kind.clone()))
            .unwrap_or_else(|| key.gvr.resource.clone());
        for object in store.objects().values() {
            let Some(name) = object.pointer("/metadata/name").and_then(|n| n.as_str()) else {
                continue;
            };
            let namespace = object
                .pointer("/metadata/namespace")
                .and_then(|n| n.as_str())
                .map(str::to_string);
            if !seen.insert((key.gvr.clone(), namespace.clone(), name.to_string())) {
                continue;
            }
            out.push(Mention {
                candidate: mentions::Candidate {
                    kind: kind.clone(),
                    namespace: namespace.clone(),
                    name: name.to_string(),
                },
                target: kubyl_core::ResourceRef::object(
                    cluster.clone(),
                    key.gvr.clone(),
                    namespace,
                    name.to_string(),
                ),
                object: Some(object.clone()),
            });
        }
    }
    out
}

fn mention_list(items: &[Mention], weak: &WeakEntity<AgentPanel>, colors: &Colors) -> AnyElement {
    v_flex()
        .py(u(4.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.elevated)
        .children(items.iter().enumerate().map(|(i, item)| {
            let weak = weak.clone();
            h_flex()
                .id(("agent-mention", i))
                .gap(u(6.0))
                .px(u(8.0))
                .py(u(3.0))
                .cursor_pointer()
                .when(i == 0, |this| this.bg(colors.selection))
                .hover(|this| this.bg(colors.hover))
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| this.pick_mention(i, window, cx))
                        .ok();
                })
                .child(Icon::new(IconName::Box).size(12.0).color(colors.text_muted))
                .child(div().text_size(u(12.5)).child(item.candidate.name.clone()))
                .child(div().flex_1())
                .child(small(
                    match &item.candidate.namespace {
                        Some(ns) => format!("{} · {ns}", item.candidate.kind),
                        None => item.candidate.kind.clone(),
                    },
                    colors.text_dim,
                ))
        }))
        .into_any_element()
}

#[cfg(test)]
mod gpui_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};
    use kubyl_agent_core::elicitation::{Field, FieldKind};
    use kubyl_agent_core::thread::Entry;
    use kubyl_core::actions::AskAgent;
    use serde_json::json;

    use super::*;

    /// A window with the Agent panel (registered like the dock builds it), agents marked as
    /// not installed, and no clusters.
    fn open(
        cx: &mut TestAppContext,
    ) -> (
        tempfile::TempDir,
        Entity<AgentPanel>,
        &mut VisualTestContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, &path);
            kubyl_ui::init(cx);
            cx.set_global(kubyl_resources::ResourceSelection::default());
            crate::init_with(false, cx);
            let service = AgentService::global(cx).unwrap();
            service.update(cx, |service, _| {
                let ids: Vec<String> = service.agents().iter().map(|a| a.spec.id.clone()).collect();
                for id in ids {
                    service.core_mut().set_program_for_tests(&id, None);
                }
            });
        });
        let slot: Rc<RefCell<Option<Entity<AgentPanel>>>> = Rc::default();
        let (_root, vcx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let _handle = AgentDockPanel.build(window, cx);
                let panel = panel_for(window.window_handle(), cx).expect("the dock registered it");
                *slot.borrow_mut() = Some(panel.clone());
                gpui_component::Root::new(panel, window, cx)
            }
        });
        let panel = slot.borrow().clone().unwrap();
        vcx.run_until_parked();
        (dir, panel, vcx)
    }

    fn service(cx: &mut VisualTestContext) -> Entity<AgentService> {
        cx.update(|_, cx| AgentService::global(cx).unwrap())
    }

    /// Attaches a chip for `cluster` through `kubyl::AskAgent`, types `text` and sends.
    /// A ready thread prepared for the new-thread composer, as if Claude had started.
    fn prepare(panel: &Entity<AgentPanel>, cx: &mut VisualTestContext) -> ThreadId {
        let service = service(cx);
        panel.update(cx, |panel, cx| {
            let id = service.update(cx, |service, _| {
                let core = service.core_mut();
                core.set_program_for_tests("claude", Some("/nonexistent/claude-agent-acp".into()));
                core.insert_ready_thread_for_tests("claude", ClusterId::new("kind-dev"))
            });
            panel.thread = None;
            panel.draft_cluster = Some(ClusterId::new("kind-dev"));
            panel.draft_agent = Some("claude".into());
            panel.draft = Some(id);
            panel.drafting = true;
            cx.notify();
            id
        })
    }

    fn start_thread(
        panel: &Entity<AgentPanel>,
        cx: &mut VisualTestContext,
        text: &str,
    ) -> ThreadId {
        prepare(panel, cx);
        cx.update(|window, cx| {
            window.dispatch_action(
                Box::new(AskAgent {
                    cluster: ClusterId::new("kind-dev"),
                    label: "2 log lines of shop/web-0".into(),
                    uri: "kubyl://kind-dev/logs/pods/shop/web-0".into(),
                    text: "GET /api Authorization: Bearer abcdefghijklmnop\nOOMKilled".into(),
                }),
                cx,
            )
        });
        cx.run_until_parked();
        let text = text.to_string();
        panel.update_in(cx, |panel, window, cx| {
            panel
                .input
                .update(cx, |input, cx| input.set_value(text, window, cx));
            panel.submit(window, cx);
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| panel.thread.expect("a thread started"))
    }

    #[gpui::test]
    fn the_welcome_lists_agents_without_a_thread(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        panel.read_with(cx, |panel, _| {
            assert!(panel.thread.is_none());
            assert!(panel.chips.is_empty());
        });
        let service = service(cx);
        service.read_with(cx, |service, _| {
            assert_eq!(
                service.agents().len(),
                kubyl_agent_core::agents::builtins().len()
            );
            assert!(
                service
                    .agents()
                    .iter()
                    .all(|a| a.checked && a.program.is_none())
            );
        });
    }

    #[gpui::test]
    fn the_composer_waits_for_the_agent(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        // No agent installed: nothing can be sent.
        panel.update_in(cx, |panel, window, cx| {
            panel.draft_cluster = Some(ClusterId::new("kind-dev"));
            panel
                .input
                .update(cx, |input, cx| input.set_value("hello", window, cx));
            assert_eq!(panel.readiness(cx), Readiness::Blocked);
            panel.submit(window, cx);
            assert!(panel.thread.is_none());
        });
        // A prepared thread that's still starting: "Starting claude…", nothing is sent.
        let draft = prepare(&panel, cx);
        let service = service(cx);
        service.update(cx, |s, _| {
            s.core_mut()
                .set_status_for_tests(draft, ThreadStatus::Starting)
        });
        panel.update_in(cx, |panel, window, cx| {
            assert_eq!(
                panel.readiness(cx),
                Readiness::Starting("Starting claude…".into())
            );
            panel.submit(window, cx);
            assert!(panel.thread.is_none());
        });
        service.update(cx, |s, _| {
            s.core_mut().set_status_for_tests(draft, ThreadStatus::Idle)
        });
        panel.update_in(cx, |panel, window, cx| {
            assert_eq!(panel.readiness(cx), Readiness::Ready);
            panel.submit(window, cx);
            assert_eq!(panel.thread, Some(draft));
        });
    }

    #[gpui::test]
    fn ask_agent_attaches_a_masked_chip_and_starts_a_thread(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        let thread = start_thread(&panel, cx, "Why does it crash?");
        panel.read_with(cx, |panel, cx| {
            assert!(panel.chips.is_empty(), "sent with the prompt");
            assert!(panel.input.read(cx).value().is_empty());
        });
        let service = service(cx);
        service.read_with(cx, |service, _| {
            let thread = service.thread(thread).unwrap();
            assert_eq!(thread.cluster, ClusterId::new("kind-dev"));
            // The prepared thread took the message.
            assert!(!thread.is_unused());
            let Entry::User { text, chips } = &thread.transcript.entries[0] else {
                panic!("{:?}", thread.transcript.entries);
            };
            assert_eq!(text, "Why does it crash?");
            assert_eq!(chips[0].label, "2 log lines of shop/web-0");
            assert!(
                !chips[0].text.contains("abcdefghijklmnop"),
                "{}",
                chips[0].text
            );
            assert!(chips[0].text.contains("OOMKilled"));
        });
    }

    #[gpui::test]
    fn commands_and_questions_wait_for_answers(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        let thread = start_thread(&panel, cx, "go");
        let service = service(cx);
        let (command, form) = service.update(cx, |service, cx| {
            let command = service.core_mut().push_pending_for_tests(
                thread,
                PendingKind::Command {
                    line: "kubectl get secret db -o yaml".into(),
                    cwd: std::env::temp_dir(),
                    env: vec![("LANG".into(), "C".into())],
                    warnings: kubyl_agent_core::policy::command_warnings(
                        "kubectl get secret db -o yaml",
                    ),
                },
            );
            let form = service.core_mut().push_pending_for_tests(
                thread,
                PendingKind::Form {
                    message: "Which namespace?".into(),
                    title: None,
                    fields: vec![Field {
                        key: "namespace".into(),
                        label: "Namespace".into(),
                        description: None,
                        required: true,
                        kind: FieldKind::Text {
                            default: None,
                            min_length: None,
                            max_length: None,
                            pattern: None,
                            format: None,
                        },
                    }],
                },
            );
            cx.notify();
            (command, form)
        });
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            let state = panel.forms.get(&form).expect("inputs for the form");
            assert!(state.inputs.contains_key("namespace"));
        });

        service.update(cx, |service, cx| {
            service.answer(thread, command, Answer::Deny, cx)
        });
        let fields = service.read_with(cx, |service, _| {
            let pending = &service.thread(thread).unwrap().pending;
            assert_eq!(pending.len(), 1);
            match &pending[0].kind {
                PendingKind::Form { fields, .. } => fields.clone(),
                other => panic!("{other:?}"),
            }
        });

        // A required field left empty: the form stays and says why.
        panel.update(cx, |panel, cx| panel.submit_form(thread, form, &fields, cx));
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.forms[&form].errors["namespace"], "Required.");
        });
        panel.update_in(cx, |panel, window, cx| {
            let input = panel.forms[&form].inputs["namespace"].clone();
            input.update(cx, |input, cx| input.set_value("shop", window, cx));
            panel.submit_form(thread, form, &fields, cx);
        });
        cx.run_until_parked();
        service.read_with(cx, |service, _| {
            assert!(service.thread(thread).unwrap().pending.is_empty());
        });
        panel.read_with(cx, |panel, _| assert!(panel.forms.is_empty()));
    }

    #[gpui::test]
    fn settings_and_context_use_show_when_the_agent_reports_them(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        let thread = start_thread(&panel, cx, "hi");
        let service = service(cx);
        service.update(cx, |service, cx| {
            let transcript = service.core_mut().transcript_for_tests(thread).unwrap();
            let update: agent_client_protocol_schema::v1::SessionUpdate = serde_json::from_value(json!({
                "sessionUpdate": "config_option_update", "configOptions": [
                    {"id": "model", "name": "Model", "category": "model", "type": "select",
                     "currentValue": "opus", "options": [{"value": "opus", "name": "Opus"}, {"value": "sonnet", "name": "Sonnet"}]},
                    {"id": "effort", "name": "Thinking", "category": "thought_level", "type": "select",
                     "currentValue": "high", "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
                    {"id": "fast", "name": "Fast", "type": "boolean", "currentValue": true}]}))
            .unwrap();
            transcript.apply(update);
            let usage: agent_client_protocol_schema::v1::SessionUpdate = serde_json::from_value(
                json!({"sessionUpdate": "usage_update", "used": 170_000, "size": 200_000}),
            )
            .unwrap();
            transcript.apply(usage);
            cx.notify();
        });
        cx.run_until_parked();
        let colors = cx.update(|_, cx| cx.colors().clone());
        service.read_with(cx, |service, _| {
            let thread = service.thread(thread).unwrap();
            assert!(session_settings(thread, &colors).is_some());
            assert_eq!(thread.transcript.usage_percent(), Some(85));
            assert!(context_use(thread, &colors).is_some());
        });
        assert_eq!(tokens(170_000), "170k");
        assert_eq!(tokens(1_500_000), "1.5M");
    }

    #[gpui::test]
    fn the_status_item_jumps_to_the_waiting_thread(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        let waiting = start_thread(&panel, cx, "first");
        panel.update(cx, |panel, cx| panel.new_thread(None, cx));
        let other = start_thread(&panel, cx, "second");
        assert_ne!(waiting, other);
        let service = service(cx);
        service.update(cx, |service, cx| {
            service.core_mut().push_pending_for_tests(
                waiting,
                PendingKind::Read {
                    path: "/etc/hosts".into(),
                },
            );
            cx.notify();
        });
        panel.update(cx, |panel, cx| {
            panel.show_threads = true;
            cx.notify();
        });
        cx.update(crate::status::show_attention);
        cx.run_until_parked();
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.thread, Some(waiting));
            assert!(!panel.show_threads);
        });
    }

    #[gpui::test]
    fn mentions_complete_the_name_and_attach_the_object(cx: &mut TestAppContext) {
        let (_dir, panel, cx) = open(cx);
        let cluster = ClusterId::new("kind-dev");
        panel.update_in(cx, |panel, window, cx| {
            panel.draft_cluster = Some(cluster.clone());
            panel.input.update(cx, |input, cx| input.set_value("look at @we", window, cx));
            // No watch caches in this test: nothing to offer.
            panel.update_mentions(cx);
            assert!(panel.mentions.is_empty());
            panel.mentions = vec![Mention {
                candidate: mentions::Candidate {
                    kind: "Pod".into(),
                    namespace: Some("shop".into()),
                    name: "web-0".into(),
                },
                target: kubyl_core::ResourceRef::object(
                    cluster.clone(),
                    kubyl_core::Gvr::new("", "v1", "pods"),
                    Some("shop".into()),
                    "web-0".into(),
                ),
                object: Some(std::sync::Arc::new(json!({
                    "metadata": {"name": "web-0", "namespace": "shop"},
                    "spec": {"containers": [{"name": "web", "env": [{"name": "DB_PASSWORD", "value": "hunter22"}]}]},
                }))),
            }];
            // Enter picks the first offer instead of sending.
            panel.submit(window, cx);
        });
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.input.read(cx).value(), "look at @web-0 ");
            assert!(panel.mentions.is_empty());
            assert_eq!(panel.chips.len(), 1);
            assert_eq!(panel.chips[0].label, "Pod shop/web-0");
            assert!(
                !panel.chips[0].text.contains("hunter22"),
                "{}",
                panel.chips[0].text
            );
            assert!(panel.thread.is_none(), "picking doesn't send");
        });
    }
}

/// How full the agent's context window is (`ctx 26%`), when the agent reports it: yellow from
/// 80 %, red from 95 %, with tokens and cost in the tooltip.
fn context_use(thread: &Thread, colors: &Colors) -> Option<AnyElement> {
    let percent = thread.transcript.usage_percent()?;
    let (used, size) = thread.transcript.usage?;
    let color = match percent {
        95.. => colors.red,
        80.. => colors.yellow,
        _ => colors.text_faint,
    };
    let mut tip = format!("Context: {} of {} tokens", tokens(used), tokens(size));
    if let Some((amount, currency)) = &thread.transcript.cost {
        tip.push_str(&format!(" · {amount:.2} {currency} so far"));
    }
    Some(
        div()
            .id(("agent-context", thread.id.0))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            .child(small(format!("ctx {percent}%"), color))
            .into_any_element(),
    )
}

/// `53k`, `1.2M`.
fn tokens(n: u64) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1_000_000.0),
        1_000.. => format!("{}k", n / 1_000),
        _ => n.to_string(),
    }
}

/// The session's settings the agent offers: model, thinking level, mode and others as
/// dropdowns, on/off settings as toggles. Agents with only the older session modes get a mode
/// dropdown. `None` when the agent offers nothing.
fn session_settings(thread: &Thread, colors: &Colors) -> Option<AnyElement> {
    let mut controls: Vec<AnyElement> = Vec::new();
    let id = thread.id;
    for (i, option) in thread.transcript.config.iter().enumerate() {
        controls.push(match &option.value {
            ConfigValue::Select { .. } => config_dropdown(id, i, option, colors),
            ConfigValue::Bool(on) => {
                let (config, on) = (option.id.clone(), *on);
                div()
                    .id(("agent-config-bool", i))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        if let Some(service) = AgentService::global(cx) {
                            let config = config.clone();
                            service.update(cx, |s, cx| {
                                s.set_config(id, config, ConfigValue::Bool(!on), cx)
                            });
                        }
                    })
                    .child(Chip::new(option.name.clone()).selected(on))
                    .into_any_element()
            }
        });
    }
    let has_mode = thread
        .transcript
        .config
        .iter()
        .any(|o| o.category == ConfigCategory::Mode);
    if !has_mode && thread.modes.len() > 1 {
        let current = thread.transcript.mode.clone();
        let label = thread
            .modes
            .iter()
            .find(|(m, _)| Some(m) == current.as_ref())
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| "Mode".into());
        let modes = thread.modes.clone();
        controls.push(
            dropdown_button(
                ("agent-legacy-mode", 0usize),
                IconName::SlidersVertical,
                label,
                colors,
            )
            .dropdown_menu(move |menu, _, _| {
                modes.iter().fold(menu, |menu, (mode, name)| {
                    let mode = mode.clone();
                    menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(current.as_ref() == Some(&mode))
                            .on_click(move |_, _, cx| {
                                if let Some(service) = AgentService::global(cx) {
                                    let mode = mode.clone();
                                    service.update(cx, |s, cx| s.set_mode(id, mode, cx));
                                }
                            }),
                    )
                })
            })
            .into_any_element(),
        );
    }
    (!controls.is_empty()).then(|| {
        h_flex()
            .flex_wrap()
            .gap(u(4.0))
            .children(controls)
            .into_any_element()
    })
}

fn dropdown_button(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    label: String,
    colors: &Colors,
) -> MenuButton {
    MenuButton::new(id).ghost().compact().child(
        h_flex()
            .gap(u(4.0))
            .text_size(u(12.0))
            .text_color(colors.text_muted)
            .child(Icon::new(icon).size(12.0))
            .child(label)
            .child(Icon::new(IconName::ChevronDown).size(10.0)),
    )
}

/// One select setting as a dropdown, its choices grouped as the agent groups them.
fn config_dropdown(
    thread: ThreadId,
    index: usize,
    option: &ConfigOption,
    colors: &Colors,
) -> AnyElement {
    let ConfigValue::Select { current, choices } = &option.value else {
        return div().into_any_element();
    };
    let icon = match option.category {
        ConfigCategory::Model | ConfigCategory::ModelConfig => IconName::Cpu,
        ConfigCategory::ThoughtLevel => IconName::Activity,
        ConfigCategory::Mode => IconName::SlidersVertical,
        ConfigCategory::Other(_) => IconName::Settings,
    };
    let (config, current, choices) = (option.id.clone(), current.clone(), choices.clone());
    let title = option.name.clone();
    let button = dropdown_button(
        ("agent-config", index),
        icon,
        option.current_label(),
        colors,
    )
    .dropdown_menu(move |menu, _, _| {
        let mut menu = menu.max_h(px(320.0)).scrollable(true).label(title.clone());
        let mut group: Option<String> = None;
        for choice in &choices {
            if choice.group != group {
                if let Some(name) = &choice.group {
                    menu = menu.separator().label(name.clone());
                }
                group = choice.group.clone();
            }
            let (config, value) = (config.clone(), choice.value.clone());
            menu = menu.item(
                PopupMenuItem::new(choice.name.clone())
                    .checked(choice.value == current)
                    .on_click(move |_, _, cx| {
                        if let Some(service) = AgentService::global(cx) {
                            let value = ConfigValue::Select {
                                current: value.clone(),
                                choices: Vec::new(),
                            };
                            let config = config.clone();
                            service.update(cx, |s, cx| s.set_config(thread, config, value, cx));
                        }
                    }),
            );
        }
        menu
    });
    match &option.description {
        Some(description) => {
            let tip = format!("{}: {description}", option.name);
            div()
                .id(("agent-config-tip", index))
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .child(button)
                .into_any_element()
        }
        None => button.into_any_element(),
    }
}

/// The heading of a permission prompt. Agents often use the command itself as the title; then
/// the command block below says it all.
fn permission_title(title: &str, command: Option<&str>) -> String {
    let title = title.trim();
    match command {
        Some(command)
            if title.is_empty()
                || command.trim().starts_with(title)
                || title.starts_with(command.trim()) =>
        {
            "The agent wants to run a command".into()
        }
        _ => format!("The agent asks: {title}"),
    }
}
