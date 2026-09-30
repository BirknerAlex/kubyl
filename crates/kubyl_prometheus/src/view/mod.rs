//! The Prometheus view: one tab per cluster with Overview, Query, Targets, Rules and Service
//! Discovery, like the Prometheus web UI, and a selector when the cluster has several servers.
//!
//! The view reads the API itself, only for the tab that is on screen (see [`PrometheusView::tick`]).

mod discovery;
mod overview;
mod query;
mod rules;
mod targets;
pub(crate) mod widgets;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, FocusHandle, Focusable, FontWeight,
    Global, IntoElement, Render, SharedString, Subscription, Task, Window, div, prelude::*,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind, ViewRegistry, ViewRequest,
    spawn_kube,
};
use kubyl_kube::ConnectionManager;
use kubyl_settings::Settings;
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, ProdBadge, fonts, h_flex, sizes, u, v_flex};

use crate::actions::{
    AcceptSuggestion, DismissSuggestions, NextSuggestion, PreviousSuggestion, RunQuery,
    TriggerSuggestions,
};
use crate::complete::Names;
use crate::fetch;
use crate::model::{Overview, RuleGroup, Targets};
use crate::service::{Instance, Kind, Phase, PrometheusService};
use crate::settings::PrometheusSettings;

/// `ViewKind::Custom` of a cluster's Prometheus tab.
pub const VIEW_KIND: &str = "prometheus";
/// The key context of the view.
pub const VIEW_CONTEXT: &str = "PrometheusView";

/// The tabs of the view, in the order of the Prometheus web UI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Overview,
    Query,
    Targets,
    Rules,
    Discovery,
}

#[derive(Default)]
struct PendingOpens(HashMap<ClusterId, Tab>);

impl Global for PendingOpens {}

fn request(cluster: &ClusterId) -> ViewRequest {
    ViewRequest::for_resource(
        ViewKind::Custom(VIEW_KIND.into()),
        ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), None),
    )
}

/// Opens (or focuses) the Prometheus tab of `cluster`, on `tab` when given.
pub fn open(cluster: &ClusterId, tab: Option<Tab>, window: &mut Window, cx: &mut App) {
    if let Some(tab) = tab {
        cx.default_global::<PendingOpens>()
            .0
            .insert(cluster.clone(), tab);
    }
    window.dispatch_action(Box::new(OpenView(request(cluster))), cx);
    // An open view picks it up on its next render.
    cx.refresh_windows();
}

pub(crate) fn init(cx: &mut App) {
    ViewRegistry::register(
        cx,
        ViewKind::Custom(VIEW_KIND.into()),
        |request, window, cx| {
            let cluster = request.target.as_ref()?.cluster.clone();
            Some(Box::new(
                cx.new(|cx| PrometheusView::new(cluster, window, cx)),
            ))
        },
    );
}

/// What one read of the API holds. The data stays while a newer read fails: the error shows
/// next to it.
pub(crate) struct Section<T> {
    pub data: Option<Arc<T>>,
    pub error: Option<SharedString>,
    pub loading: bool,
    fetched: Option<Instant>,
}

impl<T> Default for Section<T> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
            loading: false,
            fetched: None,
        }
    }
}

impl<T> Section<T> {
    fn due(&self, every: Duration) -> bool {
        !self.loading && self.fetched.is_none_or(|t| t.elapsed() >= every)
    }

    fn finish(&mut self, result: Result<T, String>) {
        self.loading = false;
        self.fetched = Some(Instant::now());
        match result {
            Ok(data) => {
                self.data = Some(Arc::new(data));
                self.error = None;
            }
            Err(err) => self.error = Some(err.into()),
        }
    }

    /// Reads it again on the next tick.
    fn stale(&mut self) {
        self.fetched = None;
    }
}

/// What explains an empty view.
enum Blocking {
    NotConnected,
    Loading,
    NoSource(SharedString),
    Disabled,
}

pub struct PrometheusView {
    pub(crate) cluster: ClusterId,
    pub(crate) tab: Tab,
    /// The server picked in the header (`None`: the first one).
    instance: Option<String>,
    /// Bumped when the server changes: reads of the old one are dropped.
    generation: u64,
    pub(crate) overview: Section<Overview>,
    pub(crate) targets: Section<Targets>,
    pub(crate) rules: Section<Vec<RuleGroup>>,
    /// Metric and label names for the query box.
    pub(crate) names: Section<Names>,
    pub(crate) targets_tab: targets::State,
    pub(crate) rules_tab: rules::State,
    pub(crate) discovery_tab: discovery::State,
    pub(crate) query_tab: query::State,
    pub(crate) focus: FocusHandle,
    /// The id of the server the sections belong to.
    seen_instance: Option<String>,
    /// The tick reads only while the view is on screen.
    rendered_at: Instant,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl PrometheusView {
    fn new(cluster: ClusterId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(service) = PrometheusService::global(cx) {
            subscriptions.push(cx.observe(&service, |_, _, cx| cx.notify()));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_secs(1)).await;
            }
        });
        let targets_tab = targets::State::new(window, cx);
        let rules_tab = rules::State::new(window, cx);
        let discovery_tab = discovery::State::new(window, cx);
        let query_tab = query::State::new(window, cx);
        subscriptions.extend(targets_tab.subscriptions(window, cx));
        subscriptions.extend(rules_tab.subscriptions(window, cx));
        subscriptions.extend(discovery_tab.subscriptions(window, cx));
        subscriptions.extend(query_tab.subscriptions(window, cx));
        Self {
            cluster,
            tab: Tab::Overview,
            instance: None,
            seen_instance: None,
            generation: 0,
            overview: Section::default(),
            targets: Section::default(),
            rules: Section::default(),
            names: Section::default(),
            targets_tab,
            rules_tab,
            discovery_tab,
            query_tab,
            focus: cx.focus_handle(),
            rendered_at: Instant::now(),
            _ticker: ticker,
            _subscriptions: subscriptions,
        }
    }

    // ----- Servers -----

    fn instances(&self, cx: &App) -> Vec<Instance> {
        PrometheusService::global(cx)
            .map(|s| s.read(cx).instances(&self.cluster).to_vec())
            .unwrap_or_default()
    }

    /// Starts over when the server the tabs show is another one than last time: the picked one
    /// went away, or the list changed its order.
    fn sync_instance(&mut self, cx: &mut Context<Self>) {
        let id = self.current(cx).map(|i| i.id);
        if id == self.seen_instance {
            return;
        }
        let first = self.seen_instance.is_none();
        self.seen_instance = id;
        if first {
            return;
        }
        self.generation += 1;
        self.overview = Section::default();
        self.targets = Section::default();
        self.rules = Section::default();
        self.names = Section::default();
        self.query_tab.reset(cx);
        cx.notify();
    }

    /// The server the tabs show: the picked one, else the first.
    pub(crate) fn current(&self, cx: &App) -> Option<Instance> {
        let list = self.instances(cx);
        let picked = self
            .instance
            .as_ref()
            .and_then(|id| list.iter().find(|i| &i.id == id));
        picked.or(list.first()).cloned()
    }

    fn select_instance(&mut self, id: String, cx: &mut Context<Self>) {
        if self.current(cx).is_some_and(|i| i.id == id) {
            return;
        }
        self.instance = Some(id);
        self.sync_instance(cx);
        cx.notify();
        self.tick(cx);
    }

    // ----- Reading -----

    /// Reads what the tab on screen shows when it is older than the refresh interval.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.sync_instance(cx);
        if self.rendered_at.elapsed() > Duration::from_secs(3) {
            return;
        }
        let Some(instance) = self.current(cx) else {
            return;
        };
        let every = Settings::get::<PrometheusSettings>(cx).refresh();
        let (overview, targets, rules) = match self.tab {
            Tab::Overview => (true, true, true),
            Tab::Query => (false, false, false),
            Tab::Targets | Tab::Discovery => (false, true, false),
            Tab::Rules => (false, false, true),
        };
        if overview && self.overview.due(every * 2) {
            self.overview.loading = true;
            let prom = instance.client.clone();
            self.spawn_read(
                cx,
                async move { fetch::overview(&prom).await },
                |this, overview, _| this.overview.finish(Ok(overview)),
            );
        }
        // The names change slowly and are big on large servers.
        // (A failed read is tried again soon.)
        let names_every = if self.names.error.is_some() { 15 } else { 600 };
        if self.tab == Tab::Query && self.names.due(Duration::from_secs(names_every)) {
            self.names.loading = true;
            let prom = instance.client.clone();
            self.spawn_read(
                cx,
                async move { fetch::names(&prom).await.map_err(|e| e.to_string()) },
                |this, result, cx| {
                    this.names.finish(result);
                    this.update_suggestions(cx);
                },
            );
        }
        if targets && self.targets.due(every) {
            self.targets.loading = true;
            let prom = instance.client.clone();
            self.spawn_read(
                cx,
                async move { fetch::targets(&prom).await.map_err(|e| e.to_string()) },
                |this, result, _| this.targets.finish(result),
            );
        }
        if rules && self.rules.due(every) {
            self.rules.loading = true;
            let prom = instance.client.clone();
            self.spawn_read(
                cx,
                async move { fetch::rules(&prom).await.map_err(|e| e.to_string()) },
                |this, result, _| this.rules.finish(result),
            );
        }
    }

    /// Runs `future` on the Tokio runtime and applies its result unless the server changed.
    fn spawn_read<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        future: impl std::future::Future<Output = T> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) {
        let generation = self.generation;
        let task = spawn_kube(cx, future);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    apply(this, result, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// "Refresh": everything is read again now.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.overview.stale();
        self.targets.stale();
        self.rules.stale();
        self.names.stale();
        self.tick(cx);
    }

    pub(crate) fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        self.focus.focus(window, cx);
        cx.notify();
        self.tick(cx);
    }

    fn take_pending(&mut self, cx: &mut Context<Self>) {
        let pending = cx
            .try_global::<PendingOpens>()
            .is_some_and(|p| p.0.contains_key(&self.cluster));
        if pending && let Some(tab) = cx.default_global::<PendingOpens>().0.remove(&self.cluster) {
            self.tab = tab;
            self.tick(cx);
        }
    }

    pub(crate) fn cluster_name(&self, cx: &App) -> String {
        ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).display_name(&self.cluster).to_string())
            .unwrap_or_else(|| self.cluster.to_string())
    }

    fn blocking(&self, cx: &App) -> Option<Blocking> {
        let connected = ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).client(&self.cluster))
            .is_some();
        if !connected {
            return Some(Blocking::NotConnected);
        }
        match PrometheusService::global(cx)?.read(cx).phase(&self.cluster) {
            Phase::Unknown | Phase::Discovering => Some(Blocking::Loading),
            Phase::NoSource(why) => Some(Blocking::NoSource(why)),
            Phase::Disabled => Some(Blocking::Disabled),
            Phase::Ready => None,
        }
    }

    // ----- Rendering -----

    fn render_header(&self, instance: Option<&Instance>, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let instances = self.instances(cx);
        let version = self
            .overview
            .data
            .as_ref()
            .and_then(|o| o.build.as_ref()?.as_ref().ok()?.version.clone());
        let production = ConnectionManager::try_global(cx)
            .is_some_and(|m| m.read(cx).caps(&self.cluster).production);
        let selector =
            instance.map(|current| self.render_selector(current, &instances, &colors, cx));
        let ui = instance
            .filter(|i| matches!(i.kind, Kind::Prometheus | Kind::Thanos))
            .and_then(|i| {
                let (namespace, service, port, path) = i.service()?;
                Some((
                    namespace.to_string(),
                    service.to_string(),
                    port.parse::<u16>().ok()?,
                    path.to_string(),
                ))
            });
        let cluster = self.cluster.clone();
        let refresh_cluster = self.cluster.clone();
        h_flex()
            .flex_none()
            .h(u(44.0))
            .px(u(16.0))
            .gap(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .overflow_hidden()
            .child(Icon::new(IconName::Flame).size(15.0).color(colors.accent))
            .child(
                div()
                    .flex_none()
                    .text_size(u(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.cluster_name(cx)),
            )
            .when(production, |this| this.child(ProdBadge))
            .children(selector)
            .when_some(version, |this, version| {
                this.child(
                    div()
                        .flex_none()
                        .px(u(7.0))
                        .h(u(22.0))
                        .flex()
                        .items_center()
                        .rounded(u(4.0))
                        .bg(colors.chip_background)
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_muted)
                        .child(format!("v{}", version.trim_start_matches('v'))),
                )
            })
            .child(div().flex_1())
            .when_some(ui, |this, (namespace, service, port, path)| {
                this.child(
                    h_flex()
                        .id("open-prometheus-ui")
                        .flex_none()
                        .gap(u(6.0))
                        .text_size(u(12.5))
                        .text_color(colors.text_muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(colors.text))
                        .child(Icon::new(IconName::Globe).size(13.0))
                        .child("Open web UI")
                        .on_click(move |_, window, cx| {
                            let target = ResourceRef::object(
                                cluster.clone(),
                                Gvr::new("", "v1", "services"),
                                Some(namespace.clone()),
                                service.clone(),
                            );
                            let path = (!path.is_empty()).then(|| format!("{path}/"));
                            window.dispatch_action(
                                Box::new(kubyl_webview::OpenWebView {
                                    target,
                                    port,
                                    path,
                                    ask: false,
                                }),
                                cx,
                            );
                        }),
                )
            })
            .child(
                kubyl_ui::IconButton::new("prometheus-refresh", IconName::RefreshCw)
                    .icon_size(13.0)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.refresh(cx))),
            )
            .child(
                kubyl_ui::IconButton::new("prometheus-redetect", IconName::Search)
                    .icon_size(13.0)
                    .on_click(move |_, _, cx| {
                        if let Some(service) = PrometheusService::global(cx) {
                            let cluster = refresh_cluster.clone();
                            service.update(cx, |s, cx| s.redetect(&cluster, cx));
                        }
                    }),
            )
            .into_any_element()
    }

    /// The server's name, a menu when the cluster has more than one.
    fn render_selector(
        &self,
        current: &Instance,
        instances: &[Instance],
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = format!("{} · {}", current.kind.label(), current.label());
        if instances.len() < 2 {
            return h_flex()
                .flex_shrink(1.0)
                .min_w(u(120.0))
                .h(u(22.0))
                .px(u(7.0))
                .gap(u(5.0))
                .rounded(u(4.0))
                .bg(colors.chip_background)
                .text_color(colors.text_muted)
                .child(Icon::new(IconName::Server).size(11.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .child(text),
                )
                .into_any_element();
        }
        let weak = cx.weak_entity();
        let list: Vec<(String, String)> = instances
            .iter()
            .map(|i| (i.id.clone(), format!("{} · {}", i.kind.label(), i.label())))
            .collect();
        let current_id = current.id.clone();
        MenuButton::new("prometheus-instance")
            .outline()
            .compact()
            .child(
                h_flex()
                    .gap(u(6.0))
                    .text_size(u(12.0))
                    .child(Icon::new(IconName::Server).size(12.0))
                    .child(div().max_w(u(320.0)).truncate().child(text))
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for (id, label) in &list {
                    let weak = weak.clone();
                    let pick = id.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(*id == current_id)
                            .on_click(move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                let pick = pick.clone();
                                weak.update(cx, |this, cx| this.select_instance(pick, cx))
                                    .ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let down = self
            .targets
            .data
            .as_ref()
            .map(|t| t.count(crate::model::Health::Down));
        let targets = self.targets.data.as_ref().map(|t| t.active.len());
        let rules = self
            .rules
            .data
            .as_ref()
            .map(|g| g.iter().map(|g| g.rules.len()).sum::<usize>());
        let pools = self.targets.data.as_ref().map(|t| t.pools().len());
        let tab = |id: &'static str,
                   tab: Tab,
                   icon: IconName,
                   label: &'static str,
                   count: Option<(usize, bool)>,
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
                .when_some(count, |this, (count, bad)| {
                    this.child(
                        div()
                            .px(u(6.0))
                            .rounded(u(4.0))
                            .bg(colors.chip_background)
                            .font_family(fonts::MONO)
                            .text_size(u(11.0))
                            .text_color(if bad { colors.red } else { colors.text_muted })
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
                "prometheus-tab-overview",
                Tab::Overview,
                IconName::Gauge,
                "Overview",
                None,
                cx,
            ))
            .child(tab(
                "prometheus-tab-query",
                Tab::Query,
                IconName::Terminal,
                "Query",
                None,
                cx,
            ))
            .child(tab(
                "prometheus-tab-targets",
                Tab::Targets,
                IconName::Activity,
                "Target Health",
                targets.map(|n| (n, down.is_some_and(|d| d > 0))),
                cx,
            ))
            .child(tab(
                "prometheus-tab-rules",
                Tab::Rules,
                IconName::ListChecks,
                "Rule Health",
                rules.map(|n| (n, false)),
                cx,
            ))
            .child(tab(
                "prometheus-tab-discovery",
                Tab::Discovery,
                IconName::Waypoints,
                "Service Discovery",
                pools.map(|n| (n, false)),
                cx,
            ))
            .into_any_element()
    }

    fn render_blocking(&self, blocking: &Blocking, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let cluster = self.cluster.clone();
        let (title, text, look_again): (&str, SharedString, bool) = match blocking {
            Blocking::NotConnected => (
                "Not connected",
                "Connect to the cluster to look for a Prometheus.".into(),
                false,
            ),
            Blocking::Loading => (
                "Looking for Prometheus…",
                "Checking the cluster's Services for Prometheus, Thanos Query and VictoriaMetrics."
                    .into(),
                false,
            ),
            Blocking::NoSource(why) => (
                "No Prometheus found",
                format!(
                    "{why} Name one in settings.json under metrics.prometheus, or check that \
                     you may get services/proxy."
                )
                .into(),
                true,
            ),
            Blocking::Disabled => (
                "Prometheus is turned off for this cluster",
                "metrics.prometheus.<cluster>.disabled is set in settings.json.".into(),
                false,
            ),
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(u(8.0))
            .p(u(24.0))
            .child(
                div()
                    .text_size(u(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .max_w(u(520.0))
                    .text_size(u(12.5))
                    .text_color(colors.text_dim)
                    .child(text),
            )
            .when(look_again, |this| {
                this.child(widgets::button(
                    "prometheus-look-again",
                    Some(IconName::Search),
                    "Look again",
                    false,
                    &colors,
                    move |_, _, cx| {
                        if let Some(service) = PrometheusService::global(cx) {
                            let cluster = cluster.clone();
                            service.update(cx, |s, cx| s.redetect(&cluster, cx));
                        }
                    },
                ))
            })
            .into_any_element()
    }

    /// A read that failed, above the tab's content.
    pub(crate) fn error_banner(error: &SharedString, colors: &Colors) -> AnyElement {
        div()
            .flex_none()
            .px(u(14.0))
            .py(u(6.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.red.opacity(0.08))
            .text_size(u(12.0))
            .text_color(colors.red)
            .child(error.clone())
            .into_any_element()
    }
}

impl Focusable for PrometheusView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for PrometheusView {
    fn tab_title(&self, cx: &App) -> SharedString {
        let active =
            ActiveContext::global(cx).cluster.as_ref().map(|c| &c.id) == Some(&self.cluster);
        if active {
            "Prometheus".into()
        } else {
            format!("Prometheus · {}", self.cluster_name(cx)).into()
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Flame.path())
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
        Some(request(&self.cluster))
    }
}

impl Render for PrometheusView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rendered_at = Instant::now();
        self.take_pending(cx);
        let colors: Colors = cx.colors().clone();
        let instance = self.current(cx);
        let blocking = self.blocking(cx);
        let header = self.render_header(instance.as_ref(), cx);
        let content = match (&blocking, &instance) {
            (Some(blocking), _) => self.render_blocking(blocking, cx),
            (None, None) => self.render_blocking(&Blocking::Loading, cx),
            (None, Some(instance)) => {
                let tabs = self.render_tabs(cx);
                let body = match self.tab {
                    Tab::Overview => self.render_overview(instance, cx),
                    Tab::Query => self.render_query(instance, window, cx),
                    Tab::Targets => self.render_targets(window, cx),
                    Tab::Rules => self.render_rules(window, cx),
                    Tab::Discovery => self.render_discovery(window, cx),
                };
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(tabs)
                    .child(div().flex_1().min_h_0().flex().child(body))
                    .into_any_element()
            }
        };
        v_flex()
            .key_context(VIEW_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            .on_action(cx.listener(|this, _: &RunQuery, window, cx| {
                this.set_tab(Tab::Query, window, cx);
                this.run_query(window, cx);
            }))
            // Without completions these keys belong to whatever else binds them.
            .on_action(cx.listener(|this, _: &AcceptSuggestion, window, cx| {
                if !this.accept_suggestion(window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &NextSuggestion, _, cx| {
                if this.query_tab.suggestions.is_none() {
                    cx.propagate();
                } else {
                    this.move_suggestion(1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousSuggestion, _, cx| {
                if this.query_tab.suggestions.is_none() {
                    cx.propagate();
                } else {
                    this.move_suggestion(-1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TriggerSuggestions, _, cx| this.suggest(true, cx)))
            .on_action(cx.listener(|this, _: &DismissSuggestions, _, cx| {
                if this.query_tab.suggestions.take().is_some() {
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .child(header)
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        BuildInfo, Dropped, Health, Labels, QueryResult, RangeSeries, Rule, RuleAlert, RuleGroup,
        RuleKind, RuntimeInfo, Stat, Target, Tsdb,
    };
    use gpui::{Entity, TestAppContext};
    use jiff::Timestamp;
    use kubyl_metrics::prometheus::PromClient;

    fn labels(pairs: &[(&str, &str)]) -> Labels {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn targets() -> Targets {
        let target = |pool: &str, url: &str, health, error: Option<&str>| Target {
            pool: pool.into(),
            url: url.into(),
            health,
            labels: labels(&[("job", pool), ("instance", url)]),
            discovered: labels(&[("__address__", url)]),
            last_error: error.map(str::to_string),
            last_scrape: Some(Timestamp::now()),
            scrape_duration: Some(0.012),
        };
        Targets {
            active: vec![
                target("node", "10.0.0.1:9100", Health::Up, None),
                target(
                    "node",
                    "10.0.0.2:9100",
                    Health::Down,
                    Some("connection refused"),
                ),
                target("kubelet", "10.0.0.1:10250", Health::Up, None),
            ],
            dropped: vec![Dropped {
                pool: "node".into(),
                discovered: labels(&[("__address__", "10.0.0.9:9100")]),
            }],
            dropped_counts: [("node".to_string(), 1)].into(),
        }
    }

    fn groups() -> Vec<RuleGroup> {
        vec![RuleGroup {
            name: "node".into(),
            file: "/etc/rules/node.yaml".into(),
            interval: 30.0,
            evaluation_time: Some(0.004),
            last_evaluation: Some(Timestamp::now()),
            rules: vec![Rule {
                name: "NodeDown".into(),
                kind: RuleKind::Alerting,
                query: "up{job=\"node\"} == 0".into(),
                duration: 300.0,
                labels: labels(&[("severity", "critical")]),
                annotations: labels(&[("summary", "A node is down")]),
                health: Health::Up,
                last_error: None,
                evaluation_time: Some(0.001),
                last_evaluation: Some(Timestamp::now()),
                state: Some("firing".into()),
                alerts: vec![RuleAlert {
                    labels: labels(&[("instance", "10.0.0.2:9100")]),
                    state: "firing".into(),
                    active_at: Some(Timestamp::now()),
                    value: Some("0".into()),
                }],
            }],
        }]
    }

    fn overview() -> Overview {
        Overview {
            build: Some(Ok(BuildInfo {
                version: Some("3.5.0".into()),
                revision: Some("0123456789abcdef".into()),
                ..Default::default()
            })),
            runtime: Some(Ok(RuntimeInfo {
                start_time: Some(Timestamp::now()),
                reload_config_success: Some(true),
                storage_retention: Some("15d".into()),
                ..Default::default()
            })),
            tsdb: Some(Ok(Tsdb {
                series: Some(1200),
                series_by_metric: vec![Stat {
                    name: "up".into(),
                    value: 40,
                }],
                memory_by_label: vec![Stat {
                    name: "job".into(),
                    value: 4096,
                }],
                ..Default::default()
            })),
        }
    }

    /// Paints one tab body of a view.
    struct Probe(Entity<PrometheusView>, Tab, Instance);

    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let (tab, instance) = (self.1, self.2.clone());
            self.0.update(cx, |view, cx| match tab {
                Tab::Overview => view.render_overview(&instance, cx),
                Tab::Query => view.render_query(&instance, window, cx),
                Tab::Targets => view.render_targets(window, cx),
                Tab::Rules => view.render_rules(window, cx),
                Tab::Discovery => view.render_discovery(window, cx),
            })
        }
    }

    #[gpui::test]
    fn every_tab_paints(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            Settings::register::<PrometheusSettings>(cx);
        });
        // The client's buffer spawns its worker: it needs a runtime, but sends nothing.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let instance =
            Instance::new(PromClient::external("http://127.0.0.1:1", false, None).unwrap());
        for tab in [
            Tab::Overview,
            Tab::Query,
            Tab::Targets,
            Tab::Rules,
            Tab::Discovery,
        ] {
            let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<PrometheusView>>>> =
                Default::default();
            let (_probe, cx) = cx.add_window_view({
                let slot = slot.clone();
                let instance = instance.clone();
                move |window, cx| {
                    let view = cx.new(|cx| PrometheusView::new(ClusterId::new("c"), window, cx));
                    *slot.borrow_mut() = Some(view.clone());
                    gpui_component::Root::new(cx.new(|_| Probe(view, tab, instance)), window, cx)
                }
            });
            let view = slot.borrow().clone().unwrap();
            view.update(cx, |view, cx| {
                view.overview.finish(Ok(overview()));
                view.targets.finish(Ok(targets()));
                view.rules.finish(Ok(groups()));
                view.targets_tab.selected = Some(("node".into(), "10.0.0.2:9100".into()));
                let groups = groups();
                view.rules_tab.selected = Some(crate::view::rules::rule_key(
                    &groups[0],
                    &groups[0].rules[0],
                ));
                view.discovery_tab.selected = Some("node".into());
                view.query_tab.outcome = Some(query::Outcome::for_test(QueryResult::Matrix(vec![
                    RangeSeries {
                        labels: labels(&[("__name__", "up")]),
                        points: vec![(1.0, 1.0)],
                    },
                ])));
                cx.notify();
            });
            cx.run_until_parked();
        }
    }

    #[gpui::test]
    fn the_query_box_takes_paste(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            crate::actions::init(cx);
            Settings::register::<PrometheusSettings>(cx);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let instance =
            Instance::new(PromClient::external("http://127.0.0.1:1", false, None).unwrap());
        let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<PrometheusView>>>> =
            Default::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| PrometheusView::new(ClusterId::new("c"), window, cx));
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(cx.new(|_| Probe(view, Tab::Query, instance)), window, cx)
            }
        });
        let view = slot.borrow().clone().unwrap();
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let input = view.query_tab.input.clone();
            input.read(cx).focus_handle(cx).focus(window, cx);
        });
        cx.run_until_parked();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("rate(up[5m])".into()));
        // The input binds the platform key: cmd on macOS, ctrl elsewhere.
        let modifier = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.simulate_keystrokes(&format!("{modifier}-v"));
        cx.run_until_parked();
        let text = view.update(cx, |view, cx| {
            view.query_tab.input.read(cx).value().to_string()
        });
        assert_eq!(text, "rate(up[5m])");
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("old".into()));
        cx.simulate_keystrokes(&format!("{modifier}-a {modifier}-c"));
        cx.run_until_parked();
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("rate(up[5m])")
        );
    }

    #[gpui::test]
    fn completion_works_at_the_cursor(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            crate::actions::init(cx);
            Settings::register::<PrometheusSettings>(cx);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let instance =
            Instance::new(PromClient::external("http://127.0.0.1:1", false, None).unwrap());
        let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<PrometheusView>>>> =
            Default::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| PrometheusView::new(ClusterId::new("c"), window, cx));
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(cx.new(|_| Probe(view, Tab::Query, instance)), window, cx)
            }
        });
        let view = slot.borrow().clone().unwrap();
        cx.run_until_parked();
        // "sum(ra) + 1" with the cursor after "ra": complete `rate` there.
        view.update_in(cx, |view, window, cx| {
            let input = view.query_tab.input.clone();
            input.update(cx, |input, cx| {
                input.set_value("sum(ra) + 1", window, cx);
                input.set_cursor_position(gpui_component::input::Position::new(0, 6), window, cx);
            });
            view.update_suggestions(cx);
            let suggestions = view.query_tab.suggestions.as_ref().expect("suggestions");
            let at = suggestions
                .items
                .iter()
                .position(|i| i.label == "rate")
                .unwrap();
            view.move_suggestion(at as isize, cx);
            assert!(view.accept_suggestion(window, cx));
        });
        let (text, cursor) = view.update(cx, |view, cx| {
            let input = view.query_tab.input.read(cx);
            (input.value().to_string(), input.cursor())
        });
        assert_eq!(text, "sum(rate() + 1");
        assert_eq!(cursor, 9, "after the `(`");
        // A snippet puts the cursor inside its brackets.
        view.update_in(cx, |view, window, cx| {
            let input = view.query_tab.input.clone();
            input.update(cx, |input, cx| input.set_value("rat", window, cx));
            view.update_suggestions(cx);
            let suggestions = view.query_tab.suggestions.as_ref().unwrap();
            let at = suggestions
                .items
                .iter()
                .position(|i| i.label == "rate(…[5m])")
                .unwrap();
            view.move_suggestion(at as isize, cx);
            assert!(view.accept_suggestion(window, cx));
        });
        let (text, cursor) = view.update(cx, |view, cx| {
            let input = view.query_tab.input.read(cx);
            (input.value().to_string(), input.cursor())
        });
        assert_eq!(text, "rate([5m])");
        assert_eq!(cursor, 5);
    }

    #[gpui::test]
    fn enter_runs_the_query_unless_it_completes_a_word(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            crate::actions::init(cx);
            Settings::register::<PrometheusSettings>(cx);
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let instance =
            Instance::new(PromClient::external("http://127.0.0.1:1", false, None).unwrap());
        let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<PrometheusView>>>> =
            Default::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| PrometheusView::new(ClusterId::new("c"), window, cx));
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(cx.new(|_| Probe(view, Tab::Query, instance)), window, cx)
            }
        });
        let view = slot.borrow().clone().unwrap();
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            let input = view.query_tab.input.clone();
            // After an operand the list offers operators: Enter still runs the query.
            input.update(cx, |input, cx| input.set_value("sum(up) ", window, cx));
            view.update_suggestions(cx);
            assert!(view.query_tab.suggestions.is_some());
            assert!(!view.enter_accepts(cx));
            // Unless the user picked one.
            view.move_suggestion(1, cx);
            assert!(view.enter_accepts(cx));
            // A word being typed is completed by Enter.
            input.update(cx, |input, cx| input.set_value("rat", window, cx));
            view.update_suggestions(cx);
            assert!(view.enter_accepts(cx));
        });
    }
}
