//! XRay (k9s `:xray`): a tree from a workload or service down to the pods it runs, their
//! containers and the ConfigMaps, Secrets, volume claims and service account they use.
//!
//! The tree is built from objects the explorer already caches ([`Inputs`]); nothing here reads
//! the cluster. Secret *data* is never looked at: only the names a pod refers to.

mod build;
pub mod filter;
mod rows;

pub use build::{Inputs, build};
pub use filter::Filter;
pub use rows::{Row, flatten};

/// What an XRay starts from (`:xray dp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Root {
    Pods,
    Services,
    Deployments,
    ReplicaSets,
    StatefulSets,
    DaemonSets,
}

impl Root {
    pub const ALL: [Root; 6] = [
        Root::Pods,
        Root::Services,
        Root::Deployments,
        Root::ReplicaSets,
        Root::StatefulSets,
        Root::DaemonSets,
    ];

    /// Parses k9s' names: `po`, `pods`, `svc`, `dp`, `deploy`, `rs`, `sts`, `ds`…
    pub fn parse(name: &str) -> Option<Root> {
        Some(match name.to_lowercase().as_str() {
            "po" | "pod" | "pods" => Root::Pods,
            "svc" | "service" | "services" => Root::Services,
            "dp" | "deploy" | "deployment" | "deployments" => Root::Deployments,
            "rs" | "replicaset" | "replicasets" => Root::ReplicaSets,
            "sts" | "statefulset" | "statefulsets" => Root::StatefulSets,
            "ds" | "daemonset" | "daemonsets" => Root::DaemonSets,
            _ => return None,
        })
    }

    /// The name stored in tab requests.
    pub fn id(self) -> &'static str {
        match self {
            Root::Pods => "po",
            Root::Services => "svc",
            Root::Deployments => "dp",
            Root::ReplicaSets => "rs",
            Root::StatefulSets => "sts",
            Root::DaemonSets => "ds",
        }
    }

    /// API group and plural of the root kind.
    pub fn api(self) -> (&'static str, &'static str) {
        match self {
            Root::Pods => ("", "pods"),
            Root::Services => ("", "services"),
            Root::Deployments => ("apps", "deployments"),
            Root::ReplicaSets => ("apps", "replicasets"),
            Root::StatefulSets => ("apps", "statefulsets"),
            Root::DaemonSets => ("apps", "daemonsets"),
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Root::Pods => "Pods",
            Root::Services => "Services",
            Root::Deployments => "Deployments",
            Root::ReplicaSets => "ReplicaSets",
            Root::StatefulSets => "StatefulSets",
            Root::DaemonSets => "DaemonSets",
        }
    }
}

/// How a node is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Health {
    Ok,
    Warning,
    Bad,
    /// Nothing to judge (config objects, labels).
    #[default]
    Neutral,
}

/// One object of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: &'static str,
    /// API group and plural of the object, to open it (`""`, `pods`). Empty `resource`: a
    /// container, which has no page of its own.
    pub group: &'static str,
    pub resource: &'static str,
    pub namespace: Option<String>,
    pub name: String,
    pub health: Health,
    /// `3/3`, `Running`, `CrashLoopBackOff`, `missing`.
    pub info: String,
    pub children: Vec<Node>,
}

impl Node {
    /// Identifies the node among its siblings (for expansion state).
    pub fn id(&self) -> String {
        format!("{}/{}", self.kind, self.name)
    }

    /// The worst health in this subtree, so a collapsed node still shows trouble below.
    pub fn worst(&self) -> Health {
        self.children
            .iter()
            .map(Node::worst)
            .chain([self.health])
            .max_by_key(|h| match h {
                Health::Bad => 3,
                Health::Warning => 2,
                Health::Ok => 1,
                Health::Neutral => 0,
            })
            .unwrap_or_default()
    }
}
