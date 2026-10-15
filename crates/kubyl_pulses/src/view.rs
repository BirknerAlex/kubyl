//! The Pulses tab: a tile per kind with its healthy and faulty counts and their history.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    App, Context, FocusHandle, Focusable, FontWeight, IntoElement, KeyBinding, Render,
    SharedString, Subscription, Task, Window, actions, div, prelude::*,
};
use kubyl_charts::{Sparkline, usage_color};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ClusterId, Gvr, ResourceRef, TabView, ViewKind,
    ViewRegistry, ViewRequest,
};
use kubyl_kube::ConnectionManager;
use kubyl_pulses_core::{Count, History, Kind, Samples, count, percent};
use kubyl_resources::metrics::Metrics;
use kubyl_resources::store::find_resource;
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_resources_core::format::{format_bytes, format_cpu, parse_quantity};
use kubyl_ui::{ActiveColors, Icon, IconName, fonts, h_flex, sizes, u, v_flex};
use serde_json::Value;

actions!(
    pulses_view,
    [
        /// Moves to the next tile.
        NextTile,
        /// Moves to the previous tile.
        PreviousTile,
        /// Opens the list of the kind of the tile.
        OpenTile,
    ]
);

/// Key context of the tab.
pub const CONTEXT: &str = "PulsesView";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", NextTile, Some(CONTEXT)),
        KeyBinding::new("right", NextTile, Some(CONTEXT)),
        KeyBinding::new("down", NextTile, Some(CONTEXT)),
        KeyBinding::new("l", NextTile, Some(CONTEXT)),
        KeyBinding::new("j", NextTile, Some(CONTEXT)),
        KeyBinding::new("shift-tab", PreviousTile, Some(CONTEXT)),
        KeyBinding::new("left", PreviousTile, Some(CONTEXT)),
        KeyBinding::new("up", PreviousTile, Some(CONTEXT)),
        KeyBinding::new("h", PreviousTile, Some(CONTEXT)),
        KeyBinding::new("k", PreviousTile, Some(CONTEXT)),
        KeyBinding::new("enter", OpenTile, Some(CONTEXT)),
    ]);
    ActionRegistry::register(
        cx,
        ActionSpec::new("Pulses: Open List", OpenTile)
            .hint("Open")
            .in_context(CONTEXT),
    );
}

/// How often the history gets a sample.
const SAMPLE: Duration = Duration::from_secs(10);

/// Cluster CPU and memory: what the nodes use, of what they can allocate.
#[derive(Clone, Copy)]
struct Gauges {
    cpu_used: f64,
    cpu_allocatable: f64,
    memory_used: f64,
    memory_allocatable: f64,
}

struct Tile {
    store: StoreHandle,
    history: History,
}

pub struct PulsesView {
    cluster: ClusterId,
    namespace: Option<String>,
    tiles: HashMap<Kind, Tile>,
    selected: usize,
    /// The nodes, for the CPU and memory tiles (which are cluster-wide in every namespace).
    nodes: Option<StoreHandle>,
    cpu: Samples,
    memory: Samples,
    focus: FocusHandle,
    observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
    _sampler: Task<()>,
}

impl PulsesView {
    pub fn new(target: ResourceRef, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        // Discovery arrives after the connection: watch what's served as soon as it is.
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |this, _, cx| this.sync(cx)));
        }
        subscriptions.push(cx.observe_global::<Metrics>(|_, cx| cx.notify()));
        let sampler = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.sample(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(SAMPLE).await;
            }
        });
        let mut this = Self {
            cluster: target.cluster,
            namespace: target.namespace,
            tiles: HashMap::new(),
            selected: 0,
            nodes: None,
            cpu: Samples::default(),
            memory: Samples::default(),
            focus: cx.focus_handle(),
            observers: Vec::new(),
            _subscriptions: subscriptions,
            _sampler: sampler,
        };
        this.sync(cx);
        this
    }

    fn resolve(&self, kind: Kind, cx: &App) -> Option<Gvr> {
        let (group, resource) = kind.api();
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(&self.cluster)?;
        find_resource(&discovery.resources, &Gvr::new(group, "", resource))
            .map(|info| info.gvr.clone())
    }

    /// Watches the kinds the cluster serves; only changes start watches.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for kind in Kind::shown(self.namespace.is_some()) {
            if self.tiles.contains_key(&kind) {
                continue;
            }
            let Some(gvr) = self.resolve(kind, cx) else {
                continue;
            };
            // Only the namespace being looked at: less to list and watch, and it works where
            // RBAC allows nothing cluster-wide.
            let namespace = self.namespace.clone().filter(|_| kind.namespaced());
            let store =
                ResourceStores::acquire(cx, StoreKey::new(self.cluster.clone(), gvr, namespace));
            self.tiles.insert(
                kind,
                Tile {
                    store,
                    history: History::default(),
                },
            );
            changed = true;
        }
        if self.nodes.is_none()
            && let Some(gvr) = self.resolve(Kind::Nodes, cx)
        {
            self.nodes = Some(ResourceStores::acquire(
                cx,
                StoreKey::new(self.cluster.clone(), gvr, None),
            ));
            changed = true;
        }
        if changed {
            // The nodes feed the CPU and memory tiles, also when no tile shows them.
            self.observers = self
                .tiles
                .values()
                .map(|t| t.store.entity())
                .chain(self.nodes.as_ref().map(|n| n.entity()))
                .map(|store| cx.observe(store, |_, _, cx| cx.notify()))
                .collect();
            cx.notify();
        }
    }

    fn current(&self, kind: Kind, cx: &App) -> Option<Count> {
        let tile = self.tiles.get(&kind)?;
        let store = tile.store.read(cx);
        let namespace = self.namespace.as_deref().filter(|_| kind.namespaced());
        Some(count(
            kind,
            store.objects().values().map(|o| o.as_ref()).filter(|o| {
                namespace.is_none_or(|ns| {
                    o.pointer("/metadata/namespace").and_then(Value::as_str) == Some(ns)
                })
            }),
        ))
    }

    /// What the nodes use and can allocate, when a metrics source reports it.
    fn gauges(&self, cx: &App) -> Option<Gauges> {
        let provider = Metrics::provider(cx)?;
        let nodes = self.nodes.as_ref()?.read(cx);
        let mut gauges = Gauges {
            cpu_used: 0.0,
            cpu_allocatable: 0.0,
            memory_used: 0.0,
            memory_allocatable: 0.0,
        };
        let mut any = false;
        for node in nodes.objects().values() {
            let Some(name) = node.pointer("/metadata/name").and_then(Value::as_str) else {
                continue;
            };
            let Some(usage) = provider.node_usage(&self.cluster, name, cx) else {
                continue;
            };
            let quantity = |key: &str| {
                node.pointer(&format!("/status/allocatable/{key}"))
                    .and_then(Value::as_str)
                    .and_then(parse_quantity)
                    .unwrap_or(0.0)
            };
            any = true;
            gauges.cpu_used += usage.cpu;
            gauges.memory_used += usage.memory;
            gauges.cpu_allocatable += quantity("cpu");
            gauges.memory_allocatable += quantity("memory");
        }
        any.then_some(gauges)
    }

    /// Adds the current counts of the tiles that have data to their history.
    fn sample(&mut self, cx: &mut Context<Self>) {
        if let Some(g) = self.gauges(cx) {
            if let Some(p) = percent(g.cpu_used, g.cpu_allocatable) {
                self.cpu.push(p);
            }
            if let Some(p) = percent(g.memory_used, g.memory_allocatable) {
                self.memory.push(p);
            }
        }
        let counts: Vec<(Kind, Count)> = self
            .tiles
            .iter()
            .filter(|(_, t)| t.store.read(cx).status().is_ready())
            .filter_map(|(kind, _)| Some((*kind, self.current(*kind, cx)?)))
            .collect();
        for (kind, count) in counts {
            if let Some(tile) = self.tiles.get_mut(&kind) {
                tile.history.push(count);
            }
        }
        cx.notify();
    }

    fn shown(&self) -> Vec<Kind> {
        Kind::shown(self.namespace.is_some()).collect()
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.shown().len() as isize;
        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            cx.notify();
        }
    }

    fn next(&mut self, _: &NextTile, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn previous(&mut self, _: &PreviousTile, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn open_selected(&mut self, _: &OpenTile, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(kind) = self.shown().get(self.selected).copied() {
            self.open(kind, cx);
        }
    }

    /// Opens the list of a tile's kind, in the namespace of the view.
    fn open(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let Some(gvr) = self.resolve(kind, cx) else {
            return;
        };
        let namespace = self.namespace.clone().filter(|_| kind.namespaced());
        let view = ViewRegistry::list_view(cx, &gvr);
        let request = ViewRequest::for_resource(
            view,
            ResourceRef::list(self.cluster.clone(), gvr, namespace),
        );
        cx.defer(move |cx| {
            let window = cx.active_window().or_else(|| cx.windows().first().copied());
            if let Some(window) = window {
                window
                    .update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(OpenView(request)), cx)
                    })
                    .ok();
            }
        });
    }

    fn render_gauge(
        &self,
        title: &'static str,
        used: String,
        allocatable: String,
        percent: Option<f64>,
        samples: &Samples,
        cx: &App,
    ) -> gpui::AnyElement {
        let colors = cx.colors().clone();
        let color = percent.map_or(colors.text_faint, |p| usage_color(p as f32, &colors));
        v_flex()
            .id(SharedString::from(format!("pulse-{title}")))
            .w(u(300.0))
            .p(u(12.0))
            .gap(u(4.0))
            .rounded(u(8.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                div()
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(title),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .items_baseline()
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(24.0))
                            .text_color(color)
                            .child(percent.map_or("–".into(), |p| format!("{p:.0}%"))),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!("{used} of {allocatable}")),
                    ),
            )
            .child(
                Sparkline::new(samples.values(), color)
                    .height(34.0)
                    .zero_based(),
            )
            .into_any_element()
    }

    fn render_tile(&self, kind: Kind, selected: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.colors().clone();
        let (ok_label, fault_label) = kind.labels();
        let count = self.current(kind, cx);
        let tile = self.tiles.get(&kind);
        let fault = count.map_or(0, |c| c.fault);
        v_flex()
            .id(SharedString::from(format!("pulse-{}", kind.title())))
            .w(u(300.0))
            .p(u(12.0))
            .gap(u(4.0))
            .rounded(u(8.0))
            .border_1()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(ix) = this.shown().iter().position(|k| *k == kind) {
                    this.selected = ix;
                }
                this.open(kind, cx);
            }))
            .border_color(if fault > 0 {
                colors.red
            } else if selected {
                colors.accent
            } else {
                colors.border
            })
            .bg(colors.panel)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(kind.title().to_uppercase()),
                    )
                    .child(div().text_size(u(11.0)).text_color(colors.text_dim).child(
                        match count {
                            Some(c) => format!("{} total", c.total),
                            None if tile.is_none() => "not served".into(),
                            None => String::new(),
                        },
                    )),
            )
            .child(
                h_flex()
                    .gap(u(16.0))
                    .items_baseline()
                    .child(
                        h_flex()
                            .gap(u(4.0))
                            .items_baseline()
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(24.0))
                                    .text_color(colors.green)
                                    .child(count.map_or("–".into(), |c| c.ok().to_string())),
                            )
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(ok_label),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(u(4.0))
                            .items_baseline()
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(24.0))
                                    .text_color(if fault > 0 {
                                        colors.red
                                    } else {
                                        colors.text_faint
                                    })
                                    .child(count.map_or("–".into(), |c| c.fault.to_string())),
                            )
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(fault_label),
                            ),
                    ),
            )
            .children(tile.map(|t| {
                v_flex()
                    .gap(u(2.0))
                    .child(Sparkline::new(t.history.oks(), colors.green).height(34.0))
                    .child(
                        Sparkline::new(t.history.faults(), colors.red)
                            .height(20.0)
                            .zero_based(),
                    )
            }))
            .into_any_element()
    }
}

impl Focusable for PulsesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for PulsesView {
    fn tab_title(&self, _: &App) -> SharedString {
        "Pulses".into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Activity.path())
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
        Some(ViewRequest::for_resource(
            ViewKind::Custom(crate::VIEW_KIND.into()),
            crate::target(&self.cluster, self.namespace.clone()),
        ))
    }
}

impl Render for PulsesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let shown = self.shown();
        let tiles: Vec<gpui::AnyElement> = shown
            .iter()
            .enumerate()
            .map(|(ix, kind)| self.render_tile(*kind, ix == self.selected, cx))
            .collect();
        let mut tiles = tiles;
        if let Some(g) = self.gauges(cx) {
            tiles.push(self.render_gauge(
                "CPU",
                format_cpu(g.cpu_used),
                format_cpu(g.cpu_allocatable),
                percent(g.cpu_used, g.cpu_allocatable),
                &self.cpu,
                cx,
            ));
            tiles.push(self.render_gauge(
                "MEMORY",
                format_bytes(g.memory_used),
                format_bytes(g.memory_allocatable),
                percent(g.memory_used, g.memory_allocatable),
                &self.memory,
                cx,
            ));
        }
        v_flex()
            .id("pulses")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::next))
            .on_action(cx.listener(Self::previous))
            .on_action(cx.listener(Self::open_selected))
            .size_full()
            .bg(colors.background)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(sizes::TOOLBAR))
                    .px(u(12.0))
                    .gap(u(6.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .text_color(colors.text_dim)
                    .child(Icon::new(IconName::Activity).color(colors.accent))
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Pulses"),
                    )
                    .child("·")
                    .child(
                        self.namespace
                            .clone()
                            .unwrap_or_else(|| "all namespaces".into()),
                    )
                    .child(div().flex_1())
                    .child(div().text_size(u(11.5)).child("every 10 s")),
            )
            .child(
                div()
                    .id("pulses-grid")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(u(12.0))
                            .p(u(12.0))
                            .items_start()
                            .children(tiles),
                    ),
            )
    }
}
