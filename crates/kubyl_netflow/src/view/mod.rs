//! The Network Flows view (board 18): one tab per cluster (and namespace scope) with the live
//! flow table, its filter bar and flow details, and the topology.

mod details;
mod filter_bar;
pub mod rows;
mod states;
mod table;
pub mod topology;
pub mod widgets;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, Render, SharedString, Subscription, Task, UniformListScrollHandle, Window, div,
    prelude::*,
};
use gpui_component::input::{InputEvent, InputState};
use jiff::Timestamp;
use kubyl_charts::TimeRange;
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_settings::State;
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, ProdBadge, fonts, h_flex, sizes, u, v_flex};

use crate::filter::{FlowFilter, ParseError, Suggestion};
use crate::model::{Flow, Verdict};
use crate::service::{FlowLease, FlowService, FlowState, StreamStatus};
use crate::settings::NetflowState;
use rows::{Rows, SplitFilter};

/// `ViewKind::Custom` of the Network Flows tab.
pub const VIEW_KIND: &str = "network_flows";

/// Key contexts: the view, the table and the topology (their single-key bindings never fire
/// while typing in the filter).
pub const VIEW_CONTEXT: &str = "NetworkFlowsView";
pub const LIST_CONTEXT: &str = "NetworkFlows";
pub const GRAPH_CONTEXT: &str = "NetworkTopology";

/// How long typing waits before the backend's server-side filter changes.
const PUSH_DELAY: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Flows,
    Topology,
}

/// What the next view of a cluster (and namespace) opens with.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub tab: Option<Tab>,
    /// A filter text (`pod=payments/checkout-api`).
    pub query: Option<String>,
}

#[derive(Default)]
struct PendingOpens(HashMap<(ClusterId, Option<String>), Pending>);

impl Global for PendingOpens {}

fn request(cluster: &ClusterId, namespace: Option<&str>) -> ViewRequest {
    ViewRequest::for_resource(
        ViewKind::Custom(VIEW_KIND.into()),
        ResourceRef::list(
            cluster.clone(),
            Gvr::new("", "", ""),
            namespace.map(str::to_string),
        ),
    )
}

/// Opens (or focuses) the Network Flows tab of `cluster`, scoped to `namespace`.
pub fn open(
    cluster: &ClusterId,
    namespace: Option<&str>,
    pending: Pending,
    window: &mut Window,
    cx: &mut App,
) {
    cx.default_global::<PendingOpens>()
        .0
        .insert((cluster.clone(), namespace.map(str::to_string)), pending);
    window.dispatch_action(Box::new(OpenView(request(cluster, namespace))), cx);
    cx.refresh_windows();
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let target = request.target.as_ref()?;
            let cluster = target.cluster.clone();
            let namespace = target.namespace.clone();
            Some(Box::new(cx.new(|cx| {
                NetworkFlowsView::new(cluster, namespace, window, cx)
            })))
        },
    );
}

pub struct NetworkFlowsView {
    pub(crate) cluster: ClusterId,
    /// The namespace scope (either side), from the request; `None`: the whole cluster.
    pub(crate) namespace: Option<String>,
    pub(crate) tab: Tab,
    pub(crate) input: Entity<InputState>,
    pub(crate) parse_error: Option<ParseError>,
    /// The last filter that parsed.
    pub(crate) user_filter: FlowFilter,
    pub(crate) window: TimeRange,
    pub(crate) paused: bool,
    cluster_lease: Option<FlowLease>,
    stream_lease: Option<FlowLease>,
    /// The server-side part in use.
    pub(crate) pushed: FlowFilter,
    /// A stream for a new server-side part, taking over once its history arrived (the rows
    /// of the current one stay until then).
    next_stream: Option<(FlowFilter, FlowLease)>,
    push_after: Option<Instant>,
    pub(crate) rows: Rows,
    /// What `rows` were built for: stream, base filter, window.
    rows_key: Option<(String, String, TimeRange)>,
    rows_revision: u64,
    scan_generation: u64,
    scan: Option<Task<()>>,
    /// The rows key of the scan in flight.
    scan_key: Option<(String, String, TimeRange)>,
    pub(crate) scanning: bool,
    pub(crate) selected: Option<u64>,
    pub(crate) details_open: bool,
    pub(crate) suggestions: Vec<Suggestion>,
    pub(crate) suggestion: usize,
    pub(crate) focus: FocusHandle,
    pub(crate) graph_focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) topology: topology::TopologyState,
    pending_input: Option<String>,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl NetworkFlowsView {
    pub fn new(
        cluster: ClusterId,
        namespace: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let options = State::get::<NetflowState>(cx);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("ns=payments verdict=dropped port=443, or any text")
        });
        let mut subscriptions = vec![cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    this.query_changed(&text, cx);
                }
                InputEvent::PressEnter { .. } if !this.accept_suggestion(window, cx) => {
                    this.focus_active(window, cx)
                }
                _ => {}
            },
        )];
        if let Some(service) = FlowService::global(cx) {
            subscriptions.push(cx.observe(&service, |this, _, cx| this.sync(cx)));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |this, _, cx| this.sync(cx)));
        }
        subscriptions.push(cx.observe_global::<ActiveContext>(|_, cx| cx.notify()));
        // Expiry, rates and "live" stay current.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |this, cx| this.sync(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            cluster,
            namespace,
            tab: Tab::Flows,
            input,
            parse_error: None,
            user_filter: FlowFilter::default(),
            window: TimeRange::from_label(&options.window).unwrap_or(TimeRange::M15),
            paused: false,
            cluster_lease: None,
            stream_lease: None,
            pushed: FlowFilter::default(),
            next_stream: None,
            push_after: None,
            rows: Rows::default(),
            rows_key: None,
            rows_revision: 0,
            scan_generation: 0,
            scan: None,
            scan_key: None,
            scanning: false,
            selected: None,
            details_open: options.details_open,
            suggestions: Vec::new(),
            suggestion: 0,
            focus: cx.focus_handle(),
            graph_focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            topology: topology::TopologyState::new(if options.zoom == "workloads" {
                crate::aggregate::Zoom::Workloads
            } else {
                crate::aggregate::Zoom::Namespaces
            }),
            pending_input: None,
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        this.take_pending(cx);
        this.sync(cx);
        this
    }

    pub fn cluster(&self) -> &ClusterId {
        &self.cluster
    }

    fn take_pending(&mut self, cx: &mut Context<Self>) {
        let key = (self.cluster.clone(), self.namespace.clone());
        let Some(pending) = cx
            .try_global::<PendingOpens>()
            .and_then(|p| p.0.get(&key).cloned())
        else {
            return;
        };
        cx.default_global::<PendingOpens>().0.remove(&key);
        if let Some(tab) = pending.tab {
            self.tab = tab;
        }
        if let Some(query) = pending.query {
            self.query_changed(&query, cx);
            self.push_after = None;
            self.pending_input = Some(query);
        }
    }

    // ----- Filter -----

    /// The scope and the user's filter.
    pub(crate) fn effective_filter(&self) -> FlowFilter {
        match &self.namespace {
            Some(ns) => FlowFilter::namespace(ns).and(&self.user_filter),
            None => self.user_filter.clone(),
        }
    }

    pub(crate) fn query_changed(&mut self, text: &str, cx: &mut Context<Self>) {
        match FlowFilter::parse(text) {
            Ok(filter) => {
                self.parse_error = None;
                if filter != self.user_filter {
                    self.user_filter = filter;
                    self.push_after = Some(Instant::now() + PUSH_DELAY);
                    let weak = cx.entity().downgrade();
                    cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(PUSH_DELAY).await;
                        weak.update(cx, |this, cx| this.sync(cx)).ok();
                    })
                    .detach();
                }
            }
            Err(err) => self.parse_error = Some(err),
        }
        self.update_suggestions(text, cx);
        self.sync(cx);
    }

    /// Replaces the filter text (chips, clicks in the table and graph).
    pub(crate) fn set_query(&mut self, text: String, cx: &mut Context<Self>) {
        self.query_changed(&text, cx);
        self.pending_input = Some(text);
        self.suggestions.clear();
        cx.notify();
    }

    /// Adds `term` to the filter (replacing a term of the same key and side).
    pub(crate) fn add_term(&mut self, term: &str, cx: &mut Context<Self>) {
        let Ok(parsed) = FlowFilter::parse(term) else {
            return;
        };
        let mut filter = self.user_filter.clone();
        for new in &parsed.terms {
            filter
                .terms
                .retain(|t| !(t.field == new.field && t.side == new.side));
        }
        filter = filter.and(&parsed);
        self.set_query(filter.canonical(), cx);
    }

    fn update_suggestions(&mut self, text: &str, cx: &mut Context<Self>) {
        let service = FlowService::global(cx);
        self.suggestions = match service
            .as_ref()
            .and_then(|s| s.read(cx).stream(&self.cluster, &self.pushed))
        {
            Some(stream) => crate::filter::complete(text, text.len(), stream.buffer.seen(), 8),
            None => crate::filter::complete(text, text.len(), &NoValues, 8),
        };
        self.suggestion = 0;
    }

    /// Takes the highlighted completion; `false` when there's none.
    pub(crate) fn accept_suggestion(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(suggestion) = self.suggestions.get(self.suggestion).cloned() else {
            return false;
        };
        let text = self.input.read(cx).value().to_string();
        let mut next = String::new();
        next.push_str(&text[..suggestion.range.start.min(text.len())]);
        next.push_str(&suggestion.text);
        next.push_str(&text[suggestion.range.end.min(text.len())..]);
        // A complete value moves on to the next term.
        if !suggestion.text.ends_with('=') && !next.ends_with(' ') {
            next.push(' ');
        }
        self.input
            .update(cx, |input, cx| input.set_value(next.clone(), window, cx));
        self.query_changed(&next, cx);
        true
    }

    // ----- Data -----

    /// Takes the service's state: leases, the stream, the rows.
    pub(crate) fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(service) = FlowService::global(cx) else {
            return;
        };
        let state = service.read(cx).state(&self.cluster, cx);
        if matches!(state, FlowState::NotConnected) {
            self.cluster_lease = None;
            self.stream_lease = None;
            cx.notify();
            return;
        }
        if self.cluster_lease.is_none() {
            let cluster = self.cluster.clone();
            self.cluster_lease = Some(service.update(cx, |s, cx| s.watch(&cluster, cx)));
        }
        if matches!(state, FlowState::Ready { .. }) {
            let wanted = service
                .read(cx)
                .pushdown(&self.cluster, &self.effective_filter());
            let typing = self.push_after.is_some_and(|t| Instant::now() < t);
            let (cluster, window) = (self.cluster.clone(), self.window.duration());
            if self.stream_lease.is_none() {
                self.pushed = wanted;
                self.next_stream = None;
                let pushed = self.pushed.clone();
                self.stream_lease =
                    Some(service.update(cx, |s, cx| s.lease(&cluster, &pushed, window, cx)));
            } else if wanted == self.pushed {
                self.next_stream = None;
            } else if !typing
                && self
                    .next_stream
                    .as_ref()
                    .is_none_or(|(next, _)| *next != wanted)
            {
                let lease = service.update(cx, |s, cx| s.lease(&cluster, &wanted, window, cx));
                self.next_stream = Some((wanted, lease));
            }
            self.hand_over(cx);
        } else {
            self.stream_lease = None;
            self.next_stream = None;
        }
        self.refresh_rows(cx);
        cx.notify();
    }

    /// Switches to the next stream once its history arrived (or it failed: its error shows):
    /// its rows are built in the background first, and the current ones stay until then.
    fn hand_over(&mut self, cx: &mut Context<Self>) {
        let Some(service) = FlowService::global(cx) else {
            return;
        };
        let Some((next, _)) = &self.next_stream else {
            return;
        };
        let service = service.read(cx);
        if service.stream(&self.cluster, &self.pushed).is_none() {
            if let Some((next, lease)) = self.next_stream.take() {
                self.pushed = next;
                self.stream_lease = Some(lease);
                self.selected = None;
            }
            return;
        }
        let Some(stream) = service
            .stream(&self.cluster, next)
            .filter(|s| s.caught_up || matches!(s.status, StreamStatus::Retrying(_)))
        else {
            return;
        };
        let split = SplitFilter::new(&self.effective_filter());
        let key = (next.canonical(), split.base.canonical(), self.window);
        if self.scan_key.as_ref() != Some(&key) {
            self.start_scan(stream.buffer.snapshot(), split, key, stream.revision, cx);
        }
    }

    /// Takes the next stream's rows (a finished scan for it), keeping the selected flow when
    /// the new stream has it too.
    fn take_next_stream(&mut self, cx: &mut Context<Self>) {
        let Some((next, lease)) = self.next_stream.take() else {
            return;
        };
        if let Some(service) = FlowService::global(cx) {
            let service = service.read(cx);
            let flow = self.selected.and_then(|seq| {
                service
                    .stream(&self.cluster, &self.pushed)?
                    .buffer
                    .get(seq)
                    .cloned()
            });
            self.selected = flow.and_then(|flow| {
                service
                    .stream(&self.cluster, &next)?
                    .buffer
                    .since(0)
                    .find(|(_, candidate)| candidate.time == flow.time && **candidate == flow)
                    .map(|(seq, _)| seq)
            });
        }
        self.pushed = next;
        self.stream_lease = Some(lease);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let Some(service) = FlowService::global(cx) else {
            return;
        };
        let split = SplitFilter::new(&self.effective_filter());
        let key = (self.pushed.canonical(), split.base.canonical(), self.window);
        let service = service.read(cx);
        let Some(stream) = service.stream(&self.cluster, &self.pushed) else {
            self.rows = Rows::default();
            self.rows_key = None;
            return;
        };
        if self.rows_key.as_ref() != Some(&key) {
            if self.scan_key.as_ref() != Some(&key) {
                self.start_scan(stream.buffer.snapshot(), split, key, stream.revision, cx);
            }
            return;
        }
        if self.rows.filter().verdicts != split.verdicts {
            self.rows.set_verdicts(split);
        }
        // Every tick too: flows age out of the window.
        // The backend's history joins in place.
        let follow = self.following() || !stream.caught_up;
        self.rows.update(
            &stream.buffer,
            Timestamp::now(),
            self.window.duration(),
            follow,
        );
        self.rows_revision = stream.revision;
        if self
            .selected
            .is_some_and(|seq| stream.buffer.get(seq).is_none())
        {
            self.selected = None;
        }
    }

    /// Re-filters the whole buffer in the background.
    fn start_scan(
        &mut self,
        snapshot: crate::buffer::Snapshot,
        split: SplitFilter,
        key: (String, String, TimeRange),
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        self.scanning = true;
        self.scan_key = Some(key.clone());
        let window = self.window.duration();
        let cutoff = rows::cutoff(Timestamp::now(), window);
        let task = cx.spawn(async move |this, cx| {
            let filter = split.clone();
            let scanned = cx
                .background_spawn(async move { rows::scan(&snapshot, &filter, cutoff) })
                .await;
            this.update(cx, |this, cx| {
                if this.scan_generation != generation {
                    return;
                }
                this.scanning = false;
                this.scan_key = None;
                if this
                    .next_stream
                    .as_ref()
                    .is_some_and(|(next, _)| next.canonical() == key.0)
                {
                    this.take_next_stream(cx);
                }
                this.rows = Rows::from_scan(split, scanned);
                this.rows_key = Some(key);
                this.rows_revision = revision;
                this.refresh_rows(cx);
                cx.notify();
            })
            .ok();
        });
        // Replaced from outside the task only (a new filter): never drops itself.
        self.scan = Some(task);
    }

    /// New rows join the table at once: it's live, at the top, and no row is open in the
    /// details (that one would move away).
    pub(crate) fn following(&self) -> bool {
        !self.paused && self.at_top() && !(self.details_open && self.selected.is_some())
    }

    /// The table shows its newest rows.
    pub(crate) fn at_top(&self) -> bool {
        let offset = self.scroll.0.borrow().base_handle.offset();
        f32::from(offset.y) > -2.0
    }

    pub(crate) fn flow(&self, seq: u64, cx: &App) -> Option<Arc<Flow>> {
        FlowService::global(cx)?
            .read(cx)
            .stream(&self.cluster, &self.pushed)?
            .buffer
            .get(seq)
            .cloned()
    }

    pub(crate) fn selected_flow(&self, cx: &App) -> Option<Arc<Flow>> {
        self.flow(self.selected?, cx)
    }

    pub(crate) fn state(&self, cx: &App) -> FlowState {
        FlowService::global(cx)
            .map(|s| s.read(cx).state(&self.cluster, cx))
            .unwrap_or(FlowState::NotConnected)
    }

    // ----- Actions -----

    pub(crate) fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.suggestions.clear();
        match self.tab {
            Tab::Flows => self.focus.focus(window, cx),
            Tab::Topology => self.graph_focus.focus(window, cx),
        }
    }

    pub(crate) fn set_window(&mut self, range: TimeRange, cx: &mut Context<Self>) {
        self.window = range;
        State::update::<NetflowState>(cx, |s| s.window = range.label().to_string());
        // A longer window asks for more history.
        self.stream_lease = None;
        self.sync(cx);
    }

    pub(crate) fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.paused = !self.paused;
        if !self.paused {
            self.rows.resume();
        }
        cx.notify();
    }

    /// Back to the newest flows (and live).
    pub(crate) fn show_newest(&mut self, cx: &mut Context<Self>) {
        self.paused = false;
        self.selected = None;
        self.rows.resume();
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        cx.notify();
    }

    pub(crate) fn select_seq(&mut self, seq: Option<u64>, cx: &mut Context<Self>) {
        self.selected = seq;
        if let Some(index) = seq.and_then(|s| self.rows.position(s)) {
            self.scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub(crate) fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.shown.is_empty() {
            return;
        }
        let current = self.selected.and_then(|s| self.rows.position(s));
        let next = match current {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, self.rows.shown.len() as isize - 1) as usize,
        };
        let seq = self.rows.shown.get(next).copied();
        self.select_seq(seq, cx);
    }

    pub(crate) fn set_details_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.details_open = open;
        State::update::<NetflowState>(cx, |s| s.details_open = open);
        cx.notify();
    }

    /// Filters to the selected flow's connection, source or destination.
    pub(crate) fn filter_to_selected(&mut self, part: SelectedPart, cx: &mut Context<Self>) {
        let Some(flow) = self.selected_flow(cx) else {
            return;
        };
        let side = |endpoint: &crate::model::Endpoint, prefix: &str| -> Option<String> {
            match (&endpoint.namespace, &endpoint.pod, endpoint.ip) {
                (Some(ns), Some(pod), _) if !pod.ends_with('*') => {
                    Some(format!("{prefix}pod={ns}/{pod}"))
                }
                (Some(ns), _, _) => endpoint
                    .workload_name()
                    .map(|w| format!("{prefix}workload={ns}/{}", w.trim_end_matches("-*")))
                    .or_else(|| Some(format!("{prefix}ns={ns}"))),
                (None, _, Some(ip)) => Some(format!("{prefix}ip={ip}")),
                _ => None,
            }
        };
        let mut terms = Vec::new();
        match part {
            SelectedPart::Source => terms.extend(side(&flow.source, "src.")),
            SelectedPart::Destination => terms.extend(side(&flow.destination, "dst.")),
            SelectedPart::Connection => {
                terms.extend(side(&flow.source, "src."));
                terms.extend(side(&flow.destination, "dst."));
                if let Some(port) = flow.destination.port {
                    terms.push(format!("dst.port={port}"));
                }
            }
        }
        if !terms.is_empty() {
            self.add_term(&terms.join(" "), cx);
        }
    }

    pub(crate) fn production(&self, cx: &App) -> bool {
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(&self.cluster).production)
    }

    pub(crate) fn cluster_name(&self, cx: &App) -> SharedString {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&self.cluster))
            .unwrap_or_else(|| self.cluster.to_string().into())
    }

    // ----- Rendering -----

    fn render_header(
        &mut self,
        state: &FlowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let backend = filter_bar::backend_chip(self, state, &colors, cx);
        let others = filter_bar::other_backends(self, state, &colors, cx);
        let live = filter_bar::live_chip(self, state, &colors, cx);
        let picker = {
            let weak = cx.entity().downgrade();
            kubyl_charts::TimeRangePicker::new("flows-window", self.window).on_change(
                move |range, _, cx| {
                    weak.update(cx, |this, cx| this.set_window(range, cx)).ok();
                },
            )
        };
        let _ = window;
        h_flex()
            .flex_none()
            .h(u(44.0))
            .px(u(16.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .text_size(u(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.cluster_name(cx)),
            )
            .when(self.production(cx), |this| this.child(ProdBadge))
            .when_some(self.namespace.clone(), |this, ns| {
                this.child(
                    h_flex()
                        .flex_none()
                        .gap(u(5.0))
                        .text_color(colors.text_muted)
                        .child(Icon::new(IconName::Folder).size(13.0))
                        .child(ns),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(u(8.0))
                    .overflow_hidden()
                    .child(backend)
                    .children(others),
            )
            .children(live)
            .when(matches!(state, FlowState::Ready { .. }), |this| {
                let weak = cx.entity().downgrade();
                this.child(
                    kubyl_ui::IconButton::new(
                        "flows-pause",
                        if self.paused {
                            IconName::Play
                        } else {
                            IconName::Pause
                        },
                    )
                    .icon_size(13.0)
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.toggle_pause(cx)).ok();
                    }),
                )
            })
            .child(picker)
            .child(filter_bar::more_menu(self, &colors, cx))
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let flows = self.rows.shown.len() + self.rows.pending.len();
        let graph = self.topology.summary();
        let tab = |id: &'static str,
                   tab: Tab,
                   icon: IconName,
                   label: &'static str,
                   count: Option<String>,
                   cx: &mut Context<Self>| {
            let active = self.tab == tab;
            h_flex()
                .id(id)
                .h_full()
                .px(u(10.0))
                .gap(u(7.0))
                .cursor_pointer()
                .text_size(u(13.0))
                .text_color(if active {
                    colors.text
                } else {
                    colors.text_muted
                })
                .border_b_2()
                .border_color(if active {
                    colors.accent
                } else {
                    gpui::transparent_black()
                })
                .child(Icon::new(icon).size(13.0).color(if active {
                    colors.accent
                } else {
                    colors.text_dim
                }))
                .child(label)
                .when_some(count, |this, count| {
                    this.child(
                        div()
                            .px(u(6.0))
                            .rounded(u(4.0))
                            .bg(colors.chip_background)
                            .font_family(fonts::MONO)
                            .text_size(u(11.0))
                            .text_color(colors.text_muted)
                            .child(count),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
        };
        h_flex()
            .flex_none()
            .h(u(34.0))
            .px(u(8.0))
            .gap(u(4.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(tab(
                "flows-tab-flows",
                Tab::Flows,
                IconName::List,
                "Flows",
                Some(widgets::count(flows)),
                cx,
            ))
            .child(tab(
                "flows-tab-topology",
                Tab::Topology,
                IconName::Waypoints,
                "Topology",
                graph,
                cx,
            ))
            .into_any_element()
    }

    fn hints(&self, blocked: bool, cx: &App) -> Vec<(SharedString, SharedString)> {
        let mut hints: Vec<(SharedString, SharedString)> = if blocked {
            Vec::new()
        } else {
            match self.tab {
                Tab::Flows => kubyl_core::ActionRegistry::global(cx).hints(LIST_CONTEXT),
                Tab::Topology => {
                    // The mouse first (as on the board); enter shows flows like a double click.
                    let mut hints: Vec<(SharedString, SharedString)> = vec![
                        ("click".into(), "Select".into()),
                        ("double click".into(), "Show flows".into()),
                        ("drag".into(), "Pan".into()),
                        ("scroll".into(), "Zoom".into()),
                    ];
                    hints.extend(
                        kubyl_core::ActionRegistry::global(cx)
                            .hints(GRAPH_CONTEXT)
                            .into_iter()
                            .filter(|(_, label)| label != "Show flows"),
                    );
                    hints
                }
            }
        };
        hints.push(("/".into(), "Filter".into()));
        hints
    }
}

/// Which part of the selected flow a filter takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedPart {
    Connection,
    Source,
    Destination,
}

/// Completion without a stream: keys only.
struct NoValues;

impl crate::filter::SeenValues for NoValues {
    fn values(&self, _: crate::filter::Field) -> Vec<(String, u32)> {
        Vec::new()
    }
}

impl Focusable for NetworkFlowsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        match self.tab {
            Tab::Flows => self.focus.clone(),
            Tab::Topology => self.graph_focus.clone(),
        }
    }
}

impl TabView for NetworkFlowsView {
    fn tab_title(&self, cx: &App) -> SharedString {
        let active =
            ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(&self.cluster);
        let mut title = String::from("Network Flows");
        if let Some(ns) = &self.namespace {
            title.push_str(&format!(" · {ns}"));
        }
        if !active {
            title.push_str(&format!(" · {}", self.cluster_name(cx)));
        }
        title.into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Waypoints.path())
    }

    fn tab_dot(&self, cx: &App) -> Option<gpui::Hsla> {
        let active = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone());
        (active.as_ref() != Some(&self.cluster))
            .then(|| ConnectionManager::try_global(cx).map(|m| m.read(cx).color(&self.cluster, cx)))
            .flatten()
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(request(&self.cluster, self.namespace.as_deref()))
    }
}

impl Render for NetworkFlowsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.take_pending(cx);
        if let Some(text) = self.pending_input.take() {
            self.input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        let colors: Colors = cx.colors().clone();
        let state = self.state(cx);
        let blocking = states::blocking(self, &state, cx);
        let header = self.render_header(&state, window, cx);
        let tabs = self.render_tabs(cx);
        let body = match blocking {
            Some(blocking) => states::render(self, blocking, &state, &colors, cx),
            None => match self.tab {
                Tab::Flows => table::render(self, &state, window, cx),
                Tab::Topology => topology::render(self, &state, window, cx),
            },
        };
        let hints = self.hints(states::blocking(self, &state, cx).is_some(), cx);
        v_flex()
            .key_context(VIEW_CONTEXT)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .map(|this| crate::actions::bind_view_actions(this, cx))
            .child(header)
            .child(tabs)
            .child(filter_bar::render(self, &state, window, cx))
            .child(div().flex_1().min_h_0().flex().child(body))
            .child(kubyl_ui::KeyHints::new(hints))
    }
}

/// Counts of the verdict chips.
pub(crate) fn verdict_counts(view: &NetworkFlowsView) -> Vec<(Verdict, usize)> {
    [Verdict::Forwarded, Verdict::Dropped, Verdict::NoReply]
        .into_iter()
        .map(|v| (v, view.rows.count(v)))
        .collect()
}

/// The stream's status line (`live · 184 flows/s`).
pub(crate) fn stream_status(
    view: &NetworkFlowsView,
    cx: &App,
) -> Option<(StreamStatus, f64, bool)> {
    let service = FlowService::global(cx)?;
    let service = service.read(cx);
    let stream = service.stream(&view.cluster, &view.pushed)?;
    Some((stream.status.clone(), stream.rate(), stream.caught_up))
}
