//! The Alerts view (board 16): one tab per cluster, or one over every connected cluster, with
//! the Alerts, Silences and Rules tabs, the details pane, and the states that explain an empty
//! list.

mod alerts_tab;
mod details;
pub mod rows;
mod rules_tab;
mod silences_tab;
mod states;
pub mod widgets;

pub(crate) use alerts_tab::heartbeat_line as heartbeat;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, Render, ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle,
    Window, div, prelude::*,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind, ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_settings::State;
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, ProdBadge, fonts, h_flex, sizes, u, v_flex};

use crate::model::{Alert, Rule, Silence};
use crate::service::{AlertsService, ClusterAlerts, Pace, Phase};
use crate::settings::AlertsState;
use rows::{Filters, Query, Row};

/// `ViewKind::Custom` of a cluster's Alerts tab.
pub const VIEW_KIND: &str = "alerts";
/// `ViewKind::Custom` of the all-clusters Alerts tab.
pub const ALL_KIND: &str = "alerts_all";

/// Key contexts: the view, and the list of each tab (the keys of a tab only work there, never
/// in the filter inputs).
pub const VIEW_CONTEXT: &str = "AlertsView";
pub const ALERTS_CONTEXT: &str = "Alerts";
pub const SILENCES_CONTEXT: &str = "AlertSilences";
pub const RULES_CONTEXT: &str = "AlertRules";

/// The tabs of the view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Alerts,
    Silences,
    Rules,
}

/// What the next Alerts view of a cluster opens with (a tab, a filter, an alert).
#[derive(Clone, Debug, Default)]
pub struct Pending {
    pub tab: Option<Tab>,
    pub query: Option<String>,
    pub object: Option<rows::ObjectFilter>,
    pub select: Option<String>,
}

#[derive(Default)]
struct PendingOpens(HashMap<Option<ClusterId>, Pending>);

impl Global for PendingOpens {}

fn request(cluster: Option<&ClusterId>) -> ViewRequest {
    match cluster {
        Some(cluster) => ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None),
        ),
        None => ViewRequest::new(ViewKind::Custom(ALL_KIND.into())),
    }
}

/// Opens (or focuses) the Alerts tab of `cluster` (`None`: all clusters) on `tab`.
pub fn open(cluster: &ClusterId, tab: Option<Tab>, window: &mut Window, cx: &mut App) {
    open_with(
        Some(cluster),
        Pending {
            tab,
            ..Default::default()
        },
        window,
        cx,
    );
}

/// Opens the Alerts tab with a pending tab, filter or selection (applied by a new or an open
/// view).
pub fn open_with(cluster: Option<&ClusterId>, pending: Pending, window: &mut Window, cx: &mut App) {
    cx.default_global::<PendingOpens>()
        .0
        .insert(cluster.cloned(), pending);
    window.dispatch_action(Box::new(OpenView(request(cluster))), cx);
    // An open view picks it up on its next render.
    cx.refresh_windows();
}

fn take_pending(cluster: Option<&ClusterId>, cx: &mut App) -> Option<Pending> {
    cx.default_global::<PendingOpens>()
        .0
        .remove(&cluster.cloned())
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let cluster = request.target.as_ref()?.cluster.clone();
            Some(Box::new(
                cx.new(|cx| AlertsView::new(Some(cluster), window, cx)),
            ))
        },
    );
    ViewRegistry::register(cx, ViewKind::Custom(ALL_KIND.into()), |_, window, cx| {
        Some(Box::new(cx.new(|cx| AlertsView::new(None, window, cx))))
    });
}

/// An alert row's data: which cluster (all-clusters view) and the alert.
#[derive(Clone)]
pub(crate) struct Entry {
    pub cluster: ClusterId,
    pub alert: Alert,
}

/// A silence with its cluster.
#[derive(Clone)]
pub(crate) struct SilenceEntry {
    pub cluster: ClusterId,
    pub silence: Silence,
}

pub struct AlertsView {
    /// `None`: every connected cluster.
    pub(crate) cluster: Option<ClusterId>,
    pub(crate) tab: Tab,
    pub(crate) options: AlertsState,
    pub(crate) filters: Filters,
    pub(crate) toggled: HashSet<String>,
    pub(crate) resolved_open: bool,
    pub(crate) rows: Vec<Row>,
    pub(crate) alerts: Vec<Alert>,
    pub(crate) resolved: Vec<Alert>,
    /// The cluster of each alert (index-aligned with `alerts`, then `resolved`).
    pub(crate) alert_clusters: Vec<ClusterId>,
    pub(crate) resolved_clusters: Vec<ClusterId>,
    revisions: Vec<(ClusterId, u64)>,
    /// Fingerprint of the selected alert.
    pub(crate) selected: Option<String>,
    pub(crate) details_open: bool,
    pub(crate) filter_input: Entity<InputState>,
    pub(crate) silence_filter: Entity<InputState>,
    pub(crate) rules_filter: Entity<InputState>,
    pub(crate) focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) silences_scroll: UniformListScrollHandle,
    pub(crate) rules_scroll: UniformListScrollHandle,
    pub(crate) selected_silence: Option<String>,
    pub(crate) expired_open: bool,
    pub(crate) selected_rule: Option<(String, String)>,
    pub(crate) rule_details_open: bool,
    pub(crate) details_state: details::DetailsState,
    /// A filter text to put into the input on the next render.
    pending_input: Option<String>,
    pub(crate) rule_objects: rules_tab::RuleObjects,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl AlertsView {
    pub fn new(cluster: Option<ClusterId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let options = State::get::<AlertsState>(cx);
        let filter_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(r#"alertname=~"Kube.*", namespace="payments""#)
        });
        let silence_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter matchers, comments"));
        let rules_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter rules"));
        let mut subscriptions = vec![
            cx.subscribe_in(
                &filter_input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        this.filters.query = Query::parse(&input.read(cx).value());
                        this.rebuild(cx);
                    }
                    InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &silence_filter,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &rules_filter,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                    _ => {}
                },
            ),
            cx.observe_global::<ActiveContext>(|this, cx| {
                if this.options.active_namespace_only {
                    this.apply_options(cx);
                }
                cx.notify();
            }),
        ];
        if let Some(service) = AlertsService::global(cx) {
            subscriptions.push(cx.observe(&service, |this, _, cx| this.sync(cx)));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        // "since" columns stay live.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            cluster,
            tab: Tab::Alerts,
            filters: Filters::default(),
            options,
            toggled: HashSet::new(),
            resolved_open: false,
            rows: Vec::new(),
            alerts: Vec::new(),
            resolved: Vec::new(),
            alert_clusters: Vec::new(),
            resolved_clusters: Vec::new(),
            revisions: Vec::new(),
            selected: None,
            details_open: true,
            filter_input,
            silence_filter,
            rules_filter,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            silences_scroll: UniformListScrollHandle::new(),
            rules_scroll: UniformListScrollHandle::new(),
            selected_silence: None,
            expired_open: false,
            selected_rule: None,
            rule_details_open: true,
            details_state: details::DetailsState::default(),
            pending_input: None,
            rule_objects: rules_tab::RuleObjects::default(),
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        this.apply_options(cx);
        this.sync(cx);
        this
    }

    pub fn cluster(&self) -> Option<&ClusterId> {
        self.cluster.as_ref()
    }

    /// The clusters shown: the one, or every connected one.
    pub(crate) fn clusters(&self, cx: &App) -> Vec<ClusterId> {
        match &self.cluster {
            Some(cluster) => vec![cluster.clone()],
            None => ConnectionManager::try_global(cx)
                .map(|m| {
                    let m = m.read(cx);
                    m.entries()
                        .iter()
                        .filter(|c| m.client(&c.id).is_some())
                        .map(|c| c.id.clone())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Reads the view options from state.json into the filters.
    fn apply_options(&mut self, cx: &mut Context<Self>) {
        self.filters.show_suppressed = self.options.show_suppressed;
        self.filters.severities = self.options.severities.iter().cloned().collect();
        self.filters.states = self.options.states.iter().cloned().collect();
        self.filters.namespace = if self.options.active_namespace_only && self.cluster.is_some() {
            ActiveContext::global(cx)
                .namespace
                .as_ref()
                .map(|ns| ns.to_string())
        } else {
            None
        };
        self.rebuild(cx);
    }

    /// Saves the view options (never alert data).
    pub(crate) fn save_options(&mut self, cx: &mut Context<Self>) {
        self.options.show_suppressed = self.filters.show_suppressed;
        self.options.severities = self.filters.severities.iter().cloned().collect();
        self.options.states = self.filters.states.iter().cloned().collect();
        let options = self.options.clone();
        State::set(cx, &options);
    }

    /// Takes the service's data when it changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(service) = AlertsService::global(cx) else {
            return;
        };
        let clusters = self.clusters(cx);
        let service = service.read(cx);
        let revisions: Vec<(ClusterId, u64)> = clusters
            .iter()
            .map(|c| {
                (
                    c.clone(),
                    service.cluster(c, Pace::View).map_or(0, |s| s.revision),
                )
            })
            .collect();
        if revisions == self.revisions {
            cx.notify();
            return;
        }
        let mut alerts = Vec::new();
        let mut alert_clusters = Vec::new();
        let mut resolved = Vec::new();
        let mut resolved_clusters = Vec::new();
        for cluster in &clusters {
            if let Some(state) = service.cluster(cluster, Pace::View) {
                alerts.extend(state.alerts.iter().cloned());
                alert_clusters.extend(std::iter::repeat_n(cluster.clone(), state.alerts.len()));
                resolved.extend(state.resolved.iter().cloned());
                resolved_clusters
                    .extend(std::iter::repeat_n(cluster.clone(), state.resolved.len()));
            }
        }
        if clusters.len() > 1 {
            // One order over every cluster (each list is sorted already).
            let mut paired: Vec<(Alert, ClusterId)> =
                alerts.into_iter().zip(alert_clusters).collect();
            paired.sort_by(|(a, _), (b, _)| {
                a.severity
                    .rank()
                    .cmp(&b.severity.rank())
                    .then_with(|| a.since().cmp(&b.since()))
                    .then_with(|| a.name.cmp(&b.name))
            });
            (alerts, alert_clusters) = paired.into_iter().unzip();
        }
        self.alerts = alerts;
        self.alert_clusters = alert_clusters;
        self.resolved = resolved;
        self.resolved_clusters = resolved_clusters;
        self.revisions = revisions;
        self.rebuild(cx);
    }

    /// Rebuilds the rows from the data and the filters, keeping the selection.
    pub(crate) fn rebuild(&mut self, cx: &mut Context<Self>) {
        let pending = take_pending(self.cluster.as_ref(), cx);
        if let Some(pending) = pending {
            self.apply_pending(pending);
        }
        self.rows = rows::build(
            &self.alerts,
            &self.resolved,
            &self.filters,
            self.options.group_by,
            &self.toggled,
            self.resolved_open,
        );
        if self.selected.is_none() {
            self.selected = self
                .rows
                .iter()
                .find_map(|r| rows::alert_of(r, &self.alerts, &self.resolved))
                .map(|a| a.fingerprint.clone());
        }
        cx.notify();
    }

    fn apply_pending(&mut self, pending: Pending) {
        if let Some(tab) = pending.tab {
            self.tab = tab;
        }
        if let Some(query) = pending.query {
            self.filters.query = Query::parse(&query);
            self.pending_input = Some(query);
        }
        if let Some(object) = pending.object {
            self.filters.object = Some(object);
            // Every state of the object's alerts.
            self.filters.show_suppressed = true;
        }
        if let Some(select) = pending.select {
            self.tab = Tab::Alerts;
            self.selected = Some(select);
            self.details_open = true;
        }
    }

    /// Picks up a pending open (the view was open already).
    fn check_pending(&mut self, cx: &mut Context<Self>) {
        if cx
            .try_global::<PendingOpens>()
            .is_some_and(|p| p.0.contains_key(&self.cluster))
        {
            self.rebuild(cx);
        }
    }

    // ----- Selection -----

    pub(crate) fn selected_index(&self) -> Option<usize> {
        let fingerprint = self.selected.as_ref()?;
        self.rows.iter().position(|r| {
            rows::alert_of(r, &self.alerts, &self.resolved)
                .is_some_and(|a| &a.fingerprint == fingerprint)
        })
    }

    /// The selected alert and its cluster.
    pub(crate) fn selected_entry(&self) -> Option<Entry> {
        let fingerprint = self.selected.as_ref()?;
        if let Some(i) = self
            .alerts
            .iter()
            .position(|a| &a.fingerprint == fingerprint)
        {
            return Some(Entry {
                cluster: self.alert_clusters[i].clone(),
                alert: self.alerts[i].clone(),
            });
        }
        let i = self
            .resolved
            .iter()
            .position(|a| &a.fingerprint == fingerprint)?;
        Some(Entry {
            cluster: self.resolved_clusters[i].clone(),
            alert: self.resolved[i].clone(),
        })
    }

    pub(crate) fn select_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(alert) = self
            .rows
            .get(index)
            .and_then(|r| rows::alert_of(r, &self.alerts, &self.resolved))
        {
            self.selected = Some(alert.fingerprint.clone());
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
            self.details_state = details::DetailsState::default();
        }
        cx.notify();
    }

    pub(crate) fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let alert_rows: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Alert { .. }))
            .map(|(i, _)| i)
            .collect();
        if alert_rows.is_empty() {
            return;
        }
        let current = self
            .selected_index()
            .and_then(|i| alert_rows.iter().position(|&r| r == i));
        let next = match current {
            None => 0,
            Some(pos) => (pos as isize + delta).clamp(0, alert_rows.len() as isize - 1) as usize,
        };
        self.select_row(alert_rows[next], cx);
    }

    pub(crate) fn toggle_group(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.toggled.remove(key) {
            self.toggled.insert(key.to_string());
        }
        self.rebuild(cx);
    }

    // ----- Data of the shown clusters -----

    pub(crate) fn state<'a>(&self, cluster: &ClusterId, cx: &'a App) -> Option<&'a ClusterAlerts> {
        let service = AlertsService::global(cx)?;
        service.read(cx).cluster(cluster, Pace::View)
    }

    pub(crate) fn silences(&self, cx: &App) -> Vec<SilenceEntry> {
        let mut out = Vec::new();
        for cluster in self.clusters(cx) {
            if let Some(state) = self.state(&cluster, cx) {
                out.extend(state.silences.iter().map(|s| SilenceEntry {
                    cluster: cluster.clone(),
                    silence: s.clone(),
                }));
            }
        }
        out
    }

    pub(crate) fn rules(&self, cx: &App) -> Vec<(ClusterId, Arc<Vec<crate::model::RuleGroup>>)> {
        self.clusters(cx)
            .into_iter()
            .filter_map(|c| {
                let rules = self.state(&c, cx)?.rules.clone();
                Some((c, rules))
            })
            .collect()
    }

    pub(crate) fn selected_rule(&self, cx: &App) -> Option<(ClusterId, Rule)> {
        let (group, name) = self.selected_rule.as_ref()?;
        self.rules(cx).into_iter().find_map(|(cluster, groups)| {
            groups
                .iter()
                .filter(|g| &g.name == group)
                .flat_map(|g| &g.rules)
                .find(|r| &r.name == name)
                .map(|r| (cluster.clone(), r.clone()))
        })
    }

    pub(crate) fn read_only(&self, cluster: &ClusterId, cx: &App) -> bool {
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).read_only)
    }

    pub(crate) fn production(&self, cluster: &ClusterId, cx: &App) -> bool {
        ConnectionManager::try_global(cx).is_some_and(|m| m.read(cx).caps(cluster).production)
    }

    pub(crate) fn cluster_name(cluster: &ClusterId, cx: &App) -> String {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(cluster).to_string())
            .unwrap_or_else(|| cluster.to_string())
    }

    /// The key context of the current tab's list.
    pub(crate) fn context(&self) -> &'static str {
        match self.tab {
            Tab::Alerts => ALERTS_CONTEXT,
            Tab::Silences => SILENCES_CONTEXT,
            Tab::Rules => RULES_CONTEXT,
        }
    }

    /// The part of a tab that takes the keys: the list (and details), never the inputs.
    pub(crate) fn focus_area(&self) -> gpui::Div {
        div()
            .key_context(self.context())
            .track_focus(&self.focus)
            .flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
    }

    pub(crate) fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.focus.focus(window, cx);
        cx.notify();
    }

    // ----- Rendering -----

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(cluster) = self.cluster.clone() else {
            return h_flex()
                .flex_none()
                .h(u(44.0))
                .px(u(16.0))
                .gap(u(10.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(Icon::new(IconName::Siren).size(15.0).color(colors.accent))
                .child(
                    div()
                        .text_size(u(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Alerts in all clusters"),
                )
                .into_any_element();
        };
        let state = self.state(&cluster, cx);
        let mut chips: Vec<AnyElement> = Vec::new();
        if let Some(state) = state {
            for (i, source) in state.sources.iter().enumerate() {
                let status = &source.conn.status;
                let mut text = format!("Alertmanager {}", source.label());
                if let Some(version) = &status.version {
                    text.push_str(&format!(" · v{}", version.trim_start_matches('v')));
                }
                if status.peers > 0 {
                    let ready = status.cluster_status.as_deref() == Some("ready");
                    text.push_str(&format!(
                        " · {}/{} peers",
                        if ready { status.peers } else { 0 },
                        status.peers
                    ));
                }
                let color = if source.error.is_some() {
                    colors.red
                } else {
                    colors.text_muted
                };
                let tooltip: SharedString = match (&source.error, &source.conn.via) {
                    (Some(err), _) => err.clone(),
                    (None, crate::client::Via::Proxy) => {
                        "Through the API server's service proxy".into()
                    }
                    (None, crate::client::Via::Route { url }) => {
                        format!("Through its Route {url}").into()
                    }
                    (None, crate::client::Via::Forward { local_port }) => {
                        format!("Through a temporary port-forward (127.0.0.1:{local_port})").into()
                    }
                    (None, crate::client::Via::Url) => "An external URL".into(),
                };
                chips.push(
                    h_flex()
                        .id(("am-source", i))
                        .flex_none()
                        .h(u(22.0))
                        .px(u(7.0))
                        .gap(u(5.0))
                        .rounded(u(4.0))
                        .bg(colors.chip_background)
                        .text_size(u(11.5))
                        .text_color(color)
                        .child(Icon::new(IconName::Siren).size(11.0).color(color))
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(11.0))
                                .child(text),
                        )
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                        })
                        .into_any_element(),
                );
            }
            if let Some(rules) = &state.rules_source {
                let rules: SharedString = rules.clone().into();
                chips.push(
                    h_flex()
                        .id("rules-source")
                        .flex_none()
                        .h(u(22.0))
                        .px(u(7.0))
                        .gap(u(5.0))
                        .rounded(u(4.0))
                        .bg(colors.chip_background)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(Icon::new(IconName::ListChecks).size(11.0))
                        .child("Rules: Prometheus")
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(rules.clone()).build(window, cx)
                        })
                        .into_any_element(),
                );
            }
        }
        let ui = state
            .and_then(|s| s.sources.first())
            .map(|s| s.conn.clone());
        let ui_cluster = cluster.clone();
        let refresh_cluster = cluster.clone();
        let redetect_cluster = cluster.clone();
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
                    .child(Self::cluster_name(&cluster, cx)),
            )
            .when(self.production(&cluster, cx), |this| this.child(ProdBadge))
            .children(chips)
            .child(div().flex_1())
            .when_some(ui, |this, conn| {
                this.child(
                    h_flex()
                        .id("open-am-ui")
                        .flex_none()
                        .gap(u(6.0))
                        .text_size(u(12.5))
                        .text_color(colors.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(colors.text))
                        .child(Icon::new(IconName::Globe).size(13.0))
                        .child("Open Alertmanager UI")
                        .on_click(move |_, window, cx| {
                            crate::actions::open_alertmanager_ui(&ui_cluster, &conn, window, cx)
                        }),
                )
            })
            .child(
                kubyl_ui::IconButton::new("alerts-refresh", IconName::RefreshCw)
                    .icon_size(13.0)
                    .on_click(move |_, _, cx| {
                        if let Some(service) = AlertsService::global(cx) {
                            let cluster = refresh_cluster.clone();
                            service.update(cx, |s, cx| s.refresh_now(&cluster, cx));
                        }
                    }),
            )
            .child(
                kubyl_ui::IconButton::new("alerts-redetect", IconName::Search)
                    .icon_size(13.0)
                    .on_click(move |_, _, cx| {
                        if let Some(service) = AlertsService::global(cx) {
                            let cluster = redetect_cluster.clone();
                            service.update(cx, |s, cx| s.redetect(&cluster, cx));
                        }
                    }),
            )
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let mut firing = 0;
        let mut silences = 0;
        let mut rules = 0;
        let mut has_am = false;
        let mut has_rules = false;
        for cluster in self.clusters(cx) {
            if let Some(state) = self.state(&cluster, cx) {
                firing += state.counts().firing();
                silences += state
                    .silences
                    .iter()
                    .filter(|s| s.state == "active" || s.state == "pending")
                    .count();
                rules += state.rule_count();
                has_am |= state.has_alertmanager();
                has_rules |= state.has_rules();
            }
        }
        let tab = |id: &'static str,
                   tab: Tab,
                   icon: IconName,
                   label: &'static str,
                   count: Option<usize>,
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
                .child(Icon::new(icon).size(13.0))
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
                            .child(count.to_string()),
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
                "alerts-tab-alerts",
                Tab::Alerts,
                IconName::Siren,
                "Alerts",
                Some(firing),
                cx,
            ))
            .child(tab(
                "alerts-tab-silences",
                Tab::Silences,
                IconName::BellOff,
                "Silences",
                has_am.then_some(silences),
                cx,
            ))
            .child(tab(
                "alerts-tab-rules",
                Tab::Rules,
                IconName::ListChecks,
                "Rules",
                has_rules.then_some(rules),
                cx,
            ))
            .into_any_element()
    }

    /// What explains an empty tab for a single cluster, if anything.
    pub(crate) fn blocking_state(&self, cx: &App) -> Option<states::Blocking> {
        let cluster = self.cluster.as_ref()?;
        let connected = ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).client(cluster))
            .is_some();
        if !connected {
            return Some(states::Blocking::NotConnected);
        }
        let Some(state) = self.state(cluster, cx) else {
            return Some(states::Blocking::Loading);
        };
        match state.phase {
            Phase::Disabled => Some(states::Blocking::Disabled),
            Phase::Unknown | Phase::Discovering => Some(states::Blocking::Loading),
            Phase::NoSource => Some(states::Blocking::NoSource),
            Phase::Ready if state.checked_at.is_none() && state.error.is_none() => {
                Some(states::Blocking::Loading)
            }
            Phase::Ready if state.checked_at.is_none() => Some(states::Blocking::Failed),
            Phase::Ready => None,
        }
    }

    pub(crate) fn hints(&self, context: &str, cx: &App) -> Vec<(SharedString, SharedString)> {
        let read_only = self
            .selected_entry()
            .map(|e| self.read_only(&e.cluster, cx))
            .or_else(|| self.cluster.as_ref().map(|c| self.read_only(c, cx)))
            .unwrap_or(false);
        kubyl_core::ActionRegistry::global(cx)
            .hints(context)
            .into_iter()
            .filter(|(_, hint)| !read_only || !crate::actions::WRITE_HINTS.contains(&hint.as_ref()))
            .collect()
    }
}

impl Focusable for AlertsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for AlertsView {
    fn tab_title(&self, cx: &App) -> SharedString {
        match &self.cluster {
            None => "Alerts · all clusters".into(),
            Some(cluster) => {
                let active =
                    ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(cluster);
                if active {
                    "Alerts".into()
                } else {
                    format!("Alerts · {}", Self::cluster_name(cluster, cx)).into()
                }
            }
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Siren.path())
    }

    fn tab_dot(&self, cx: &App) -> Option<gpui::Hsla> {
        let cluster = self.cluster.as_ref()?;
        let active = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .map(|c| c.id.clone());
        (active.as_ref() != Some(cluster))
            .then(|| ConnectionManager::try_global(cx).map(|m| m.read(cx).color(cluster, cx)))
            .flatten()
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(request(self.cluster.as_ref()))
    }
}

impl Render for AlertsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.check_pending(cx);
        if let Some(query) = self.pending_input.take() {
            self.filter_input
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
        let colors: Colors = cx.colors().clone();
        let header = self.render_header(cx);
        let tabs = self.render_tabs(cx);
        let context = self.context();
        let body = match self.tab {
            Tab::Alerts => self.render_alerts_tab(window, cx),
            Tab::Silences => self.render_silences_tab(window, cx),
            Tab::Rules => self.render_rules_tab(window, cx),
        };
        let hints = {
            let mut hints = self.hints(context, cx);
            hints.push(("/".into(), "Filter".into()));
            hints
        };
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
            .child(div().flex_1().min_h_0().flex().child(body))
            .child(kubyl_ui::KeyHints::new(hints))
    }
}
