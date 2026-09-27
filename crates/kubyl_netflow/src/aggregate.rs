//! The topology (README "Topology graph"): flows aggregated per source and destination at
//! namespace or workload zoom, with flow counts, bytes, packets and the verdict mix. Runs in the
//! background over a snapshot of the filtered window.

use std::collections::HashMap;

use crate::model::{Endpoint, EndpointKind, Flow, Verdict};

/// How far the graph zooms in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Zoom {
    #[default]
    Namespaces,
    Workloads,
}

impl Zoom {
    pub fn key(self) -> &'static str {
        match self {
            Zoom::Namespaces => "namespaces",
            Zoom::Workloads => "workloads",
        }
    }
}

/// What a node stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Namespace,
    Workload,
    /// "more in <namespace>": workloads folded past the node limit.
    Folded,
    World,
    Host,
    RemoteNode,
    KubeApiServer,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopoNode {
    /// Stable id (`ns:payments`, `wl:payments/checkout-api`, `world`).
    pub id: String,
    pub label: String,
    pub namespace: Option<String>,
    pub kind: NodeKind,
    /// Flows in and out.
    pub flows: u64,
    pub bytes: u64,
    /// Blocked flows touching the node.
    pub blocked: u64,
    /// The filter term that shows this node's flows (`ns=payments`).
    pub filter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopoEdge {
    pub source: usize,
    pub target: usize,
    pub flows: u64,
    pub bytes: u64,
    pub packets: u64,
    pub forwarded: u64,
    pub dropped: u64,
    pub no_reply: u64,
    /// Policies behind the blocked flows (`denied by storefront/web-guard`), most common first.
    pub policies: Vec<(String, u64)>,
    /// Destination ports, most common first.
    pub ports: Vec<(u16, u64)>,
}

impl TopoEdge {
    pub fn blocked(&self) -> u64 {
        self.dropped + self.no_reply
    }
}

/// The graph of a window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Topology {
    pub nodes: Vec<TopoNode>,
    pub edges: Vec<TopoEdge>,
    /// Workloads folded into "more in" nodes.
    pub folded: usize,
    /// Volume is bytes (metrics) rather than flow counts.
    pub bytes_only: bool,
}

/// Workload nodes shown before the smallest fold into "more in <namespace>".
pub const NODE_LIMIT: usize = 150;

struct NodeKey {
    id: String,
    label: String,
    namespace: Option<String>,
    kind: NodeKind,
    filter: String,
}

fn node_key(endpoint: &Endpoint, zoom: Zoom) -> NodeKey {
    let other = |id: &str, label: &str, kind: NodeKind, filter: String| NodeKey {
        id: id.into(),
        label: label.into(),
        namespace: None,
        kind,
        filter,
    };
    match (endpoint.kind, &endpoint.namespace) {
        (_, Some(ns)) => match zoom {
            Zoom::Namespaces => NodeKey {
                id: format!("ns:{ns}"),
                label: ns.to_string(),
                namespace: Some(ns.to_string()),
                kind: NodeKind::Namespace,
                filter: format!("ns={ns}"),
            },
            Zoom::Workloads => {
                let name = endpoint
                    .workload_name()
                    .or(endpoint.service.as_deref())
                    .unwrap_or("?")
                    .trim_end_matches("-*")
                    .to_string();
                NodeKey {
                    id: format!("wl:{ns}/{name}"),
                    label: name.clone(),
                    namespace: Some(ns.to_string()),
                    kind: NodeKind::Workload,
                    filter: format!("workload={ns}/{name}"),
                }
            }
        },
        (EndpointKind::World, _) => other("world", "world", NodeKind::World, "kind=world".into()),
        (EndpointKind::Host, _) => other("host", "host", NodeKind::Host, "kind=host".into()),
        (EndpointKind::RemoteNode, _) => other(
            "remote-node",
            "remote nodes",
            NodeKind::RemoteNode,
            "kind=remote-node".into(),
        ),
        (EndpointKind::KubeApiServer, _) => other(
            "kube-apiserver",
            "kube-apiserver",
            NodeKind::KubeApiServer,
            "kind=kube-apiserver".into(),
        ),
        _ => other(
            "other",
            "other addresses",
            NodeKind::Other,
            "kind=unknown,internal".into(),
        ),
    }
}

#[derive(Default)]
struct EdgeAcc {
    flows: u64,
    bytes: u64,
    packets: u64,
    forwarded: u64,
    dropped: u64,
    no_reply: u64,
    policies: HashMap<String, u64>,
    ports: HashMap<u16, u64>,
}

/// Aggregates `flows` at `zoom`. Beyond `limit` workload nodes, the smallest of each
/// namespace fold into one "more in" node.
pub fn aggregate<'a>(
    flows: impl IntoIterator<Item = &'a Flow>,
    zoom: Zoom,
    limit: usize,
) -> Topology {
    let mut nodes: Vec<TopoNode> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut edges: HashMap<(usize, usize), EdgeAcc> = HashMap::new();
    let mut node = |key: NodeKey, nodes: &mut Vec<TopoNode>| -> usize {
        *index.entry(key.id.clone()).or_insert_with(|| {
            nodes.push(TopoNode {
                id: key.id,
                label: key.label,
                namespace: key.namespace,
                kind: key.kind,
                flows: 0,
                bytes: 0,
                blocked: 0,
                filter: key.filter,
            });
            nodes.len() - 1
        })
    };
    for flow in flows {
        let a = node(node_key(&flow.source, zoom), &mut nodes);
        let b = node(node_key(&flow.destination, zoom), &mut nodes);
        let bytes = flow.bytes.unwrap_or(0);
        let blocked = flow.verdict.blocked();
        for i in [a, b] {
            nodes[i].flows += 1;
            nodes[i].bytes += bytes;
            nodes[i].blocked += u64::from(blocked);
        }
        let edge = edges.entry((a, b)).or_default();
        edge.flows += 1;
        edge.bytes += bytes;
        edge.packets += flow.packets.unwrap_or(0);
        match flow.verdict {
            Verdict::Dropped => edge.dropped += 1,
            Verdict::NoReply => edge.no_reply += 1,
            _ => edge.forwarded += 1,
        }
        if blocked {
            let summary = flow.policies.summary(flow.verdict).text();
            if !summary.is_empty() {
                *edge.policies.entry(summary).or_default() += 1;
            }
        }
        if let Some(port) = flow.destination.port {
            *edge.ports.entry(port).or_default() += 1;
        }
    }
    let mut topology = Topology {
        nodes,
        edges: edges
            .into_iter()
            .map(|((source, target), acc)| {
                let mut policies: Vec<(String, u64)> = acc.policies.into_iter().collect();
                policies.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let mut ports: Vec<(u16, u64)> = acc.ports.into_iter().collect();
                ports.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                ports.truncate(5);
                TopoEdge {
                    source,
                    target,
                    flows: acc.flows,
                    bytes: acc.bytes,
                    packets: acc.packets,
                    forwarded: acc.forwarded,
                    dropped: acc.dropped,
                    no_reply: acc.no_reply,
                    policies,
                    ports,
                }
            })
            .collect(),
        folded: 0,
        bytes_only: false,
    };
    topology.edges.sort_by_key(|e| (e.source, e.target));
    if zoom == Zoom::Workloads {
        fold(&mut topology, limit);
    }
    topology
}

/// Keeps the `limit` busiest nodes; the other workloads of each namespace become one node.
fn fold(topology: &mut Topology, limit: usize) {
    if topology.nodes.len() <= limit {
        return;
    }
    let mut order: Vec<usize> = (0..topology.nodes.len()).collect();
    order.sort_by(|&a, &b| {
        topology.nodes[b]
            .flows
            .cmp(&topology.nodes[a].flows)
            .then_with(|| topology.nodes[a].id.cmp(&topology.nodes[b].id))
    });
    let keep: std::collections::HashSet<usize> = order
        .iter()
        .copied()
        .filter(|&i| topology.nodes[i].kind != NodeKind::Workload)
        .chain(
            order
                .iter()
                .copied()
                .filter(|&i| topology.nodes[i].kind == NodeKind::Workload),
        )
        .take(limit)
        .collect();
    let mut remap = vec![0usize; topology.nodes.len()];
    let mut nodes: Vec<TopoNode> = Vec::new();
    let mut folded_index: HashMap<String, usize> = HashMap::new();
    let mut folded = 0;
    for (i, node) in topology.nodes.iter().enumerate() {
        if keep.contains(&i) {
            remap[i] = nodes.len();
            nodes.push(node.clone());
            continue;
        }
        folded += 1;
        let ns = node.namespace.clone().unwrap_or_default();
        let at = *folded_index.entry(ns.clone()).or_insert_with(|| {
            nodes.push(TopoNode {
                id: format!("more:{ns}"),
                label: format!("more in {ns}"),
                namespace: Some(ns.clone()),
                kind: NodeKind::Folded,
                flows: 0,
                bytes: 0,
                blocked: 0,
                filter: format!("ns={ns}"),
            });
            nodes.len() - 1
        });
        nodes[at].flows += node.flows;
        nodes[at].bytes += node.bytes;
        nodes[at].blocked += node.blocked;
        remap[i] = at;
    }
    let mut merged: HashMap<(usize, usize), TopoEdge> = HashMap::new();
    for edge in topology.edges.drain(..) {
        let key = (remap[edge.source], remap[edge.target]);
        match merged.get_mut(&key) {
            Some(existing) => {
                existing.flows += edge.flows;
                existing.bytes += edge.bytes;
                existing.packets += edge.packets;
                existing.forwarded += edge.forwarded;
                existing.dropped += edge.dropped;
                existing.no_reply += edge.no_reply;
                existing.policies.extend(edge.policies);
                existing.ports.extend(edge.ports);
            }
            None => {
                merged.insert(
                    key,
                    TopoEdge {
                        source: key.0,
                        target: key.1,
                        ..edge
                    },
                );
            }
        }
    }
    topology.nodes = nodes;
    topology.edges = merged.into_values().collect();
    topology.edges.sort_by_key(|e| (e.source, e.target));
    topology.folded = folded;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Policies, Workload};
    use jiff::Timestamp;

    fn pod(ns: &str, workload: &str) -> Endpoint {
        Endpoint {
            kind: EndpointKind::Pod,
            namespace: Some(ns.into()),
            pod: Some(format!("{workload}-abc12").into()),
            workload: Some(Workload {
                kind: "Deployment".into(),
                name: workload.into(),
            }),
            port: Some(80),
            ..Endpoint::default()
        }
    }

    fn flow(from: Endpoint, to: Endpoint, verdict: Verdict) -> Flow {
        let mut flow = Flow::new(Timestamp::UNIX_EPOCH);
        flow.source = from;
        flow.destination = to;
        flow.verdict = verdict;
        flow.bytes = Some(100);
        if verdict == Verdict::Dropped {
            flow.policies = Policies {
                isolated: true,
                ..Policies::default()
            };
        }
        flow
    }

    #[test]
    fn namespaces_and_workloads() {
        let flows = vec![
            flow(
                pod("storefront", "shopper"),
                pod("payments", "checkout-api"),
                Verdict::Forwarded,
            ),
            flow(
                pod("storefront", "shopper"),
                pod("payments", "ledger-api"),
                Verdict::Dropped,
            ),
            flow(
                pod("storefront", "shopper"),
                pod("payments", "ledger-api"),
                Verdict::Dropped,
            ),
            flow(
                pod("payments", "checkout-client"),
                pod("payments", "ledger-api"),
                Verdict::Forwarded,
            ),
            flow(
                pod("payments", "payment-gateway"),
                Endpoint {
                    kind: EndpointKind::World,
                    ..Endpoint::default()
                },
                Verdict::Forwarded,
            ),
        ];
        let namespaces = aggregate(&flows, Zoom::Namespaces, NODE_LIMIT);
        let ids: Vec<&str> = namespaces.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["ns:storefront", "ns:payments", "world"]);
        let cross = &namespaces.edges[0];
        assert_eq!((cross.source, cross.target, cross.flows), (0, 1, 3));
        assert_eq!((cross.forwarded, cross.dropped), (1, 2));
        assert_eq!(
            cross.policies,
            vec![("denied: isolated, no policy allows it".to_string(), 2)]
        );
        assert_eq!(cross.ports, vec![(80, 3)]);
        assert_eq!(namespaces.nodes[1].blocked, 2);
        assert_eq!(namespaces.nodes[0].filter, "ns=storefront");
        let workloads = aggregate(&flows, Zoom::Workloads, NODE_LIMIT);
        assert_eq!(workloads.nodes.len(), 6);
        assert!(
            workloads
                .nodes
                .iter()
                .any(|n| n.filter == "workload=payments/ledger-api")
        );
    }

    #[test]
    fn workloads_past_the_limit_fold_per_namespace() {
        let flows: Vec<Flow> = (0..40)
            .map(|i| {
                let ns = if i % 2 == 0 { "a" } else { "b" };
                let mut f = flow(
                    pod(ns, &format!("client-{i}")),
                    pod("server", "api"),
                    Verdict::Forwarded,
                );
                // Busier clients first.
                f.bytes = Some(1);
                f
            })
            .chain((0..10).map(|_| {
                flow(
                    pod("a", "client-0"),
                    pod("server", "api"),
                    Verdict::Forwarded,
                )
            }))
            .collect();
        let topology = aggregate(&flows, Zoom::Workloads, 5);
        assert_eq!(topology.nodes.len(), 7);
        assert_eq!(topology.folded, 36);
        assert!(topology.nodes.iter().any(|n| n.id == "wl:a/client-0"));
        let more_a = topology
            .nodes
            .iter()
            .position(|n| n.id == "more:a")
            .unwrap();
        let api = topology
            .nodes
            .iter()
            .position(|n| n.id == "wl:server/api")
            .unwrap();
        let edge = topology
            .edges
            .iter()
            .find(|e| e.source == more_a && e.target == api)
            .unwrap();
        assert_eq!(edge.flows, 16);
        let total: u64 = topology.edges.iter().map(|e| e.flows).sum();
        assert_eq!(total, 50);
    }
}
