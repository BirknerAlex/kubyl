//! The topology (board 18, README "Topology graph"): namespaces or workloads as nodes sized by
//! volume, edges by verdict (forwarded, dropped dashed, no reply) and width by volume.
//! Aggregation and layout run in the background, warm-started from the previous positions;
//! nodes are elements (click, double-click, hover), edges are painted on a canvas and hit-tested.
//! Drag pans, the wheel zooms. A click selects (the side panel explains it); a double-click or
//! "Show flows" filters the table to the node or edge.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, ClickEvent, Context, FontWeight, Hsla, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, ScrollWheelEvent,
    SharedString, Task, Window, canvas, div, point, prelude::*, px,
};
use kubyl_charts::graph::{self, GraphEdge, GraphNode, Positions, Viewport};
use kubyl_core::actions::OpenView;
use kubyl_core::{ViewKind, ViewRequest};
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{NetworkFlowsView, Tab, widgets};
use crate::aggregate::{self, NodeKind, TopoEdge, Topology, Zoom};
use crate::model::Flow;
use crate::service::{FlowService, FlowState};
use crate::settings::NetflowState;

/// Recompute a live graph at most this often.
const EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Node(String),
    /// Source and target node ids.
    Edge(String, String),
}

struct Drag {
    start: gpui::Point<Pixels>,
    pan: [f32; 2],
    moved: bool,
}

pub struct TopologyState {
    pub zoom: Zoom,
    pub graph: Option<Arc<Topology>>,
    /// Layout units, index-aligned with `graph.nodes`.
    pub positions: Vec<[f32; 2]>,
    radii: Vec<f32>,
    /// What fitting keeps in view: the nodes and the tops of their loops.
    fit: (Vec<[f32; 2]>, Vec<f32>),
    previous: Positions,
    pub selected: Option<Selection>,
    /// The user's zoom and pan on top of fitting.
    scale: f32,
    pan: [f32; 2],
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    drag: Option<Drag>,
    /// What the graph was computed for, and when.
    key: Option<String>,
    computed: Option<Instant>,
    computing: bool,
    generation: u64,
    task: Option<Task<()>>,
}

impl TopologyState {
    pub fn new(zoom: Zoom) -> Self {
        Self {
            zoom,
            graph: None,
            positions: Vec::new(),
            radii: Vec::new(),
            fit: (Vec::new(), Vec::new()),
            previous: Positions::new(),
            selected: None,
            scale: 1.0,
            pan: [0.0, 0.0],
            bounds: Rc::new(Cell::new(None)),
            drag: None,
            key: None,
            computed: None,
            computing: false,
            generation: 0,
            task: None,
        }
    }

    /// `6 · 9` (nodes · edges), for the tab.
    pub fn summary(&self) -> Option<String> {
        let graph = self.graph.as_ref()?;
        Some(format!("{} · {}", graph.nodes.len(), graph.edges.len()))
    }

    fn fit(&mut self) {
        self.scale = 1.0;
        self.pan = [0.0, 0.0];
    }

    /// Layout → view coordinates (inside the canvas).
    fn viewport(&self) -> Option<(Viewport, [f32; 2])> {
        let bounds = self.bounds.get()?;
        let size = [f32::from(bounds.size.width), f32::from(bounds.size.height)];
        let (points, radii) = if self.fit.0.len() >= self.positions.len() && !self.fit.0.is_empty()
        {
            (&self.fit.0, &self.fit.1)
        } else {
            (&self.positions, &self.radii)
        };
        let base = graph::fit(points, radii, size, 40.0, 1.6);
        let center = [size[0] / 2.0, size[1] / 2.0];
        let scale = base.scale * self.scale;
        let offset = [
            (base.offset[0] - center[0]) * self.scale + center[0] + self.pan[0],
            (base.offset[1] - center[1]) * self.scale + center[1] + self.pan[1],
        ];
        Some((Viewport { scale, offset }, size))
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.graph.as_ref()?.nodes.iter().position(|n| n.id == id)
    }
}

/// A node's radius (layout units) by volume.
fn radius(volume: u64, max: u64, zoom: Zoom) -> f32 {
    let share = if max == 0 {
        0.0
    } else {
        (volume as f32 / max as f32).sqrt()
    };
    match zoom {
        Zoom::Namespaces => 16.0 + 26.0 * share,
        Zoom::Workloads => 11.0 + 18.0 * share,
    }
}

fn volume(topology: &Topology, flows: u64, bytes: u64) -> u64 {
    if topology.bytes_only { bytes } else { flows }
}

/// Aggregates and lays out off the UI thread.
fn compute(
    topology: Topology,
    zoom: Zoom,
    previous: Positions,
) -> (Topology, Vec<[f32; 2]>, Vec<f32>) {
    let max = topology
        .nodes
        .iter()
        .map(|n| volume(&topology, n.flows, n.bytes))
        .max()
        .unwrap_or(0);
    let radii: Vec<f32> = topology
        .nodes
        .iter()
        .map(|n| radius(volume(&topology, n.flows, n.bytes), max, zoom))
        .collect();
    let max_edge = topology
        .edges
        .iter()
        .map(|e| volume(&topology, e.flows, e.bytes))
        .max()
        .unwrap_or(1)
        .max(1);
    let nodes: Vec<GraphNode> = topology
        .nodes
        .iter()
        .zip(&radii)
        .map(|(n, r)| GraphNode {
            id: n.id.clone(),
            radius: *r,
        })
        .collect();
    let edges: Vec<GraphEdge> = topology
        .edges
        .iter()
        .map(|e| GraphEdge {
            source: e.source,
            target: e.target,
            weight: ((1 + volume(&topology, e.flows, e.bytes)) as f32).ln()
                / ((1 + max_edge) as f32).ln(),
        })
        .collect();
    // At workload zoom, each namespace's workloads are laid out together (their box).
    let positions = match zoom {
        Zoom::Namespaces => graph::layout(&nodes, &edges, &previous),
        Zoom::Workloads => {
            let groups: Vec<Option<String>> =
                topology.nodes.iter().map(|n| n.namespace.clone()).collect();
            graph::layout_grouped(&nodes, &edges, &groups, &previous)
        }
    };
    (topology, positions, radii)
}

impl NetworkFlowsView {
    /// Recomputes the graph when its inputs changed (at most once a second while live).
    fn refresh_topology(&mut self, state: &FlowState, cx: &mut Context<Self>) {
        let FlowState::Ready { capabilities, .. } = state else {
            return;
        };
        let zoom = self.topology.zoom;
        let filter = self.effective_filter();
        let window = self.window.duration();
        if !capabilities.single_flows && capabilities.graph_from_metrics {
            // NetObserv without Loki: the backend's metrics.
            let Some(service) = FlowService::global(cx) else {
                return;
            };
            let cluster = self.cluster.clone();
            let graph = service.update(cx, |s, cx| {
                s.metrics_graph(&cluster, zoom, window, &filter, cx)
            });
            let Some(topology) = graph.and_then(|g| g.topology) else {
                return;
            };
            let key = format!("metrics|{:p}", Arc::as_ptr(&topology));
            if self.topology.key.as_deref() != Some(&key) && !self.topology.computing {
                self.start_layout((*topology).clone(), key, cx);
            }
            return;
        }
        // Paused, the graph holds still too: what arrived since waits with the table's rows.
        let waiting = if self.paused {
            0
        } else {
            self.rows.pending.len()
        };
        let newest = if self.paused {
            None
        } else {
            self.rows.pending.front()
        }
        .or(self.rows.shown.front())
        .copied()
        .unwrap_or(0);
        let revision = newest ^ ((self.rows.shown.len() + waiting) as u64) << 32;
        let key = format!(
            "{}|{}|{}|{revision}",
            zoom.key(),
            filter.canonical(),
            window.as_secs()
        );
        let base = format!("{}|{}|{}", zoom.key(), filter.canonical(), window.as_secs());
        if self.topology.key.as_deref() == Some(&key) || self.topology.computing {
            return;
        }
        let same_inputs = self
            .topology
            .key
            .as_deref()
            .is_some_and(|k| k.starts_with(&base));
        if same_inputs && self.topology.computed.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        let Some(service) = FlowService::global(cx) else {
            return;
        };
        let flows: Vec<Arc<Flow>> = {
            let service = service.read(cx);
            let Some(stream) = service.stream(&self.cluster, &self.pushed) else {
                return;
            };
            self.rows
                .pending
                .iter()
                .take(waiting)
                .chain(self.rows.shown.iter())
                .filter_map(|seq| stream.buffer.get(*seq).cloned())
                .collect()
        };
        self.topology.computing = true;
        self.topology.generation += 1;
        let generation = self.topology.generation;
        let previous = self.topology.previous.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let topology = aggregate::aggregate(
                        flows.iter().map(|f| f.as_ref()),
                        zoom,
                        aggregate::NODE_LIMIT,
                    );
                    compute(topology, zoom, previous)
                })
                .await;
            this.update(cx, |this, cx| {
                this.finish_layout(generation, key, result, cx)
            })
            .ok();
        });
        self.topology.task = Some(task);
    }

    fn start_layout(&mut self, topology: Topology, key: String, cx: &mut Context<Self>) {
        self.topology.computing = true;
        self.topology.generation += 1;
        let generation = self.topology.generation;
        let zoom = self.topology.zoom;
        let previous = self.topology.previous.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { compute(topology, zoom, previous) })
                .await;
            this.update(cx, |this, cx| {
                this.finish_layout(generation, key, result, cx)
            })
            .ok();
        });
        self.topology.task = Some(task);
    }

    fn finish_layout(
        &mut self,
        generation: u64,
        key: String,
        (topology, positions, radii): (Topology, Vec<[f32; 2]>, Vec<f32>),
        cx: &mut Context<Self>,
    ) {
        if self.topology.generation != generation {
            return;
        }
        self.topology.computing = false;
        self.topology.computed = Some(Instant::now());
        self.topology.key = Some(key);
        self.topology.previous = topology
            .nodes
            .iter()
            .zip(&positions)
            .map(|(n, p)| (n.id.clone(), *p))
            .collect();
        // Loops and their labels need room when fitting: their tops join the nodes.
        let center = middle(&positions);
        let mut fit_points = positions.clone();
        let mut fit_radii = radii.clone();
        for edge in topology.edges.iter().filter(|e| e.source == e.target) {
            let (Some(&p), Some(&r)) = (positions.get(edge.source), radii.get(edge.source)) else {
                continue;
            };
            let out = loop_angle(p, center);
            let reach = r * LOOP_REACH * 0.75 + LOOP_EXTRA;
            fit_points.push([p[0] + reach * out.cos(), p[1] + reach * out.sin()]);
            fit_radii.push(26.0);
        }
        self.topology.fit = (fit_points, fit_radii);
        self.topology.positions = positions;
        self.topology.radii = radii;
        self.topology.graph = Some(Arc::new(topology));
        cx.notify();
    }

    pub(crate) fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        if self.topology.zoom == zoom {
            return;
        }
        self.topology.zoom = zoom;
        self.topology.key = None;
        self.topology.selected = None;
        self.topology.fit();
        kubyl_settings::State::update::<NetflowState>(cx, |s| s.zoom = zoom.key().into());
        cx.notify();
    }

    pub(crate) fn fit_graph(&mut self, cx: &mut Context<Self>) {
        self.topology.fit();
        cx.notify();
    }

    /// Filters the table to a node or edge and shows it.
    pub(crate) fn show_selection_flows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(graph) = self.topology.graph.clone() else {
            return;
        };
        let term = match &self.topology.selected {
            Some(Selection::Node(id)) => graph
                .nodes
                .iter()
                .find(|n| &n.id == id)
                .map(|n| n.filter.clone()),
            Some(Selection::Edge(a, b)) => {
                let side = |id: &str, prefix: &str| {
                    graph
                        .nodes
                        .iter()
                        .find(|n| n.id == id)
                        .map(|n| format!("{prefix}{}", n.filter))
                };
                match (side(a, "src."), side(b, "dst.")) {
                    (Some(a), Some(b)) => Some(format!("{a} {b}")),
                    _ => None,
                }
            }
            None => None,
        };
        if let Some(term) = term {
            self.add_term(&term, cx);
            self.set_tab(Tab::Flows, window, cx);
        }
    }

    fn select(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        self.topology.selected = selection;
        cx.notify();
    }

    /// Selects the next (`delta` 1) or previous (-1) node, busiest first, or connection,
    /// blocked first.
    pub(crate) fn step_graph(&mut self, edges: bool, delta: isize, cx: &mut Context<Self>) {
        let Some(graph) = self.topology.graph.clone() else {
            return;
        };
        let order: Vec<Selection> = if edges {
            let mut list: Vec<&TopoEdge> = graph.edges.iter().collect();
            list.sort_by(|a, b| {
                b.blocked()
                    .cmp(&a.blocked())
                    .then(b.flows.cmp(&a.flows))
                    .then(b.bytes.cmp(&a.bytes))
            });
            list.iter()
                .map(|e| {
                    Selection::Edge(
                        graph.nodes[e.source].id.clone(),
                        graph.nodes[e.target].id.clone(),
                    )
                })
                .collect()
        } else {
            let mut list: Vec<&aggregate::TopoNode> = graph.nodes.iter().collect();
            list.sort_by(|a, b| {
                b.flows
                    .cmp(&a.flows)
                    .then(b.bytes.cmp(&a.bytes))
                    .then(a.id.cmp(&b.id))
            });
            list.iter().map(|n| Selection::Node(n.id.clone())).collect()
        };
        if order.is_empty() {
            return;
        }
        let len = order.len() as isize;
        let next = match order
            .iter()
            .position(|s| Some(s) == self.topology.selected.as_ref())
        {
            Some(at) => (at as isize + delta).rem_euclid(len),
            None if delta < 0 => len - 1,
            None => 0,
        };
        self.select(Some(order[next as usize].clone()), cx);
    }

    /// The edge under a point (view coordinates), if any.
    fn edge_at(&self, at: [f32; 2]) -> Option<Selection> {
        let graph = self.topology.graph.as_ref()?;
        let (viewport, _) = self.topology.viewport()?;
        let mut best: Option<(f32, usize)> = None;
        let pairs = edge_pairs(graph);
        for (i, edge) in graph.edges.iter().enumerate() {
            for curve in curves(graph, &pairs, edge) {
                let (from, ctrl, to) = geometry(&self.topology, &viewport, edge, curve.bend)?;
                for step in 0..=20 {
                    let t = step as f32 / 20.0;
                    let x =
                        (1.0 - t).powi(2) * from[0] + 2.0 * (1.0 - t) * t * ctrl[0] + t * t * to[0];
                    let y =
                        (1.0 - t).powi(2) * from[1] + 2.0 * (1.0 - t) * t * ctrl[1] + t * t * to[1];
                    let d = ((x - at[0]).powi(2) + (y - at[1]).powi(2)).sqrt();
                    if d < 7.0 && best.is_none_or(|(b, _)| d < b) {
                        best = Some((d, i));
                    }
                }
            }
        }
        let (_, i) = best?;
        let edge = &graph.edges[i];
        Some(Selection::Edge(
            graph.nodes[edge.source].id.clone(),
            graph.nodes[edge.target].id.clone(),
        ))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CurveKind {
    Forwarded,
    Dropped,
    NoReply,
}

struct Curve {
    kind: CurveKind,
    volume: u64,
    bend: f32,
}

/// The (source, target) pairs of a graph's edges, to find edges with a reverse in O(1): the
/// paint, labels and hit tests go through every edge.
fn edge_pairs(graph: &Topology) -> std::collections::HashSet<(usize, usize)> {
    graph.edges.iter().map(|e| (e.source, e.target)).collect()
}

/// The strokes of an edge: forwarded, dropped and unanswered traffic bend apart.
fn curves(
    graph: &Topology,
    pairs: &std::collections::HashSet<(usize, usize)>,
    edge: &TopoEdge,
) -> Vec<Curve> {
    let reverse = pairs.contains(&(edge.target, edge.source));
    let base = if reverse { 14.0 } else { 0.0 };
    let share = |count: u64| -> u64 {
        if graph.bytes_only || edge.flows == 0 {
            edge.bytes
        } else {
            count
        }
    };
    let mut out = Vec::new();
    if edge.forwarded > 0 {
        out.push(Curve {
            kind: CurveKind::Forwarded,
            volume: share(edge.forwarded),
            bend: base,
        });
    }
    if edge.dropped > 0 {
        out.push(Curve {
            kind: CurveKind::Dropped,
            volume: share(edge.dropped),
            bend: base + if edge.forwarded > 0 { 22.0 } else { 0.0 },
        });
    }
    if edge.no_reply > 0 {
        out.push(Curve {
            kind: CurveKind::NoReply,
            volume: share(edge.no_reply),
            bend: base
                + if edge.forwarded > 0 || edge.dropped > 0 {
                    -22.0
                } else {
                    0.0
                },
        });
    }
    out
}

/// How far a loop's control point reaches: this many radii plus a constant (view pixels).
const LOOP_REACH: f32 = 1.9;
const LOOP_EXTRA: f32 = 24.0;

/// The middle of a layout.
fn middle(positions: &[[f32; 2]]) -> [f32; 2] {
    let count = positions.len().max(1) as f32;
    positions.iter().fold([0.0, 0.0], |acc, p| {
        [acc[0] + p[0] / count, acc[1] + p[1] / count]
    })
}

/// Where a node's loop points: away from the middle of the graph, where the other nodes are,
/// and never into the label under the node. The same in layout and view coordinates.
fn loop_angle(node: [f32; 2], middle: [f32; 2]) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let (dx, dy) = (node[0] - middle[0], node[1] - middle[1]);
    let out = if dx.abs() + dy.abs() < 1.0 {
        -FRAC_PI_2
    } else {
        dy.atan2(dx)
    };
    let off = (out - FRAC_PI_2 + PI).rem_euclid(TAU) - PI;
    if off.abs() < 1.3 {
        FRAC_PI_2 + 1.3 * off.signum()
    } else {
        out
    }
}

/// Start, control and end point of a curve, from the source's rim to the target's. Traffic
/// inside a node (a namespace talking to itself) loops above it.
fn geometry(
    state: &TopologyState,
    viewport: &Viewport,
    edge: &TopoEdge,
    bend: f32,
) -> Option<([f32; 2], [f32; 2], [f32; 2])> {
    let a = viewport.apply(*state.positions.get(edge.source)?);
    let b = viewport.apply(*state.positions.get(edge.target)?);
    let ra = state.radii.get(edge.source).copied().unwrap_or(10.0) * viewport.scale;
    let r = state.radii.get(edge.target).copied().unwrap_or(10.0) * viewport.scale;
    if edge.source == edge.target {
        let middle = viewport.apply(middle(&state.positions));
        let out = loop_angle(a, middle);
        let rim = |angle: f32, extra: f32| {
            [
                a[0] + (r + extra) * angle.cos(),
                a[1] + (r + extra) * angle.sin(),
            ]
        };
        let from = rim(out - 0.63, 0.0);
        // Arrow heads sit a little off the rim, as on other edges.
        let to = rim(out + 0.63, 3.0);
        let reach = r * LOOP_REACH + LOOP_EXTRA + bend.abs();
        let ctrl = [a[0] + reach * out.cos(), a[1] + reach * out.sin()];
        return Some((from, ctrl, to));
    }
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length = (dx * dx + dy * dy).sqrt().max(1.0);
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let ctrl = [mid[0] - dy / length * bend, mid[1] + dx / length * bend];
    let (sx, sy) = (ctrl[0] - a[0], ctrl[1] - a[1]);
    let sl = (sx * sx + sy * sy).sqrt().max(1.0);
    let start = [a[0] + sx / sl * ra, a[1] + sy / sl * ra];
    let (tx, ty) = (b[0] - ctrl[0], b[1] - ctrl[1]);
    let tl = (tx * tx + ty * ty).sqrt().max(1.0);
    let end = [b[0] - tx / tl * (r + 3.0), b[1] - ty / tl * (r + 3.0)];
    Some((start, ctrl, end))
}

fn node_color(
    node: &aggregate::TopoNode,
    colors: &Colors,
    palette: &HashMap<String, Hsla>,
) -> Hsla {
    match node.kind {
        NodeKind::Namespace | NodeKind::Workload | NodeKind::Folded => node
            .namespace
            .as_ref()
            .and_then(|ns| palette.get(ns).copied())
            .unwrap_or(colors.accent),
        NodeKind::World => colors.text_dim,
        _ => colors.text_faint,
    }
}

/// `1.2k`.
fn compact(value: u64) -> String {
    match value {
        v if v >= 1_000_000 => format!("{:.1}M", v as f64 / 1e6),
        v if v >= 10_000 => format!("{:.0}k", v as f64 / 1e3),
        v if v >= 1_000 => format!("{:.1}k", v as f64 / 1e3),
        v => v.to_string(),
    }
}

pub(super) fn render(
    view: &mut NetworkFlowsView,
    state: &FlowState,
    window: &mut Window,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let _ = window;
    view.refresh_topology(state, cx);
    let colors = cx.colors().clone();
    let notice = match state {
        FlowState::Ready { capabilities, .. } => {
            super::table::no_single_flows(state, capabilities, &colors)
        }
        _ => None,
    };
    let toolbar = toolbar(view, &colors, cx);
    let canvas = graph_canvas(view, &colors, cx);
    let panel = side_panel(view, &colors, cx);
    v_flex()
        .size_full()
        .children(notice)
        .child(toolbar)
        .child(
            div()
                .key_context(super::GRAPH_CONTEXT)
                .track_focus(&view.graph_focus)
                .flex()
                .flex_1()
                .min_h_0()
                .child(canvas)
                .children(panel),
        )
        .into_any_element()
}

fn toolbar(
    view: &NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let segment = |zoom: Zoom, label: &'static str, cx: &mut Context<NetworkFlowsView>| {
        let on = view.topology.zoom == zoom;
        div()
            .id(label)
            .px(u(10.0))
            .py(u(3.0))
            .map(|this| {
                if on {
                    this.bg(colors.chip_selected_background)
                        .text_color(colors.chip_selected_text)
                } else {
                    let hover = colors.hover;
                    this.text_color(colors.text_dim)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                }
            })
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| this.set_zoom(zoom, cx)))
    };
    let summary = match &view.topology.graph {
        Some(graph) => {
            let kind = match view.topology.zoom {
                Zoom::Namespaces => ("namespace", "namespaces"),
                Zoom::Workloads => ("workload", "workloads"),
            };
            let named = graph
                .nodes
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::Namespace | NodeKind::Workload))
                .count();
            let mut text = format!(
                "{} · {}",
                widgets::plural(named, kind.0, kind.1),
                widgets::plural(graph.edges.len(), "edge", "edges")
            );
            if graph.folded > 0 {
                text.push_str(&format!(" · {} folded", widgets::count(graph.folded)));
            }
            if graph.bytes_only {
                text.push_str(" · bytes from metrics");
            }
            text
        }
        None if view.topology.computing => "Laying out…".into(),
        None => String::new(),
    };
    let legend = |color: Hsla, dashed: bool, label: &'static str| {
        h_flex()
            .gap(u(5.0))
            .text_size(u(11.5))
            .text_color(colors.text_muted)
            .child(if dashed {
                div()
                    .w(u(14.0))
                    .h(u(0.0))
                    .border_t_2()
                    .border_color(color)
                    .border_dashed()
            } else {
                div().w(u(14.0)).h(u(3.0)).bg(color)
            })
            .child(label)
    };
    let fit = cx.listener(|this, _: &ClickEvent, _, cx| this.fit_graph(cx));
    h_flex()
        .flex_none()
        .h(u(38.0))
        .px(u(12.0))
        .gap(u(10.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(
            h_flex()
                .flex_none()
                .border_1()
                .border_color(colors.border)
                .rounded(u(5.0))
                .overflow_hidden()
                .text_size(u(12.0))
                .child(segment(Zoom::Namespaces, "Namespaces", cx))
                .child(segment(Zoom::Workloads, "Workloads", cx)),
        )
        .child(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(summary),
        )
        .child(div().flex_1())
        .child(legend(colors.green.opacity(0.7), false, "forwarded"))
        .child(legend(colors.red, true, "dropped"))
        .child(legend(colors.yellow, false, "no reply"))
        .child(
            div()
                .text_size(u(11.5))
                .text_color(colors.text_dim)
                .child("width: volume"),
        )
        .child(
            h_flex()
                .id("graph-fit")
                .gap(u(4.0))
                .px(u(6.0))
                .h(u(24.0))
                .rounded(u(5.0))
                .cursor_pointer()
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .hover(|s| s.bg(colors.hover))
                .child(Icon::new(IconName::Maximize).size(12.0))
                .child("Fit")
                .on_click(fit),
        )
        .into_any_element()
}

fn graph_canvas(
    view: &mut NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> AnyElement {
    let bounds_cell = view.topology.bounds.clone();
    let graph = view.topology.graph.clone();
    let viewport = view.topology.viewport();
    let names: Vec<SharedString> = graph
        .as_ref()
        .map(|g| {
            let mut ns: Vec<SharedString> = g
                .nodes
                .iter()
                .filter_map(|n| n.namespace.clone().map(SharedString::from))
                .collect();
            ns.sort();
            ns.dedup();
            ns
        })
        .unwrap_or_default();
    let slots =
        kubyl_charts::ColorRegistry::assign(cx, &format!("{}/namespace", view.cluster), &names);
    let palette: HashMap<String, Hsla> = names
        .iter()
        .zip(slots)
        .map(|(n, slot)| (n.to_string(), kubyl_charts::series_color(slot, colors)))
        .collect();
    let empty = graph.as_ref().is_none_or(|g| g.nodes.is_empty());
    let mut layer = div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .bg(colors.background);

    // Workloads sit in a box per namespace, behind the edges.
    if let (Some(graph), Some((viewport, _))) = (graph.as_ref(), viewport)
        && view.topology.zoom == Zoom::Workloads
    {
        let mut boxes: Vec<(String, [f32; 4])> = Vec::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            let (Some(ns), Some(position)) =
                (node.namespace.as_ref(), view.topology.positions.get(i))
            else {
                continue;
            };
            let [x, y] = viewport.apply(*position);
            let r = view.topology.radii.get(i).copied().unwrap_or(12.0) * viewport.scale;
            // Room for the label under the circle and the namespace's name above (as wide as
            // the name needs).
            let half = (r + 36.0).max(ns.len() as f32 * 3.6 + 14.0);
            let area = [x - half, y - r - 30.0, x + half, y + r + 30.0];
            match boxes.iter_mut().find(|(name, _)| name == ns) {
                Some((_, b)) => {
                    *b = [
                        b[0].min(area[0]),
                        b[1].min(area[1]),
                        b[2].max(area[2]),
                        b[3].max(area[3]),
                    ]
                }
                None => boxes.push((ns.clone(), area)),
            }
        }
        for (ns, [x0, y0, x1, y1]) in boxes {
            let color = palette.get(&ns).copied().unwrap_or(colors.accent);
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(x0))
                    .top(px(y0))
                    .w(px(x1 - x0))
                    .h(px(y1 - y0))
                    .rounded(u(10.0))
                    .border_1()
                    .border_dashed()
                    .border_color(colors.text_faint.opacity(0.45))
                    .bg(color.opacity(0.04))
                    .child(
                        div()
                            .absolute()
                            .left(u(10.0))
                            .top(u(6.0))
                            .whitespace_nowrap()
                            .text_size(u(11.5))
                            .text_color(color)
                            .child(ns),
                    ),
            );
        }
    }

    // Edges.
    let edge_state = {
        let graph = graph.clone();
        let positions = view.topology.positions.clone();
        let radii = view.topology.radii.clone();
        let fit = view.topology.fit.clone();
        let selected = view.topology.selected.clone();
        let colors = colors.clone();
        (graph, positions, radii, fit, selected, colors)
    };
    let scale_cell = view.topology.scale;
    let pan_cell = view.topology.pan;
    layer = layer.child(
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let changed = bounds_cell.get() != Some(bounds);
                bounds_cell.set(Some(bounds));
                if changed {
                    window.refresh();
                }
                let (graph, positions, radii, fit, selected, colors) = &edge_state;
                let Some(graph) = graph else {
                    return;
                };
                let state = TopologyState {
                    positions: positions.clone(),
                    radii: radii.clone(),
                    fit: fit.clone(),
                    bounds: Rc::new(Cell::new(Some(bounds))),
                    scale: scale_cell,
                    pan: pan_cell,
                    ..TopologyState::new(Zoom::Namespaces)
                };
                let Some((viewport, _)) = state.viewport() else {
                    return;
                };
                let max = graph
                    .edges
                    .iter()
                    .map(|e| if graph.bytes_only { e.bytes } else { e.flows })
                    .max()
                    .unwrap_or(1)
                    .max(1);
                let at = |p: [f32; 2]| point(bounds.origin.x + px(p[0]), bounds.origin.y + px(p[1]));
                let pairs = edge_pairs(graph);
                for edge in &graph.edges {
                    let is_selected = matches!(selected, Some(Selection::Edge(a, b)) if graph.nodes[edge.source].id == *a && graph.nodes[edge.target].id == *b);
                    for curve in curves(graph, &pairs, edge) {
                        let Some((from, ctrl, to)) = geometry(&state, &viewport, edge, curve.bend) else {
                            continue;
                        };
                        let share = ((1 + curve.volume) as f32).ln() / ((1 + max) as f32).ln();
                        let width = 1.25 + 4.5 * share;
                        let color = match curve.kind {
                            CurveKind::Forwarded => colors.green.opacity(0.55),
                            CurveKind::Dropped => colors.red,
                            CurveKind::NoReply => colors.yellow,
                        };
                        if is_selected {
                            let mut glow = PathBuilder::stroke(px(width + 7.0));
                            glow.move_to(at(from));
                            glow.curve_to(at(to), at(ctrl));
                            if let Ok(path) = glow.build() {
                                window.paint_path(path, colors.accent.opacity(0.35));
                            }
                        }
                        let mut path = PathBuilder::stroke(px(width));
                        if curve.kind == CurveKind::Dropped {
                            path = path.dash_array(&[px(6.0), px(4.0)]);
                        }
                        path.move_to(at(from));
                        path.curve_to(at(to), at(ctrl));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color);
                        }
                        // The arrow head.
                        let angle = (to[1] - ctrl[1]).atan2(to[0] - ctrl[0]);
                        let head = 6.0 + width;
                        let mut arrow = PathBuilder::fill();
                        arrow.move_to(at(to));
                        arrow.line_to(at([to[0] - head * (angle - 0.4).cos(), to[1] - head * (angle - 0.4).sin()]));
                        arrow.line_to(at([to[0] - head * (angle + 0.4).cos(), to[1] - head * (angle + 0.4).sin()]));
                        arrow.close();
                        if let Ok(path) = arrow.build() {
                            window.paint_path(path, color);
                        }
                    }
                }
                let _ = radii;
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    );

    // Nodes and labels (elements: clicks and hovers for free).
    if let (Some(graph), Some((viewport, _))) = (graph.as_ref(), viewport) {
        for (i, node) in graph.nodes.iter().enumerate() {
            let Some(position) = view.topology.positions.get(i) else {
                continue;
            };
            let [x, y] = viewport.apply(*position);
            let r = view.topology.radii.get(i).copied().unwrap_or(12.0) * viewport.scale;
            let color = node_color(node, colors, &palette);
            let selected = view.topology.selected == Some(Selection::Node(node.id.clone()));
            let id = node.id.clone();
            let icon = match node.kind {
                NodeKind::World => Some(IconName::Globe),
                NodeKind::Host | NodeKind::RemoteNode | NodeKind::KubeApiServer => {
                    Some(IconName::Server)
                }
                _ => None,
            };
            let inner = if graph.bytes_only {
                widgets::bytes(node.bytes)
            } else {
                compact(node.flows)
            };
            let blocked = node.blocked > 0;
            let tooltip: SharedString = format!(
                "{}\n{}{}",
                node.label,
                if graph.bytes_only {
                    widgets::bytes(node.bytes)
                } else {
                    widgets::plural(node.flows as usize, "flow", "flows")
                },
                if blocked {
                    format!(" · {} blocked", widgets::count(node.blocked as usize))
                } else {
                    String::new()
                }
            )
            .into();
            let click_id = id.clone();
            layer = layer.child(
                div()
                    .id(SharedString::from(format!("graph-node-{id}")))
                    .absolute()
                    .left(px(x - r))
                    .top(px(y - r))
                    .size(px(r * 2.0))
                    .rounded_full()
                    .border_2()
                    .border_color(if selected { colors.accent } else { color })
                    .bg(color.opacity(if icon.is_some() { 0.35 } else { 0.2 }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(selected, |this| this.shadow_lg())
                    .when_some(icon, |this, icon| {
                        this.child(
                            Icon::new(icon)
                                .size((r * 0.9).clamp(10.0, 18.0))
                                .color(colors.text),
                        )
                    })
                    .when(icon.is_none() && r > 16.0, |this| {
                        this.child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(px(10.5))
                                .text_color(colors.text)
                                .child(inner),
                        )
                    })
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.graph_focus.focus(window, cx);
                        this.select(Some(Selection::Node(click_id.clone())), cx);
                        if event.click_count() == 2 {
                            this.show_selection_flows(window, cx);
                        }
                    }))
                    .tooltip(move |window, cx| {
                        gpui_component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                    }),
            );
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(x - 90.0))
                    .top(px(y + r + 3.0))
                    .w(px(180.0))
                    .flex()
                    .justify_center()
                    .text_size(u(11.5))
                    .text_color(if selected {
                        colors.text
                    } else {
                        colors.text_muted
                    })
                    .when(selected, |this| this.font_weight(FontWeight::SEMIBOLD))
                    .child(div().max_w_full().truncate().child(node.label.clone())),
            );
        }
        // Blocked edges say what blocked them.
        let pairs = edge_pairs(graph);
        for edge in &graph.edges {
            if edge.blocked() == 0 {
                continue;
            }
            let Some((from, ctrl, to)) = geometry(
                &view.topology,
                &viewport,
                edge,
                curves(graph, &pairs, edge)
                    .iter()
                    .find(|c| c.kind != CurveKind::Forwarded)
                    .map_or(0.0, |c| c.bend),
            ) else {
                continue;
            };
            let mut mid = [
                0.25 * from[0] + 0.5 * ctrl[0] + 0.25 * to[0],
                0.25 * from[1] + 0.5 * ctrl[1] + 0.25 * to[1],
            ];
            // A loop's label sits beyond its top.
            if edge.source == edge.target
                && let Some(center) = view
                    .topology
                    .positions
                    .get(edge.source)
                    .map(|p| viewport.apply(*p))
            {
                let (dx, dy) = (mid[0] - center[0], mid[1] - center[1]);
                let length = (dx * dx + dy * dy).sqrt().max(1.0);
                mid = [
                    mid[0] + dx / length * 16.0,
                    mid[1] + dy / length * 16.0 + 8.0,
                ];
            }
            let label = if edge.dropped > 0 {
                format!("{} dropped", compact(edge.dropped))
            } else {
                format!("{} no reply", compact(edge.no_reply))
            };
            layer = layer.child(
                div()
                    .absolute()
                    .left(px(mid[0] - 60.0))
                    .top(px(mid[1] - 16.0))
                    .w(px(120.0))
                    .flex()
                    .justify_center()
                    .text_size(u(11.0))
                    .text_color(if edge.dropped > 0 {
                        colors.red
                    } else {
                        colors.yellow
                    })
                    .child(label),
            );
        }
    }
    if empty {
        let message = if view.topology.computing || view.scanning {
            "Laying out…"
        } else {
            "No traffic in the window."
        };
        layer = layer.child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_dim)
                .text_size(u(12.5))
                .child(message),
        );
    }
    // Zoom buttons.
    let zoom_button =
        |id: &'static str, icon: IconName, factor: f32, cx: &mut Context<NetworkFlowsView>| {
            div()
                .id(id)
                .size(u(26.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(u(5.0))
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .cursor_pointer()
                .child(Icon::new(icon).size(13.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.topology.scale = (this.topology.scale * factor).clamp(0.2, 5.0);
                    cx.notify();
                }))
        };
    layer = layer.child(
        v_flex()
            .absolute()
            .right(u(12.0))
            .bottom(u(10.0))
            .gap(u(2.0))
            .child(zoom_button("graph-zoom-in", IconName::Plus, 1.25, cx))
            .child(zoom_button("graph-zoom-out", IconName::Minus, 0.8, cx)),
    );
    layer
        .id("graph-layer")
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                this.topology.drag = Some(Drag {
                    start: event.position,
                    pan: this.topology.pan,
                    moved: false,
                });
                cx.notify();
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
            let Some(drag) = &mut this.topology.drag else {
                return;
            };
            if event.pressed_button != Some(MouseButton::Left) {
                this.topology.drag = None;
                return;
            }
            let dx = f32::from(event.position.x - drag.start.x);
            let dy = f32::from(event.position.y - drag.start.y);
            if dx.abs() + dy.abs() > 3.0 {
                drag.moved = true;
            }
            if drag.moved {
                this.topology.pan = [drag.pan[0] + dx, drag.pan[1] + dy];
                cx.notify();
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, event: &MouseUpEvent, window, cx| {
                let Some(drag) = this.topology.drag.take() else {
                    return;
                };
                if drag.moved {
                    return;
                }
                // A click on the background: an edge, or nothing.
                let Some(bounds) = this.topology.bounds.get() else {
                    return;
                };
                let at = [
                    f32::from(event.position.x - bounds.origin.x),
                    f32::from(event.position.y - bounds.origin.y),
                ];
                let edge = this.edge_at(at);
                this.graph_focus.focus(window, cx);
                let double = event.click_count == 2;
                this.select(edge, cx);
                if double && this.topology.selected.is_some() {
                    this.show_selection_flows(window, cx);
                }
            }),
        )
        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
            let Some(bounds) = this.topology.bounds.get() else {
                return;
            };
            let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
            let factor = (1.0 + delta / 400.0).clamp(0.8, 1.25);
            let old = this.topology.scale;
            let new = (old * factor).clamp(0.2, 5.0);
            // Keep the point under the cursor in place.
            let cursor = [
                f32::from(event.position.x - bounds.origin.x) - f32::from(bounds.size.width) / 2.0,
                f32::from(event.position.y - bounds.origin.y) - f32::from(bounds.size.height) / 2.0,
            ];
            let ratio = new / old;
            this.topology.pan = [
                cursor[0] - (cursor[0] - this.topology.pan[0]) * ratio,
                cursor[1] - (cursor[1] - this.topology.pan[1]) * ratio,
            ];
            this.topology.scale = new;
            cx.notify();
        }))
        .into_any_element()
}

fn side_panel(
    view: &NetworkFlowsView,
    colors: &Colors,
    cx: &mut Context<NetworkFlowsView>,
) -> Option<AnyElement> {
    let graph = view.topology.graph.as_ref()?;
    let selection = view.topology.selected.as_ref()?;
    let weak = cx.entity().downgrade();
    let show = {
        let weak = weak.clone();
        widgets::button(
            "graph-show-flows",
            Some(IconName::List),
            "Show flows",
            true,
            colors,
            move |_, window, cx| {
                weak.update(cx, |this, cx| this.show_selection_flows(window, cx))
                    .ok();
            },
        )
    };
    let volume = |flows: u64, bytes: u64| {
        if graph.bytes_only {
            widgets::bytes(bytes)
        } else {
            widgets::plural(flows as usize, "flow", "flows")
        }
    };
    let (title, heading, sections): (&str, AnyElement, Vec<AnyElement>) = match selection {
        Selection::Node(id) => {
            let i = view.topology.index_of(id)?;
            let node = &graph.nodes[i];
            let incoming: Vec<&TopoEdge> = graph
                .edges
                .iter()
                .filter(|e| e.target == i && e.source != i)
                .collect();
            let outgoing: Vec<&TopoEdge> = graph
                .edges
                .iter()
                .filter(|e| e.source == i && e.target != i)
                .collect();
            let inside = graph.edges.iter().find(|e| e.source == i && e.target == i);
            let sum = |edges: &[&TopoEdge]| {
                (
                    edges.iter().map(|e| e.flows).sum::<u64>(),
                    edges.iter().map(|e| e.bytes).sum::<u64>(),
                )
            };
            let (in_flows, in_bytes) = sum(&incoming);
            let (out_flows, out_bytes) = sum(&outgoing);
            let mut rows: Vec<(SharedString, AnyElement)> = vec![
                (
                    "In".into(),
                    div()
                        .child(format!(
                            "{} · {}",
                            volume(in_flows, in_bytes),
                            widgets::plural(incoming.len(), "peer", "peers")
                        ))
                        .into_any_element(),
                ),
                (
                    "Out".into(),
                    div()
                        .child(format!(
                            "{} · {}",
                            volume(out_flows, out_bytes),
                            widgets::plural(outgoing.len(), "peer", "peers")
                        ))
                        .into_any_element(),
                ),
            ];
            if let Some(edge) = inside {
                rows.push((
                    "Inside".into(),
                    div()
                        .child(volume(edge.flows, edge.bytes))
                        .into_any_element(),
                ));
            }
            if node.blocked > 0 {
                // The most common reason among the edges that touch it.
                let reason = graph
                    .edges
                    .iter()
                    .filter(|e| e.source == i || e.target == i)
                    .flat_map(|e| e.policies.iter())
                    .max_by_key(|p| p.flows)
                    .map(|p| format!(" · {}", p.text))
                    .unwrap_or_default();
                rows.push((
                    "Blocked".into(),
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(colors.red)
                        .child(format!("{}{reason}", widgets::count(node.blocked as usize)))
                        .into_any_element(),
                ));
            }
            let mut peers: Vec<(String, u64, u64)> = incoming
                .iter()
                .map(|e| {
                    (
                        format!("← {}", graph.nodes[e.source].label),
                        e.flows,
                        e.blocked(),
                    )
                })
                .chain(outgoing.iter().map(|e| {
                    (
                        format!("→ {}", graph.nodes[e.target].label),
                        e.flows,
                        e.blocked(),
                    )
                }))
                .collect();
            peers.sort_by_key(|p| std::cmp::Reverse(p.1));
            let kind = match node.kind {
                NodeKind::Namespace => "Namespace",
                NodeKind::Workload => "Workload",
                NodeKind::Folded => "Workloads",
                _ => "Endpoint",
            };
            let zoom_in = (node.kind == NodeKind::Namespace).then(|| {
                let weak = weak.clone();
                let filter = node.filter.clone();
                widgets::button(
                    "graph-workloads",
                    Some(IconName::Waypoints),
                    "Workloads",
                    false,
                    colors,
                    move |_, _, cx| {
                        let filter = filter.clone();
                        weak.update(cx, |this, cx| {
                            this.add_term(&filter, cx);
                            this.set_zoom(Zoom::Workloads, cx);
                        })
                        .ok();
                    },
                )
            });
            let heading = v_flex()
                .gap(u(8.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            Icon::new(match node.kind {
                                NodeKind::Namespace => IconName::Folder,
                                NodeKind::Workload => IconName::Box,
                                NodeKind::Folded => IconName::Layers,
                                NodeKind::World => IconName::Globe,
                                _ => IconName::Server,
                            })
                            .size(14.0)
                            .color(colors.accent),
                        )
                        .child(widgets::mono(node.label.clone(), colors).text_size(u(12.5))),
                )
                .child(h_flex().gap(u(6.0)).child(show).children(zoom_in))
                .into_any_element();
            let traffic =
                widgets::section(format!("Traffic · last {}", view.window.label()), colors)
                    .child(widgets::kv(rows, 84.0, colors))
                    .into_any_element();
            let counts = |label: String, flows: u64, blocked: u64| {
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(label),
                    )
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .text_color(colors.text_muted)
                            .child(compact(flows)),
                    )
                    .child(
                        div()
                            .w(u(48.0))
                            .flex()
                            .justify_end()
                            .text_color(if blocked > 0 {
                                colors.red
                            } else {
                                colors.text_faint
                            })
                            .child(if blocked > 0 {
                                compact(blocked)
                            } else {
                                "—".into()
                            }),
                    )
            };
            let members = graph.members.get(&node.id).filter(|m| !m.is_empty());
            let peer_list = widgets::section("Peers", colors)
                .when(members.is_none(), |this| this.border_b_0())
                .child(
                    v_flex().gap(u(3.0)).children(
                        peers
                            .into_iter()
                            .take(12)
                            .map(|(label, flows, blocked)| counts(label, flows, blocked)),
                    ),
                )
                .into_any_element();
            let mut sections = vec![traffic, peer_list];
            if let Some(members) = members {
                sections.push(
                    widgets::section("Workloads", colors)
                        .border_b_0()
                        .child(
                            v_flex().gap(u(3.0)).children(
                                members
                                    .iter()
                                    .take(12)
                                    .map(|m| counts(m.label.clone(), m.flows, m.blocked)),
                            ),
                        )
                        .into_any_element(),
                );
            }
            (kind, heading, sections)
        }
        Selection::Edge(a, b) => {
            let (ia, ib) = (view.topology.index_of(a)?, view.topology.index_of(b)?);
            let edge = graph
                .edges
                .iter()
                .find(|e| e.source == ia && e.target == ib)?;
            let mut rows: Vec<(SharedString, AnyElement)> = vec![(
                "Volume".into(),
                div()
                    .child(format!(
                        "{} · last {}",
                        volume(edge.flows, edge.bytes),
                        view.window.label()
                    ))
                    .into_any_element(),
            )];
            if !graph.bytes_only {
                rows.push((
                    "Verdicts".into(),
                    h_flex()
                        .gap(u(8.0))
                        .when(edge.forwarded > 0, |this| {
                            this.child(
                                div()
                                    .text_color(colors.green)
                                    .child(format!("{} forwarded", compact(edge.forwarded))),
                            )
                        })
                        .when(edge.dropped > 0, |this| {
                            this.child(
                                div()
                                    .text_color(colors.red)
                                    .child(format!("{} dropped", compact(edge.dropped))),
                            )
                        })
                        .when(edge.no_reply > 0, |this| {
                            this.child(
                                div()
                                    .text_color(colors.yellow)
                                    .child(format!("{} no reply", compact(edge.no_reply))),
                            )
                        })
                        .into_any_element(),
                ));
            }
            if !edge.ports.is_empty() {
                rows.push((
                    "Ports".into(),
                    widgets::mono(
                        edge.ports
                            .iter()
                            .map(|(p, _)| p.to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                        colors,
                    )
                    .into_any_element(),
                ));
            }
            let heading = v_flex()
                .gap(u(8.0))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(Icon::new(IconName::ArrowRight).size(14.0).color(
                            if edge.blocked() > 0 {
                                colors.red
                            } else {
                                colors.accent
                            },
                        ))
                        .child(
                            widgets::mono(
                                format!("{} → {}", graph.nodes[ia].label, graph.nodes[ib].label),
                                colors,
                            )
                            .text_size(u(12.5)),
                        ),
                )
                .child(
                    h_flex().gap(u(6.0)).child(show).children(
                        edge.policies
                            .iter()
                            .find_map(|p| p.policy.as_ref())
                            .and_then(|policy| {
                                Some((
                                    policy.name.clone(),
                                    widgets::policy_target(&view.cluster, policy, cx)?,
                                ))
                            })
                            .map(|(name, target)| {
                                widgets::button(
                                    "graph-open-policy",
                                    Some(IconName::Shield),
                                    format!("Open {name}"),
                                    false,
                                    colors,
                                    move |_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(OpenView(ViewRequest::for_resource(
                                                ViewKind::Details,
                                                target.clone(),
                                            ))),
                                            cx,
                                        );
                                    },
                                )
                            }),
                    ),
                )
                .into_any_element();
            let mut sections = vec![
                widgets::section("Connection", colors)
                    .child(widgets::kv(rows, 84.0, colors))
                    .into_any_element(),
            ];
            if !edge.policies.is_empty() {
                sections.push(
                    widgets::section("Policy", colors)
                        .border_b_0()
                        .child(
                            v_flex()
                                .gap(u(6.0))
                                .children(edge.policies.iter().take(5).map(|policy| {
                                    h_flex()
                                        .items_start()
                                        .gap(u(8.0))
                                        .text_size(u(12.5))
                                        .child(
                                            Icon::new(IconName::Shield)
                                                .size(14.0)
                                                .color(colors.red),
                                        )
                                        .child(div().flex_1().min_w_0().child(policy.text.clone()))
                                        .child(
                                            div()
                                                .font_family(fonts::MONO)
                                                .text_size(u(11.5))
                                                .text_color(colors.text_dim)
                                                .child(compact(policy.flows)),
                                        )
                                })),
                        )
                        .into_any_element(),
                );
            }
            ("Connection", heading, sections)
        }
    };
    let close = kubyl_ui::IconButton::new("graph-panel-close", IconName::X)
        .icon_size(13.0)
        .on_click(move |_, _, cx| {
            weak.update(cx, |this, cx| this.select(None, cx)).ok();
        });
    Some(
        v_flex()
            .flex_none()
            .w(u(320.0))
            .h_full()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .flex_none()
                    .h(u(34.0))
                    .px(u(12.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(div().flex_1().font_weight(FontWeight::MEDIUM).child(title))
                    .child(close),
            )
            .child(
                v_flex()
                    .id("graph-panel-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .px(u(14.0))
                            .py(u(12.0))
                            .border_b_1()
                            .border_color(colors.border_variant)
                            .child(heading),
                    )
                    .children(sections),
            )
            .into_any_element(),
    )
}
