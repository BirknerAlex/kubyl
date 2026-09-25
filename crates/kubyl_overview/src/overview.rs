//! `ViewKind::Overview` (board 4): the cluster dashboard, or a namespace's when the target has
//! a namespace (a favorite's "Open Namespace Overview").
//!
//! Counts, requests, limits and capacity come from shared watches (nodes, pods, workloads), so
//! they're live and work without any metrics source. Usage comes from `kubyl_metrics`: current
//! values from Prometheus or metrics-server, charts and sparklines from Prometheus range queries
//! (without Prometheus, KPI sparklines use the metrics-server samples collected while the
//! overview is open, and the charts make room for a "connect Prometheus" hint).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable,
    FontWeight, Hsla, IntoElement, Render, SharedString, Subscription, Task, Window, div,
    prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_charts::data::top_n;
use kubyl_charts::{
    ChartData, ChartKind, LineChart, Meter, SERIES_COLORS, Series, Sparkline, TimeRange,
    TimeRangePicker, other_color, series_color,
};
use kubyl_core::actions::{OpenSettings, OpenView};
use kubyl_core::{
    ActiveContext, ClusterId, Gvr, ResourceRef, TabView, Tone, ViewKind, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_kube::cluster_info::Distribution;
use kubyl_metrics::{MetricsService, RangeKey, RangeState, Source};
use kubyl_resources::columns::{node_status, pod_status};
use kubyl_resources::format::{format_bytes, format_cpu, int_at, parse_quantity, str_at};
use kubyl_resources::metrics::{Usage, pod_resource};
use kubyl_resources::{ResourceSelection, ResourceStores, Selected, StoreHandle, StoreKey};
use kubyl_settings::{Settings, State};
use kubyl_ui::{
    ActiveColors, Button, Chip, Colors, Icon, IconName, ProdBadge, ProgressBar, StatusPill, fonts,
    h_flex, sizes, u, v_flex,
};
use serde_json::Value;

use crate::events::{EventsFeed, ui as event_ui};
use crate::settings::{OverviewSettings, OverviewState};

/// Keeps a series' color while it stays in the chart ("color follows the entity, not its rank").
#[derive(Default)]
struct ColorSlots(HashMap<SharedString, usize>);

impl ColorSlots {
    fn assign(&mut self, names: &[SharedString]) {
        self.0.retain(|name, _| names.contains(name));
        for name in names {
            if self.0.contains_key(name) {
                continue;
            }
            let free = (0..SERIES_COLORS).find(|slot| !self.0.values().any(|s| s == slot));
            self.0.insert(name.clone(), free.unwrap_or(0));
        }
    }

    fn get(&self, name: &SharedString) -> usize {
        self.0.get(name).copied().unwrap_or(0)
    }
}

/// Totals over nodes and pods.
#[derive(Default)]
struct Totals {
    nodes: usize,
    ready: usize,
    cpu_allocatable: f64,
    memory_allocatable: f64,
    pods_allocatable: f64,
    pods: usize,
    running: usize,
    failing: usize,
    cpu_requests: f64,
    cpu_limits: f64,
    memory_requests: f64,
    memory_limits: f64,
}

fn active(pod: &Value) -> bool {
    !matches!(str_at(pod, "/status/phase"), "Succeeded" | "Failed")
}

fn totals(nodes: Option<&StoreHandle>, pods: &StoreHandle, cx: &App) -> Totals {
    let mut t = Totals::default();
    if let Some(nodes) = nodes {
        for node in nodes.read(cx).objects().values() {
            t.nodes += 1;
            t.ready += node_status(node).starts_with("Ready") as usize;
            let allocatable = |key: &str| {
                parse_quantity(str_at(node, &format!("/status/allocatable/{key}"))).unwrap_or(0.0)
            };
            t.cpu_allocatable += allocatable("cpu");
            t.memory_allocatable += allocatable("memory");
            t.pods_allocatable += allocatable("pods");
        }
    }
    for pod in pods.read(cx).objects().values() {
        if !active(pod) {
            continue;
        }
        t.pods += 1;
        t.running += (str_at(pod, "/status/phase") == "Running") as usize;
        t.failing += pod_status(pod).is_degraded() as usize;
        let sum = |kind: &str, resource: &str| pod_resource(pod, kind, resource).unwrap_or(0.0);
        t.cpu_requests += sum("requests", "cpu");
        t.cpu_limits += sum("limits", "cpu");
        t.memory_requests += sum("requests", "memory");
        t.memory_limits += sum("limits", "memory");
    }
    t
}

fn percent(value: f64, of: f64) -> Option<f64> {
    (of > 0.0).then(|| value / of * 100.0)
}

/// `Amazon EKS`, `kind`…
fn distribution_name(distribution: Distribution) -> &'static str {
    match distribution {
        Distribution::Eks => "Amazon EKS",
        Distribution::Gke => "Google GKE",
        Distribution::Aks => "Azure AKS",
        other => other.label(),
    }
}

/// `1.2 cores` / `250m`.
fn cores(value: f64) -> String {
    format_cpu(value)
}

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A workload's readiness: `(ready, desired, label, tone)`.
fn workload_health(kind: &str, object: &Value) -> (i64, i64, &'static str, Tone) {
    let (ready, desired, updated) = match kind {
        "DaemonSet" => (
            int_at(object, "/status/numberReady"),
            int_at(object, "/status/desiredNumberScheduled"),
            int_at(object, "/status/updatedNumberScheduled"),
        ),
        _ => (
            int_at(object, "/status/readyReplicas"),
            object
                .pointer("/spec/replicas")
                .and_then(Value::as_i64)
                .unwrap_or(1),
            int_at(object, "/status/updatedReplicas"),
        ),
    };
    let (label, tone) = if desired == 0 {
        ("Scaled down", Tone::Muted)
    } else if ready >= desired && updated >= desired {
        ("Healthy", Tone::Good)
    } else if ready == 0 {
        ("Unavailable", Tone::Bad)
    } else {
        ("Progressing", Tone::Warning)
    };
    (ready, desired, label, tone)
}

pub struct OverviewView {
    cluster: ClusterId,
    /// Namespace variant.
    namespace: Option<String>,
    range: TimeRange,
    nodes: Option<StoreHandle>,
    pods: StoreHandle,
    workloads: Vec<(&'static str, StoreHandle)>,
    feed: Option<Entity<EventsFeed>>,
    cpu_chart: Entity<LineChart>,
    memory_chart: Entity<LineChart>,
    cpu_slots: ColorSlots,
    memory_slots: ColorSlots,
    selected_node: Option<String>,
    focus: FocusHandle,
    _ticker: Task<()>,
    /// Observers of the scope's stores and feed, replaced when the scope changes.
    _scope: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl OverviewView {
    pub fn new(cluster: ClusterId, namespace: Option<String>, cx: &mut Context<Self>) -> Self {
        let range = State::get::<OverviewState>(cx)
            .range
            .as_deref()
            .and_then(TimeRange::from_label)
            .unwrap_or_default();
        let cpu_chart = cx.new(|_| LineChart::new(ChartKind::Line, cores));
        let memory_chart = cx.new(|_| LineChart::new(ChartKind::Line, format_bytes).binary_scale());
        let mut subscriptions =
            vec![cx.observe_global::<Settings>(|this, cx| this.refresh_charts(cx))];
        if let Some(service) = MetricsService::global(cx) {
            subscriptions.push(cx.observe(&service, |this, _, cx| {
                this.refresh_charts(cx);
                cx.notify();
            }));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        // Marks the data as wanted (the metrics cache forgets unwatched data) and ages labels.
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh_charts(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        // Replaced right away by `set_namespace`; the key is the same, so it's one watch.
        let pods = ResourceStores::acquire(
            cx,
            StoreKey::new(
                cluster.clone(),
                Gvr::new("", "v1", "pods"),
                namespace.clone(),
            ),
        );
        let mut this = Self {
            cluster,
            namespace: None,
            range,
            nodes: None,
            pods,
            workloads: Vec::new(),
            feed: None,
            cpu_chart,
            memory_chart,
            cpu_slots: ColorSlots::default(),
            memory_slots: ColorSlots::default(),
            selected_node: None,
            focus: cx.focus_handle(),
            _ticker: ticker,
            _scope: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.set_namespace(namespace, cx);
        this
    }

    /// Switches between the cluster and a namespace's overview.
    fn set_namespace(&mut self, namespace: Option<String>, cx: &mut Context<Self>) {
        self.namespace = namespace.clone();
        self._scope.clear();
        let cluster = self.cluster.clone();
        let key = |resource: &str, group: &str, ns: Option<String>| {
            StoreKey::new(cluster.clone(), Gvr::new(group, "v1", resource), ns)
        };
        self.pods = ResourceStores::acquire(cx, key("pods", "", namespace.clone()));
        let mut stores = vec![self.pods.clone()];
        match &namespace {
            None => {
                let nodes = ResourceStores::acquire(cx, key("nodes", "", None));
                stores.push(nodes.clone());
                self.nodes = Some(nodes);
                self.workloads.clear();
                self.feed = None;
            }
            Some(ns) => {
                self.nodes = None;
                self.workloads = [
                    ("Deployment", "deployments"),
                    ("StatefulSet", "statefulsets"),
                    ("DaemonSet", "daemonsets"),
                ]
                .into_iter()
                .map(|(kind, resource)| {
                    (
                        kind,
                        ResourceStores::acquire(cx, key(resource, "apps", Some(ns.clone()))),
                    )
                })
                .collect();
                stores.extend(self.workloads.iter().map(|(_, s)| s.clone()));
                let feed = cx.new(EventsFeed::new);
                let cluster = self.cluster.clone();
                let ns = ns.clone();
                feed.update(cx, |feed, cx| feed.set_scope(Some(cluster), Some(ns), cx));
                self._scope.push(cx.observe(&feed, |_, _, cx| cx.notify()));
                self.feed = Some(feed);
            }
        }
        for store in stores {
            self._scope
                .push(cx.observe(store.entity(), |_, _, cx| cx.notify()));
        }
        self.cpu_slots = ColorSlots::default();
        self.memory_slots = ColorSlots::default();
        self.refresh_charts(cx);
        cx.notify();
    }

    fn set_range(&mut self, range: TimeRange, cx: &mut Context<Self>) {
        self.range = range;
        State::update::<OverviewState>(cx, |state| state.range = Some(range.label().into()));
        self.refresh_charts(cx);
        cx.notify();
    }

    fn filters(&self) -> Vec<(&str, &str)> {
        self.namespace
            .as_deref()
            .map(|ns| vec![("namespace", ns)])
            .unwrap_or_default()
    }

    fn range_key(&self, query: &'static str) -> RangeKey {
        let mut key = RangeKey::new(query, self.range);
        for (label, value) in self.filters() {
            key = key.filter(label, value);
        }
        key
    }

    /// Pushes fresh range-query results into the two charts.
    fn refresh_charts(&mut self, cx: &mut Context<Self>) {
        let Some(service) = MetricsService::global(cx) else {
            return;
        };
        let top = Settings::get::<OverviewSettings>(cx)
            .top_n
            .clamp(1, SERIES_COLORS);
        let (label, cpu_query, memory_query) = match self.namespace {
            None => ("namespace", "namespace_cpu", "namespace_memory"),
            Some(_) => ("pod", "pod_cpu", "pod_memory"),
        };
        let colors = cx.colors().clone();
        for (query, chart, memory) in [
            (cpu_query, self.cpu_chart.clone(), false),
            (memory_query, self.memory_chart.clone(), true),
        ] {
            let key = self.range_key(query);
            let (data, placeholder) = match service.read(cx).range(&self.cluster, &key) {
                RangeState::Ready(result) => {
                    let named = result
                        .series
                        .iter()
                        .map(|s| {
                            (
                                SharedString::from(s.label(label).to_string()),
                                result.aligned(s),
                            )
                        })
                        .filter(|(name, _)| !name.is_empty())
                        .collect();
                    let (top, other) = top_n(named, top);
                    let slots = if memory {
                        &mut self.memory_slots
                    } else {
                        &mut self.cpu_slots
                    };
                    let names: Vec<SharedString> = top.iter().map(|(n, _)| n.clone()).collect();
                    slots.assign(&names);
                    let mut series: Vec<Series> = top
                        .into_iter()
                        .map(|(name, values)| {
                            Series::new(
                                name.clone(),
                                name.clone(),
                                series_color(slots.get(&name), &colors),
                                values,
                            )
                        })
                        .collect();
                    if let Some(values) = other {
                        series.push(
                            Series::new("__other", "other", other_color(&colors), values).dashed(),
                        );
                    }
                    (
                        ChartData {
                            times: result.times(),
                            series,
                        },
                        SharedString::from("No samples in this range."),
                    )
                }
                RangeState::Failed(err) => (ChartData::default(), err),
                RangeState::Loading => (ChartData::default(), "Loading…".into()),
                RangeState::Unavailable => (ChartData::default(), "Needs Prometheus.".into()),
            };
            chart.update(cx, |chart, cx| {
                chart.set_placeholder(placeholder, cx);
                chart.set_data(data, cx);
            });
        }
    }

    /// Samples of a single-series range query for a KPI sparkline.
    fn spark(&self, query: &'static str, cx: &App) -> Vec<f64> {
        let Some(service) = MetricsService::global(cx) else {
            return Vec::new();
        };
        match service
            .read(cx)
            .range(&self.cluster, &self.range_key(query))
        {
            RangeState::Ready(result) => result.total().into_iter().flatten().collect(),
            _ => Vec::new(),
        }
    }

    fn select_node(&mut self, name: &str, cx: &mut Context<Self>) {
        self.selected_node = Some(name.to_string());
        self.publish_node(cx);
        cx.notify();
    }

    /// Makes the selected node the [`ResourceSelection`], so the explorer's Cordon/Drain
    /// actions (and the details dock) act on it.
    fn publish_node(&self, cx: &mut App) -> bool {
        let (Some(name), Some(nodes)) = (&self.selected_node, &self.nodes) else {
            return false;
        };
        let object = nodes.read(cx).get(name.as_str()).cloned();
        let caps = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(&self.cluster))
            .unwrap_or_default();
        ResourceSelection::set(
            cx,
            ResourceSelection {
                items: vec![Selected {
                    target: ResourceRef::object(
                        self.cluster.clone(),
                        Gvr::new("", "v1", "nodes"),
                        None,
                        name.clone(),
                    ),
                    kind: "Node".into(),
                    object,
                    store: Some(nodes.entity().clone()),
                }],
                caps,
            },
        );
        true
    }

    fn open_details(&self, target: ResourceRef, window: &mut Window, cx: &mut App) {
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(
                ViewKind::Details,
                target,
            ))),
            cx,
        );
    }

    // ----- Rendering -----

    fn header(&self, source: &Source, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let manager = ConnectionManager::try_global(cx);
        let (name, production, meta) = match &manager {
            Some(manager) => {
                let manager = manager.read(cx);
                let info = manager.cluster(&self.cluster).and_then(|c| c.info.as_ref());
                let mut meta = Vec::new();
                if let Some(info) = info {
                    let version = info
                        .version
                        .split(['-', '+'])
                        .next()
                        .unwrap_or(&info.version)
                        .to_string();
                    meta.push(distribution_name(info.distribution).to_string());
                    meta.push(version);
                }
                match &self.namespace {
                    None => {
                        if let Some(nodes) = &self.nodes {
                            meta.push(format!("{} nodes", nodes.read(cx).len()));
                        }
                        let namespaces = manager.namespaces(&self.cluster);
                        if namespaces.listed {
                            meta.push(format!("{} namespaces", namespaces.names.len()));
                        }
                    }
                    Some(_) => {
                        meta.insert(
                            0,
                            format!("namespace in {}", manager.display_name(&self.cluster)),
                        );
                    }
                }
                (
                    manager.display_name(&self.cluster).to_string(),
                    manager.caps(&self.cluster).production,
                    meta.join(" · "),
                )
            }
            None => (self.cluster.to_string(), false, String::new()),
        };
        let title = self.namespace.clone().unwrap_or(name);
        let active_namespace = ActiveContext::global(cx)
            .cluster
            .as_ref()
            .filter(|c| c.id == self.cluster)
            .and_then(|_| ActiveContext::global(cx).namespace.clone());
        let scope_switch = match (&self.namespace, active_namespace) {
            (Some(_), _) => Some(("Cluster overview".to_string(), None)),
            (None, Some(ns)) => Some((format!("{ns} overview"), Some(ns.to_string()))),
            (None, None) => None,
        };
        h_flex()
            .gap(u(10.0))
            .child(
                div()
                    .text_size(u(18.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .when(production && self.namespace.is_none(), |this| {
                this.child(ProdBadge)
            })
            .child(
                div()
                    .flex_none()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(meta),
            )
            .when_some(scope_switch, |this, (label, namespace)| {
                this.child(
                    div()
                        .id("overview-scope")
                        .cursor_pointer()
                        .child(Chip::new(label).icon(IconName::ArrowRight))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_namespace(namespace.clone(), cx)
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .min_w_0()
                    .flex_shrink_1()
                    .child(self.source_chip(source, &colors)),
            )
            .child(
                TimeRangePicker::new("overview-range", self.range)
                    .disabled(!source.has_history())
                    .on_change({
                        let weak = cx.weak_entity();
                        move |range, _, cx| {
                            weak.update(cx, |this, cx| this.set_range(range, cx)).ok();
                        }
                    }),
            )
    }

    /// `Prometheus · monitoring/prometheus-k8s`; truncates first when the header is narrow.
    fn source_chip(&self, source: &Source, colors: &Colors) -> impl IntoElement {
        let (label, color): (String, Hsla) = match source {
            Source::Prometheus { target } => {
                (format!("Prometheus · {}", target.label()), colors.green)
            }
            Source::MetricsServer { .. } => ("metrics-server".into(), colors.text_muted),
            Source::None { .. } => ("No metrics".into(), colors.text_dim),
            Source::Detecting | Source::Unknown => ("Looking for metrics…".into(), colors.text_dim),
        };
        let cluster = self.cluster.clone();
        MenuButton::new("overview-source")
            .ghost()
            .compact()
            .p_0()
            .min_w_0()
            .child(
                h_flex()
                    .min_w_0()
                    .h(u(20.0))
                    .px(u(7.0))
                    .gap(u(5.0))
                    .rounded(u(4.0))
                    .bg(colors.chip_background)
                    .text_size(u(11.5))
                    .text_color(color)
                    .child(Icon::new(IconName::Activity).size(11.0).color(color))
                    .child(div().min_w_0().truncate().child(label)),
            )
            .dropdown_menu(move |menu, _, _| {
                let cluster = cluster.clone();
                menu.item(PopupMenuItem::new("Look for Prometheus again").on_click(
                    move |_, _, cx| {
                        if let Some(service) = MetricsService::global(cx) {
                            service.update(cx, |s, cx| s.redetect(&cluster, cx));
                        }
                    },
                ))
                .item(
                    PopupMenuItem::new("Configure in settings.json…").on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(OpenSettings), cx)
                    }),
                )
            })
    }

    fn kpis(
        &self,
        totals: &Totals,
        usage: Option<Usage>,
        source: &Source,
        cx: &App,
    ) -> impl IntoElement {
        let colors = cx.colors().clone();
        let sampled = match (source, MetricsService::global(cx)) {
            (Source::MetricsServer { .. }, Some(service)) if self.namespace.is_none() => {
                service.read(cx).sampled_totals(&self.cluster)
            }
            _ => Vec::new(),
        };
        let history = source.has_history();
        let spark = |query: &'static str, pick: &dyn Fn(&Usage) -> f64| {
            if history {
                self.spark(query, cx)
            } else {
                sampled.iter().map(|(_, u)| pick(u)).collect()
            }
        };
        let pct = |value: f64, of: f64| {
            percent(value, of)
                .map(|p| format!("{p:.0}%"))
                .unwrap_or_else(|| "—".into())
        };
        // The big number: one decimal below 10% so a quiet cluster doesn't read "0%".
        let big_pct = |value: f64, of: f64| match percent(value, of) {
            Some(p) if p < 9.95 => format!("{p:.1}%"),
            Some(p) => format!("{p:.0}%"),
            None => "—".into(),
        };
        let namespace = self.namespace.is_some();

        // CPU
        let (cpu_big, cpu_sub) = match usage {
            Some(u) if !namespace => (
                big_pct(u.cpu, totals.cpu_allocatable),
                format!("{} / {} cores", cores(u.cpu), cores(totals.cpu_allocatable)),
            ),
            Some(u) => (
                cores(u.cpu),
                format!("of {} req", cores(totals.cpu_requests)),
            ),
            None if !namespace => (
                "—".into(),
                format!("{} cores allocatable", cores(totals.cpu_allocatable)),
            ),
            None => (
                "—".into(),
                format!("{} requested", cores(totals.cpu_requests)),
            ),
        };
        let cpu_right = if namespace {
            format!("lim {}", cores(totals.cpu_limits))
        } else {
            format!(
                "req {} · lim {}",
                pct(totals.cpu_requests, totals.cpu_allocatable),
                pct(totals.cpu_limits, totals.cpu_allocatable)
            )
        };
        // Memory
        let (memory_big, memory_sub) = match usage {
            Some(u) if !namespace => (
                big_pct(u.memory, totals.memory_allocatable),
                format!(
                    "{} / {}",
                    format_bytes(u.memory),
                    format_bytes(totals.memory_allocatable)
                ),
            ),
            Some(u) => (
                format_bytes(u.memory),
                format!("of {} req", format_bytes(totals.memory_requests)),
            ),
            None if !namespace => (
                "—".into(),
                format!("{} allocatable", format_bytes(totals.memory_allocatable)),
            ),
            None => (
                "—".into(),
                format!("{} requested", format_bytes(totals.memory_requests)),
            ),
        };
        let memory_right = if namespace {
            format!("lim {}", format_bytes(totals.memory_limits))
        } else {
            format!(
                "req {} · lim {}",
                pct(totals.memory_requests, totals.memory_allocatable),
                pct(totals.memory_limits, totals.memory_allocatable)
            )
        };
        // Pods
        let pods_card = if namespace {
            kpi(
                "Pods",
                if totals.failing > 0 {
                    (format!("{} failing", totals.failing), colors.red)
                } else {
                    ("none failing".into(), colors.text_dim)
                },
                format_count(totals.pods),
                format!("{} running", totals.running),
                spark("pods_running", &|_| 0.0),
                colors.cyan,
                &colors,
            )
        } else {
            kpi(
                "Pods",
                (
                    pct(totals.pods as f64, totals.pods_allocatable),
                    colors.text_dim,
                ),
                format_count(totals.pods),
                format!(
                    "of {} capacity",
                    format_count(totals.pods_allocatable as usize)
                ),
                spark("pods_running", &|_| 0.0),
                colors.cyan,
                &colors,
            )
        };
        // Nodes, or workloads in the namespace variant.
        let last = if namespace {
            let (healthy, total) = self
                .workloads
                .iter()
                .flat_map(|(kind, store)| {
                    store
                        .read(cx)
                        .objects()
                        .values()
                        .map(|w| workload_health(kind, w).3 == Tone::Good)
                        .collect::<Vec<_>>()
                })
                .fold((0, 0), |(h, t), ok| (h + ok as usize, t + 1));
            let unhealthy = total - healthy;
            kpi(
                "Workloads",
                if unhealthy > 0 {
                    (format!("{unhealthy} not ready"), colors.red)
                } else {
                    ("all ready".into(), colors.text_dim)
                },
                format!("{healthy} / {total}"),
                "healthy".into(),
                Vec::new(),
                colors.green,
                &colors,
            )
        } else {
            let not_ready = totals.nodes - totals.ready;
            kpi(
                "Nodes",
                if not_ready > 0 {
                    (format!("{not_ready} NotReady"), colors.red)
                } else {
                    ("all ready".into(), colors.text_dim)
                },
                format!("{} / {}", totals.ready, totals.nodes),
                "ready".into(),
                spark("nodes_ready", &|_| 0.0),
                colors.green,
                &colors,
            )
        };
        h_flex()
            .items_stretch()
            .gap(u(12.0))
            .child(kpi(
                "CPU",
                (cpu_right, colors.text_dim),
                cpu_big,
                cpu_sub,
                spark("cluster_cpu", &|u| u.cpu),
                colors.accent,
                &colors,
            ))
            .child(kpi(
                "Memory",
                (memory_right, colors.text_dim),
                memory_big,
                memory_sub,
                spark("cluster_memory", &|u| u.memory),
                colors.purple,
                &colors,
            ))
            .child(pods_card)
            .child(last)
    }

    fn charts(&self, source: &Source, cx: &App) -> AnyElement {
        let colors = cx.colors().clone();
        if !source.has_history() && !matches!(source, Source::Unknown | Source::Detecting) {
            return self.connect_hint(source, &colors).into_any_element();
        }
        let (cpu_title, memory_title, by) = match self.namespace {
            None => ("CPU by namespace", "Memory working set", "by namespace"),
            Some(_) => ("CPU by pod", "Memory working set by pod", "by pod"),
        };
        h_flex()
            .items_stretch()
            .gap(u(12.0))
            .child(chart_card(
                cpu_title,
                "cores · rate(container_cpu_usage_seconds_total[5m])".into(),
                self.cpu_chart.clone(),
                &colors,
            ))
            .child(chart_card(
                memory_title,
                format!("bytes · {by}"),
                self.memory_chart.clone(),
                &colors,
            ))
            .into_any_element()
    }

    /// Replaces the charts without Prometheus.
    fn connect_hint(&self, source: &Source, colors: &Colors) -> impl IntoElement {
        let reason = match source {
            Source::MetricsServer { note } => format!(
                "{note} Current usage comes from metrics-server; charts and time ranges need Prometheus."
            ),
            Source::None { reason } => reason.to_string(),
            _ => String::new(),
        };
        let cluster = self.cluster.clone();
        card(colors)
            .p(u(14.0))
            .gap(u(8.0))
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(Icon::new(IconName::Activity).color(colors.text_dim))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Connect Prometheus for charts"),
                    ),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_muted)
                    .child(reason),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(
                        "Kubyl looks for kube-prometheus-stack, prometheus-operated, prometheus-server, \
                         OpenShift's thanos-querier and VictoriaMetrics through the API server's service \
                         proxy. Point it at another one with metrics.prometheus in settings.json.",
                    ),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .pt(u(4.0))
                    .child(Button::new("prometheus-redetect").label("Look again").on_click(
                        move |_, _, cx| {
                            if let Some(service) = MetricsService::global(cx) {
                                service.update(cx, |s, cx| s.redetect(&cluster, cx));
                            }
                        },
                    ))
                    .child(
                        Button::new("prometheus-settings")
                            .ghost()
                            .label("Open settings.json")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenSettings), cx)
                            }),
                    ),
            )
    }

    fn nodes_card(&self, pods: &[Arc<Value>], cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(store) = &self.nodes else {
            return div().into_any_element();
        };
        let usage = MetricsService::global(cx)
            .and_then(|s| s.read(cx).nodes(&self.cluster).cloned())
            .unwrap_or_default();
        let mut per_node: HashMap<&str, usize> = HashMap::new();
        for pod in pods.iter().filter(|p| active(p)) {
            *per_node.entry(str_at(pod, "/spec/nodeName")).or_default() += 1;
        }
        let mut nodes: Vec<Arc<Value>> = store.read(cx).objects().values().cloned().collect();
        nodes.sort_by(|a, b| {
            kubyl_explorer::list::natural_cmp(
                str_at(a, "/metadata/name"),
                str_at(b, "/metadata/name"),
            )
        });
        let selected = self
            .selected_node
            .as_ref()
            .and_then(|name| nodes.iter().find(|n| str_at(n, "/metadata/name") == name));
        let cordoned = selected.is_some_and(|n| {
            n.pointer("/spec/unschedulable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        let has_selection = selected.is_some();
        const W: [f32; 6] = [84.0, 110.0, 110.0, 50.0, 96.0, 64.0];
        let cell = |w: f32| div().flex_none().w(u(w)).min_w_0().truncate();
        let header = h_flex()
            .h(u(sizes::TABLE_HEADER))
            .px(u(12.0))
            .bg(colors.subheader_background)
            .border_y_1()
            .border_color(colors.border_variant)
            .text_size(u(11.5))
            .text_color(colors.text_dim)
            .child(div().flex_1().child("NAME"))
            .children(
                ["STATUS", "CPU", "MEMORY", "PODS", "TYPE", "KUBELET"]
                    .iter()
                    .zip(W)
                    .map(|(t, w)| cell(w).child(*t)),
            );
        let bar_cell = |value: Option<f64>, w: f32| {
            h_flex()
                .flex_none()
                .w(u(w))
                .gap(u(6.0))
                .pr(u(12.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(34.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(value.map(|v| format!("{v:.0}%")).unwrap_or("—".into())),
                )
                .when_some(value, |this, v| {
                    this.child(
                        div()
                            .flex_none()
                            .w(u(56.0))
                            .child(ProgressBar::new(v as f32)),
                    )
                })
        };
        let rows = nodes.iter().take(200).enumerate().map(|(i, node)| {
            let name = str_at(node, "/metadata/name").to_string();
            let status = node_status(node);
            let tone = if status.starts_with("NotReady") {
                Tone::Bad
            } else if status.starts_with("Ready") && !status.contains("SchedulingDisabled") {
                Tone::Good
            } else {
                Tone::Warning
            };
            let allocatable =
                |key: &str| parse_quantity(str_at(node, &format!("/status/allocatable/{key}")));
            let used = usage.get(&name);
            let cpu = used.and_then(|u| percent(u.cpu, allocatable("cpu")?));
            let memory = used.and_then(|u| percent(u.memory, allocatable("memory")?));
            let labels = &node["metadata"]["labels"];
            let instance = labels["node.kubernetes.io/instance-type"]
                .as_str()
                .or(labels["beta.kubernetes.io/instance-type"].as_str())
                .unwrap_or("—")
                .to_string();
            let kubelet = str_at(node, "/status/nodeInfo/kubeletVersion").to_string();
            let is_selected = self.selected_node.as_deref() == Some(name.as_str());
            let select = name.clone();
            let cluster = self.cluster.clone();
            h_flex()
                .id(("node", i))
                .h(u(sizes::TABLE_ROW))
                .px(u(12.0))
                .border_b_1()
                .border_color(colors.row_border)
                .cursor_pointer()
                .when(is_selected, |this| {
                    this.bg(colors.selection)
                        .border_1()
                        .border_color(colors.accent)
                })
                .when(!is_selected, |this| {
                    this.hover(|this| this.bg(colors.hover))
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(name.clone()),
                )
                .child(cell(W[0]).child(StatusPill::new(
                    if status == "Ready,SchedulingDisabled" {
                        "Cordoned".to_string()
                    } else {
                        status.split(',').next().unwrap_or_default().to_string()
                    },
                    tone,
                )))
                .child(bar_cell(cpu, W[1]))
                .child(bar_cell(memory, W[2]))
                .child(
                    cell(W[3])
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(
                            per_node
                                .get(name.as_str())
                                .copied()
                                .unwrap_or(0)
                                .to_string(),
                        ),
                )
                .child(
                    cell(W[4])
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(instance),
                )
                .child(
                    cell(W[5])
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(kubelet),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.select_node(&select, cx);
                    if event.click_count() > 1 {
                        this.open_details(
                            ResourceRef::object(
                                cluster.clone(),
                                Gvr::new("", "v1", "nodes"),
                                None,
                                select.clone(),
                            ),
                            window,
                            cx,
                        );
                    }
                }))
        });
        let action_button = |id: &'static str,
                             label: &'static str,
                             icon: Option<IconName>,
                             action: fn() -> Box<dyn gpui::Action>| {
            let mut button = Button::new(id)
                .ghost()
                .label(label)
                .disabled(!has_selection);
            if let Some(icon) = icon {
                button = button.icon(icon);
            }
            button.on_click(cx.listener(move |this, _, window, cx| {
                if this.publish_node(cx) {
                    window.dispatch_action(action(), cx);
                }
            }))
        };
        card(&colors)
            .overflow_hidden()
            .child(
                h_flex()
                    .px(u(12.0))
                    .py(u(10.0))
                    .gap(u(8.0))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Nodes"))
                    .child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(nodes.len().to_string()),
                    )
                    .child(div().flex_1())
                    .child(if cordoned {
                        action_button("node-uncordon", "Uncordon", Some(IconName::Server), || {
                            Box::new(kubyl_explorer::actions::Uncordon)
                        })
                    } else {
                        action_button("node-cordon", "Cordon", Some(IconName::Server), || {
                            Box::new(kubyl_explorer::actions::Cordon)
                        })
                    })
                    .child(action_button("node-drain", "Drain…", None, || {
                        Box::new(kubyl_explorer::actions::Drain)
                    })),
            )
            .child(header)
            .children(rows)
            .into_any_element()
    }

    fn workloads_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let mut rows: Vec<(&'static str, Arc<Value>)> = self
            .workloads
            .iter()
            .flat_map(|(kind, store)| {
                store
                    .read(cx)
                    .objects()
                    .values()
                    .map(|w| (*kind, w.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        // Problems first, then by name.
        rows.sort_by(|(ka, a), (kb, b)| {
            let rank = |kind: &str, w: &Value| match workload_health(kind, w).3 {
                Tone::Bad => 0,
                Tone::Warning => 1,
                _ => 2,
            };
            rank(ka, a).cmp(&rank(kb, b)).then_with(|| {
                kubyl_explorer::list::natural_cmp(
                    str_at(a, "/metadata/name"),
                    str_at(b, "/metadata/name"),
                )
            })
        });
        let cluster = self.cluster.clone();
        let namespace = self.namespace.clone();
        let list = rows.into_iter().take(12).enumerate().map(|(i, (kind, w))| {
            let (ready, desired, label, tone) = workload_health(kind, &w);
            let name = str_at(&w, "/metadata/name").to_string();
            let resource = match kind {
                "Deployment" => "deployments",
                "StatefulSet" => "statefulsets",
                _ => "daemonsets",
            };
            let target = ResourceRef::object(
                cluster.clone(),
                Gvr::new("apps", "v1", resource),
                namespace.clone(),
                name.clone(),
            );
            h_flex()
                .id(("workload", i))
                .h(u(sizes::TABLE_ROW))
                .px(u(12.0))
                .gap(u(8.0))
                .border_t_1()
                .border_color(colors.row_border)
                .cursor_pointer()
                .hover(|this| this.bg(colors.hover))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(name),
                )
                .child(
                    div()
                        .flex_none()
                        .w(u(84.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(kind),
                )
                .child(
                    div()
                        .flex_none()
                        .w(u(48.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(if ready < desired {
                            colors.red
                        } else {
                            colors.text
                        })
                        .child(format!("{ready}/{desired}")),
                )
                .child(
                    div()
                        .flex_none()
                        .w(u(96.0))
                        .child(StatusPill::new(label, tone)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_details(target.clone(), window, cx)
                }))
        });
        card(&colors)
            .overflow_hidden()
            .flex_1()
            .min_w_0()
            .child(card_title("Workloads", "health", &colors))
            .children(list)
    }

    fn top_pods_card(&self, pods: &[Arc<Value>], cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let usage = MetricsService::global(cx)
            .and_then(|s| s.read(cx).pods(&self.cluster).cloned())
            .unwrap_or_default();
        let mut ranked: Vec<(Arc<Value>, Usage)> = pods
            .iter()
            .filter(|p| active(p))
            .filter_map(|p| {
                let key = kubyl_resources::key_of(p);
                usage.get(&key).map(|u| (p.clone(), *u))
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.1.cpu
                .partial_cmp(&a.1.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let cluster = self.cluster.clone();
        let namespace = self.namespace.clone();
        let empty = ranked.is_empty();
        let meter = |value: f64, pod: &Value, resource: &str, format: fn(f64) -> String| {
            let limit = pod_resource(pod, "limits", resource);
            let request = pod_resource(pod, "requests", resource);
            let scale = limit.or(request);
            h_flex()
                .flex_none()
                .w(u(96.0))
                .gap(u(6.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(44.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(format(value)),
                )
                .when_some(scale.filter(|s| *s > 0.0), |this, scale| {
                    this.child(
                        Meter::new((value / scale * 100.0) as f32)
                            .marker(
                                request
                                    .filter(|_| limit.is_some())
                                    .map(|r| (r / scale * 100.0) as f32),
                            )
                            .width(44.0),
                    )
                })
        };
        let rows = ranked
            .into_iter()
            .take(8)
            .enumerate()
            .map(|(i, (pod, u_))| {
                let name = str_at(&pod, "/metadata/name").to_string();
                let restarts = pod_status(&pod).restarts;
                let target = ResourceRef::object(
                    cluster.clone(),
                    Gvr::new("", "v1", "pods"),
                    namespace.clone(),
                    name.clone(),
                );
                h_flex()
                    .id(("top-pod", i))
                    .h(u(sizes::TABLE_ROW))
                    .px(u(12.0))
                    .gap(u(8.0))
                    .border_t_1()
                    .border_color(colors.row_border)
                    .cursor_pointer()
                    .hover(|this| this.bg(colors.hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(name),
                    )
                    .child(meter(u_.cpu, &pod, "cpu", format_cpu))
                    .child(meter(u_.memory, &pod, "memory", format_bytes))
                    .child(
                        div()
                            .flex_none()
                            .w(u(28.0))
                            .text_right()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .text_color(if restarts >= 3 {
                                colors.red
                            } else {
                                colors.text
                            })
                            .child(restarts.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_details(target.clone(), window, cx)
                    }))
            });
        card(&colors)
            .overflow_hidden()
            .flex_1()
            .min_w_0()
            .child(card_title("Top pods", "by CPU · bars vs limit", &colors))
            .children(rows)
            .when(empty, |this| {
                this.child(
                    div()
                        .px(u(12.0))
                        .pb(u(12.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("No usage yet."),
                )
            })
    }

    fn warnings_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(feed) = &self.feed else {
            return div().into_any_element();
        };
        let now = jiff::Timestamp::now();
        let rows: Vec<_> = feed
            .read(cx)
            .rows()
            .iter()
            .filter(|r| r.warning)
            .take(5)
            .cloned()
            .collect();
        let empty = rows.is_empty();
        card(&colors)
            .overflow_hidden()
            .child(card_title("Recent warnings", "", &colors))
            .children(
                rows.iter()
                    .enumerate()
                    .map(|(i, row)| event_ui::event_item(i, row, feed, now, &colors)),
            )
            .when(empty, |this| {
                this.child(
                    div()
                        .px(u(12.0))
                        .pb(u(12.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("No warnings."),
                )
            })
            .into_any_element()
    }
}

fn card(colors: &Colors) -> gpui::Div {
    v_flex()
        .bg(colors.panel)
        .border_1()
        .border_color(colors.border)
        .rounded(u(8.0))
}

fn card_title(title: &'static str, sub: &'static str, colors: &Colors) -> impl IntoElement {
    h_flex()
        .px(u(12.0))
        .py(u(10.0))
        .gap(u(8.0))
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .child(div().flex_1())
        .child(
            div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child(sub),
        )
}

/// A KPI tile: title and a note on the right, a big number with a subtitle, a sparkline.
fn kpi(
    title: &'static str,
    right: (String, Hsla),
    big: String,
    sub: String,
    spark: Vec<f64>,
    color: Hsla,
    colors: &Colors,
) -> gpui::Div {
    card(colors)
        .flex_1()
        .min_w_0()
        .px(u(14.0))
        .py(u(12.0))
        .gap(u(6.0))
        .child(
            h_flex()
                .justify_between()
                .gap(u(8.0))
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(title)
                .child(
                    div()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(right.1)
                        .child(right.0),
                ),
        )
        .child(
            h_flex()
                .items_baseline()
                .gap(u(8.0))
                .child(
                    div()
                        .text_size(u(24.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(big),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(sub),
                ),
        )
        .child(Sparkline::new(spark, color).height(30.0))
}

fn chart_card(
    title: &'static str,
    sub: String,
    chart: Entity<LineChart>,
    colors: &Colors,
) -> impl IntoElement {
    card(colors)
        .flex_1()
        .min_w_0()
        .px(u(14.0))
        .py(u(12.0))
        .child(
            h_flex()
                .justify_between()
                .gap(u(8.0))
                .mb(u(8.0))
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .child(
                    div()
                        .truncate()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(sub),
                ),
        )
        .child(chart)
}

impl Focusable for OverviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for OverviewView {
    fn tab_title(&self, _: &App) -> SharedString {
        match &self.namespace {
            Some(ns) => format!("{ns} · Overview").into(),
            None => "Overview".into(),
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Gauge.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::for_resource(
            ViewKind::Overview,
            ResourceRef::list(
                self.cluster.clone(),
                Gvr::new("", "", ""),
                self.namespace.clone(),
            ),
        ))
    }
}

impl Render for OverviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let service = MetricsService::global(cx);
        let source = service
            .as_ref()
            .map(|s| s.read(cx).source(&self.cluster))
            .unwrap_or(Source::Unknown);
        let totals = totals(self.nodes.as_ref(), &self.pods, cx);
        let pods: Vec<Arc<Value>> = self.pods.read(cx).objects().values().cloned().collect();
        // Current usage: node totals (cluster), or the namespace's pods.
        let usage = service.as_ref().and_then(|s| {
            let s = s.read(cx);
            match &self.namespace {
                None => s
                    .nodes(&self.cluster)
                    .filter(|n| !n.is_empty())
                    .map(|nodes| {
                        nodes.values().fold(Usage::default(), |a, b| Usage {
                            cpu: a.cpu + b.cpu,
                            memory: a.memory + b.memory,
                        })
                    }),
                Some(ns) => {
                    let prefix = format!("{ns}/");
                    s.pods(&self.cluster).map(|pods| {
                        pods.iter().filter(|(k, _)| k.starts_with(&prefix)).fold(
                            Usage::default(),
                            |a, (_, b)| Usage {
                                cpu: a.cpu + b.cpu,
                                memory: a.memory + b.memory,
                            },
                        )
                    })
                }
            }
        });
        let body = v_flex()
            .px(u(18.0))
            .py(u(16.0))
            .gap(u(14.0))
            .child(self.header(&source, cx))
            .child(self.kpis(&totals, usage, &source, cx))
            .child(self.charts(&source, cx))
            .map(|this| match self.namespace {
                None => this.child(self.nodes_card(&pods, cx)),
                Some(_) => this
                    .child(
                        h_flex()
                            .items_start()
                            .gap(u(12.0))
                            .child(self.workloads_card(cx))
                            .child(self.top_pods_card(&pods, cx)),
                    )
                    .child(self.warnings_card(cx)),
            });
        div()
            .id("overview")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .bg(colors.background)
            .text_color(colors.text)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn colors_follow_the_series() {
        let mut slots = ColorSlots::default();
        let names = |n: &[&str]| {
            n.iter()
                .map(|s| SharedString::from(s.to_string()))
                .collect::<Vec<_>>()
        };
        slots.assign(&names(&["a", "b", "c"]));
        let (a, c) = (slots.get(&"a".into()), slots.get(&"c".into()));
        // "b" drops out, "d" comes in: survivors keep their colors, "d" takes the free slot.
        slots.assign(&names(&["c", "a", "d"]));
        assert_eq!(slots.get(&"a".into()), a);
        assert_eq!(slots.get(&"c".into()), c);
        assert_eq!(slots.get(&"d".into()), 1);
    }

    #[test]
    fn workload_health_states() {
        let deployment = |replicas: i64, ready: i64, updated: i64| json!({"spec": {"replicas": replicas}, "status": {"readyReplicas": ready, "updatedReplicas": updated}});
        assert_eq!(
            workload_health("Deployment", &deployment(3, 3, 3)).2,
            "Healthy"
        );
        assert_eq!(
            workload_health("Deployment", &deployment(3, 2, 3)).2,
            "Progressing"
        );
        assert_eq!(
            workload_health("Deployment", &deployment(3, 0, 3)).2,
            "Unavailable"
        );
        assert_eq!(
            workload_health("Deployment", &deployment(0, 0, 0)).2,
            "Scaled down"
        );
        let ds = json!({"status": {"desiredNumberScheduled": 2, "numberReady": 2, "updatedNumberScheduled": 2}});
        assert_eq!(workload_health("DaemonSet", &ds).2, "Healthy");
    }

    #[test]
    fn numbers() {
        assert_eq!(format_count(1320), "1,320");
        assert_eq!(format_count(842), "842");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(percent(1.0, 0.0), None);
        assert_eq!(percent(1.0, 4.0), Some(25.0));
        assert_eq!(distribution_name(Distribution::Eks), "Amazon EKS");
        assert_eq!(distribution_name(Distribution::Kind), "kind");
    }
}
