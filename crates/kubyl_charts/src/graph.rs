//! Force-directed layout for node-link views (phase 16's network topology).
//!
//! [`layout`] places nodes with d3-force's model (the `fjadra` port): links pull connected nodes
//! to a distance, nodes push each other away (Barnes–Hut), circles don't overlap, and a weak pull
//! toward the middle keeps separate components together. It's CPU work: run it off the UI thread
//! (`cx.background_spawn`); its inputs and output are plain data.
//!
//! A live graph stays calm: nodes start where the previous layout put them (by id) and the
//! simulation only settles what changed. New nodes start next to a neighbor that's already
//! placed, else at a spot derived from their id, so the same graph always gets the same layout.
//! [`fit`] maps the result into a view.

use std::collections::HashMap;

use fjadra::{Center, Collide, Link, ManyBody, Node, PositionX, PositionY, SimulationBuilder};

/// A node to place: its id (stable across layouts) and the radius it's drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub radius: f32,
}

/// A link between two nodes (indices into the node list). `weight` in `0.0..=1.0` pulls
/// harder and closer: heavy traffic ends up short.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphEdge {
    pub source: usize,
    pub target: usize,
    pub weight: f32,
}

/// Positions of a previous layout, by node id.
pub type Positions = HashMap<String, [f32; 2]>;

/// Room kept around each circle (labels sit under the nodes).
const LABEL_ROOM: f64 = 22.0;
/// Ticks of a cold start (d3's default: alpha from 1 to 0.001).
const COLD_TICKS: usize = 300;

/// Places `nodes` (in layout units, centered on the origin). `previous` warms the start.
pub fn layout(nodes: &[GraphNode], edges: &[GraphEdge], previous: &Positions) -> Vec<[f32; 2]> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let known = nodes
        .iter()
        .filter(|n| previous.contains_key(&n.id))
        .count();
    let warm = known * 2 >= nodes.len();
    let starts = starts(nodes, edges, previous);
    let radii: Vec<f64> = nodes.iter().map(|n| f64::from(n.radius)).collect();
    let edges: Vec<&GraphEdge> = edges
        .iter()
        .filter(|e| e.source != e.target && e.source < nodes.len() && e.target < nodes.len())
        .collect();
    let links: Vec<(usize, usize)> = edges.iter().map(|e| (e.source, e.target)).collect();
    // Heavy traffic pulls closer.
    let distances: Vec<f64> = edges
        .iter()
        .map(|e| {
            radii[e.source] + radii[e.target] + 70.0 - 30.0 * f64::from(e.weight.clamp(0.0, 1.0))
        })
        .collect();
    let charges: Vec<f64> = radii.iter().map(|r| -120.0 - 4.0 * r).collect();
    let collide_radii: Vec<f64> = radii.iter().map(|r| r + LABEL_ROOM).collect();
    let builder = if warm {
        // Most nodes are placed: settle the changes without shaking the rest.
        SimulationBuilder::default()
            .with_alpha(0.3)
            .with_alpha_min(0.01)
    } else {
        SimulationBuilder::default()
    };
    let mut simulation = builder
        .build(starts.iter().map(|&[x, y]| Node::default().position(x, y)))
        .add_force(
            "link",
            Link::new(links).distance(by_index2(distances)).iterations(2),
        )
        .add_force("charge", ManyBody::new().strength(by_index(charges)))
        .add_force(
            "collide",
            Collide::new()
                .radius(move |i| collide_radii[i])
                .iterations(2),
        )
        .add_force("center", Center::new())
        .add_force("x", PositionX::new().strength(0.04))
        .add_force("y", PositionY::new().strength(0.06));
    let mut ticks = 0;
    while !simulation.is_finished() && ticks < COLD_TICKS {
        simulation.tick(1);
        ticks += 1;
    }
    simulation
        .positions()
        .map(|[x, y]| [x as f32, y as f32])
        .collect()
}

/// A per-node value for fjadra (its callbacks get the node's index second).
fn by_index<T>(values: Vec<f64>) -> impl Fn(T, usize) -> f64 + 'static {
    move |_, i| values[i]
}

/// A per-link value for fjadra (its callbacks get the link's index second).
fn by_index2<T>(values: Vec<f64>) -> impl Fn(&T, usize) -> f64 + 'static {
    move |_, i| values[i]
}

/// Where each node starts: its previous position, else next to a placed neighbor, else a spot
/// derived from its id.
fn starts(nodes: &[GraphNode], edges: &[GraphEdge], previous: &Positions) -> Vec<[f64; 2]> {
    let mut placed: Vec<Option<[f64; 2]>> = nodes
        .iter()
        .map(|n| {
            previous
                .get(&n.id)
                .map(|&[x, y]| [f64::from(x), f64::from(y)])
        })
        .collect();
    for (i, node) in nodes.iter().enumerate() {
        if placed[i].is_some() {
            continue;
        }
        let (angle, distance) = seed(&node.id);
        let neighbor = edges.iter().find_map(|e| {
            let other = if e.source == i {
                e.target
            } else if e.target == i {
                e.source
            } else {
                return None;
            };
            placed.get(other).copied().flatten()
        });
        placed[i] = Some(match neighbor {
            Some([x, y]) => {
                let d = f64::from(node.radius) + 60.0;
                [x + d * angle.cos(), y + d * angle.sin()]
            }
            None => [distance * angle.cos(), distance * angle.sin()],
        });
    }
    placed.into_iter().map(Option::unwrap_or_default).collect()
}

/// An angle and a distance from the id (FNV-1a), the same on every run.
fn seed(id: &str) -> (f64, f64) {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let angle = (hash & 0xffff) as f64 / 65_536.0 * std::f64::consts::TAU;
    let distance = 40.0 + ((hash >> 16) & 0xff) as f64;
    (angle, distance)
}

/// How layout units map into a view: `view = layout * scale + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub scale: f32,
    pub offset: [f32; 2],
}

impl Viewport {
    pub fn apply(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            x * self.scale + self.offset[0],
            y * self.scale + self.offset[1],
        ]
    }

    /// The layout position under a view position.
    pub fn invert(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            (x - self.offset[0]) / self.scale,
            (y - self.offset[1]) / self.scale,
        ]
    }
}

/// Fits circles at `positions` with `radii` (plus room for labels below) into a `size` view
/// with `padding`, never enlarging past `max_scale`.
pub fn fit(
    positions: &[[f32; 2]],
    radii: &[f32],
    size: [f32; 2],
    padding: f32,
    max_scale: f32,
) -> Viewport {
    let label = LABEL_ROOM as f32;
    let mut min = [f32::MAX, f32::MAX];
    let mut max = [f32::MIN, f32::MIN];
    for (i, &[x, y]) in positions.iter().enumerate() {
        let r = radii.get(i).copied().unwrap_or(0.0);
        min[0] = min[0].min(x - r);
        min[1] = min[1].min(y - r);
        max[0] = max[0].max(x + r);
        max[1] = max[1].max(y + r + label);
    }
    if positions.is_empty() {
        return Viewport {
            scale: 1.0,
            offset: [size[0] / 2.0, size[1] / 2.0],
        };
    }
    let width = (max[0] - min[0]).max(1.0);
    let height = (max[1] - min[1]).max(1.0);
    let scale = ((size[0] - 2.0 * padding) / width)
        .min((size[1] - 2.0 * padding) / height)
        .clamp(0.05, max_scale);
    let center = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    Viewport {
        scale,
        offset: [
            size[0] / 2.0 - center[0] * scale,
            size[1] / 2.0 - center[1] * scale,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(n: usize) -> (Vec<GraphNode>, Vec<GraphEdge>) {
        let nodes = (0..n)
            .map(|i| GraphNode {
                id: format!("node-{i}"),
                radius: 8.0 + (i % 5) as f32 * 4.0,
            })
            .collect();
        let edges = (0..n)
            .flat_map(|i| {
                [
                    GraphEdge {
                        source: i,
                        target: (i + 1) % n,
                        weight: 1.0,
                    },
                    GraphEdge {
                        source: i,
                        target: (i * 7 + 3) % n,
                        weight: 0.3,
                    },
                    GraphEdge {
                        source: i,
                        target: (i * 13 + 5) % n,
                        weight: 0.1,
                    },
                ]
            })
            .collect();
        (nodes, edges)
    }

    #[test]
    fn same_graph_same_layout_and_no_overlaps() {
        let (nodes, edges) = ring(24);
        let a = layout(&nodes, &edges, &Positions::new());
        let b = layout(&nodes, &edges, &Positions::new());
        assert_eq!(a, b);
        for i in 0..nodes.len() {
            for j in i + 1..nodes.len() {
                let d = ((a[i][0] - a[j][0]).powi(2) + (a[i][1] - a[j][1]).powi(2)).sqrt();
                assert!(
                    d > nodes[i].radius + nodes[j].radius,
                    "{i} and {j} overlap: {d}"
                );
            }
        }
    }

    /// A node added to a laid-out graph doesn't move the others far.
    #[test]
    fn warm_starts_keep_the_graph_calm() {
        let (mut nodes, mut edges) = ring(20);
        let first = layout(&nodes, &edges, &Positions::new());
        let previous: Positions = nodes
            .iter()
            .zip(&first)
            .map(|(n, &p)| (n.id.clone(), p))
            .collect();
        nodes.push(GraphNode {
            id: "new".into(),
            radius: 10.0,
        });
        edges.push(GraphEdge {
            source: 20,
            target: 3,
            weight: 0.5,
        });
        let second = layout(&nodes, &edges, &previous);
        let moved: f32 = first
            .iter()
            .zip(&second)
            .map(|(a, b)| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt())
            .sum::<f32>()
            / first.len() as f32;
        assert!(moved < 40.0, "nodes moved {moved} on average");
        // The new node starts near its neighbor and stays in the picture.
        let d = ((second[20][0] - second[3][0]).powi(2) + (second[20][1] - second[3][1]).powi(2))
            .sqrt();
        assert!(d < 300.0, "{d}");
    }

    /// The topology's limit: a few hundred nodes lay out quickly (debug builds too).
    #[test]
    fn three_hundred_nodes_are_quick() {
        let (nodes, edges) = ring(300);
        let start = std::time::Instant::now();
        let positions = layout(&nodes, &edges, &Positions::new());
        let took = start.elapsed();
        assert_eq!(positions.len(), 300);
        assert!(positions.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
        assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn fit_centers_and_scales() {
        let viewport = fit(
            &[[-100.0, -50.0], [100.0, 50.0]],
            &[10.0, 10.0],
            [440.0, 240.0],
            10.0,
            2.0,
        );
        let a = viewport.apply([-110.0, -60.0]);
        let b = viewport.apply([110.0, 60.0 + LABEL_ROOM as f32]);
        // The height limits: it touches the padding, the width is centered inside it.
        assert!((a[1] - 10.0).abs() < 0.01 && (b[1] - 230.0).abs() < 0.01, "{a:?} {b:?}");
        assert!(a[0] >= 10.0 && b[0] <= 430.0, "{a:?} {b:?}");
        assert!(((a[0] + b[0]) / 2.0 - 220.0).abs() < 0.01, "{a:?} {b:?}");
        let back = viewport.invert(viewport.apply([12.0, -7.0]));
        assert!((back[0] - 12.0).abs() < 0.001 && (back[1] + 7.0).abs() < 0.001);
        // One node isn't blown up.
        assert_eq!(fit(&[[0.0, 0.0]], &[10.0], [800.0, 600.0], 20.0, 1.5).scale, 1.5);
    }
}
