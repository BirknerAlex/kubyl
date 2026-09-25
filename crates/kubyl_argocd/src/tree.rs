//! The resource tree of an Application (board 13): the app, its managed resources
//! (`status.resources`) and their children.
//!
//! Kubernetes mode builds it from Kubyl's watch caches: managed objects by key, children by
//! `ownerReferences` (Deployment → ReplicaSet → Pod, StatefulSet/DaemonSet/Job → Pod, CronJob →
//! Job → Pod), health from [`crate::health`]. API mode uses Argo CD's own tree
//! (`resource-tree`), which also has children of other kinds (EndpointSlices, Rollouts…).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::api::ResourceTree;
use crate::health;
use crate::model::{Application, Health, ManagedResource, SyncStatus, short_revision};

/// How deep children are followed.
const MAX_DEPTH: usize = 6;

/// One row of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// `group/kind/namespace/name`.
    pub id: String,
    pub group: String,
    pub version: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub depth: usize,
    /// In `status.resources` (Argo CD manages it) as opposed to a child.
    pub managed: bool,
    /// Shown from Kubyl's own watches (Kubernetes mode children).
    pub live: bool,
    /// The object exists in the cluster.
    pub exists: bool,
    pub sync: Option<SyncStatus>,
    pub health: Option<(Health, Option<String>)>,
    pub info: String,
    pub created: Option<String>,
    pub has_children: bool,
    pub requires_pruning: bool,
    pub hook: bool,
}

impl Node {
    fn new(
        group: &str,
        version: &str,
        kind: &str,
        namespace: &str,
        name: &str,
        depth: usize,
    ) -> Self {
        Self {
            id: node_id(group, kind, namespace, name),
            group: group.into(),
            version: version.into(),
            kind: kind.into(),
            namespace: namespace.into(),
            name: name.into(),
            depth,
            managed: false,
            live: false,
            exists: false,
            sync: None,
            health: None,
            info: String::new(),
            created: None,
            has_children: false,
            requires_pruning: false,
            hook: false,
        }
    }

    /// The app itself (the tree's root).
    pub fn is_app(&self) -> bool {
        self.depth == 0 && self.kind == "Application" && self.group == crate::model::GROUP
    }

    /// Whether the node matches the filter text (kind or name).
    pub fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.name.to_lowercase().contains(&query) || self.kind.to_lowercase().contains(&query)
    }

    pub fn unhealthy(&self) -> bool {
        matches!(
            self.health,
            Some((Health::Degraded | Health::Missing | Health::Unknown, _))
        )
    }
}

pub fn node_id(group: &str, kind: &str, namespace: &str, name: &str) -> String {
    format!("{group}/{kind}/{namespace}/{name}")
}

/// Live objects from Kubyl's watches, by key and by owner.
#[derive(Default)]
pub struct Live {
    by_key: HashMap<String, (String, Arc<Value>)>,
    by_owner: HashMap<String, Vec<(String, String, Arc<Value>)>>,
    /// `(group, kind, namespace)` whose watch has listed: absent objects are really missing.
    ready: HashSet<(String, String, String)>,
}

impl Live {
    /// Adds an object of `group`/`kind` (objects from list watches carry no `kind`).
    pub fn insert(&mut self, group: &str, kind: &str, object: Arc<Value>) {
        let kind = kind.to_string();
        let namespace = object
            .pointer("/metadata/namespace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let name = object
            .pointer("/metadata/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(owners) = object
            .pointer("/metadata/ownerReferences")
            .and_then(Value::as_array)
        {
            for owner in owners {
                if let Some(uid) = owner.get("uid").and_then(Value::as_str) {
                    self.by_owner.entry(uid.to_string()).or_default().push((
                        group.to_string(),
                        kind.clone(),
                        object.clone(),
                    ));
                }
            }
        }
        self.by_key
            .insert(node_id(group, &kind, &namespace, &name), (kind, object));
    }

    pub fn get(&self, group: &str, kind: &str, namespace: &str, name: &str) -> Option<&Arc<Value>> {
        self.by_key
            .get(&node_id(group, kind, namespace, name))
            .map(|(_, v)| v)
    }

    fn children(&self, uid: &str) -> &[(String, String, Arc<Value>)] {
        self.by_owner.get(uid).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// The watch of `kind` in `namespace` has listed.
    pub fn mark_ready(&mut self, group: &str, kind: &str, namespace: &str) {
        self.ready
            .insert((group.to_string(), kind.to_string(), namespace.to_string()));
    }

    fn is_ready(&self, group: &str, kind: &str, namespace: &str) -> bool {
        self.ready
            .contains(&(group.to_string(), kind.to_string(), namespace.to_string()))
    }
}

/// Kinds whose children Kubernetes mode follows, and the (group, plural) of those children.
pub fn child_kinds(group: &str, kind: &str) -> &'static [(&'static str, &'static str)] {
    match (group, kind) {
        ("apps", "Deployment") => &[("apps", "replicasets"), ("", "pods")],
        ("apps", "ReplicaSet" | "StatefulSet" | "DaemonSet") => &[("", "pods")],
        ("batch", "Job") => &[("", "pods")],
        ("batch", "CronJob") => &[("batch", "jobs"), ("", "pods")],
        _ => &[],
    }
}

fn kind_order(kind: &str) -> u8 {
    match kind {
        "Namespace" => 0,
        "ServiceAccount" | "ClusterRole" | "ClusterRoleBinding" | "Role" | "RoleBinding" => 1,
        "ConfigMap" | "Secret" => 2,
        "PersistentVolumeClaim" => 3,
        "Service" | "Ingress" => 4,
        "Deployment" | "StatefulSet" | "DaemonSet" | "CronJob" | "Job" => 5,
        "ReplicaSet" => 6,
        "Pod" => 7,
        _ => 8,
    }
}

fn int(object: &Value, pointer: &str) -> i64 {
    object.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

fn text<'a>(object: &'a Value, pointer: &str) -> &'a str {
    object
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A short description of a live object (`3/3 ready · rev 14`, `Running · 10.0.12.188`).
/// Never Secret values: Secrets show their key count.
pub fn info(group: &str, kind: &str, object: &Value) -> String {
    let revision = || {
        object
            .pointer("/metadata/annotations/deployment.kubernetes.io~1revision")
            .and_then(Value::as_str)
            .map(|r| format!(" · rev {r}"))
            .unwrap_or_default()
    };
    match (group, kind) {
        ("apps", "Deployment") | ("apps", "StatefulSet") => format!(
            "{}/{} ready{}",
            int(object, "/status/readyReplicas"),
            int(object, "/spec/replicas"),
            revision()
        ),
        ("apps", "DaemonSet") => format!(
            "{}/{} ready",
            int(object, "/status/numberReady"),
            int(object, "/status/desiredNumberScheduled")
        ),
        ("apps", "ReplicaSet") => {
            let pods = int(object, "/status/replicas");
            format!(
                "{pods} pod{}{}",
                if pods == 1 { "" } else { "s" },
                revision()
            )
        }
        ("", "Pod") => {
            let status = kubyl_resources::columns::pod_status(object).reason;
            match text(object, "/status/podIP") {
                "" => status,
                ip => format!("{status} · {ip}"),
            }
        }
        ("", "Service") => {
            let kind = text(object, "/spec/type");
            match text(object, "/spec/clusterIP") {
                "" | "None" => kind.to_string(),
                ip => format!("{kind} {ip}"),
            }
        }
        ("", "ConfigMap") | ("", "Secret") => {
            let keys = object
                .get("data")
                .and_then(Value::as_object)
                .map_or(0, |d| d.len())
                + object
                    .get("binaryData")
                    .and_then(Value::as_object)
                    .map_or(0, |d| d.len());
            format!("{keys} key{}", if keys == 1 { "" } else { "s" })
        }
        ("", "PersistentVolumeClaim") => {
            let capacity = text(object, "/status/capacity/storage");
            format!("{} {capacity}", text(object, "/status/phase"))
                .trim()
                .to_string()
        }
        ("batch", "Job") => format!(
            "{}/{} succeeded",
            int(object, "/status/succeeded"),
            object
                .pointer("/spec/completions")
                .and_then(Value::as_i64)
                .unwrap_or(1)
        ),
        ("batch", "CronJob") => text(object, "/spec/schedule").to_string(),
        ("autoscaling", "HorizontalPodAutoscaler") => format!(
            "{}–{} · {} now",
            object
                .pointer("/spec/minReplicas")
                .and_then(Value::as_i64)
                .unwrap_or(1),
            int(object, "/spec/maxReplicas"),
            int(object, "/status/currentReplicas")
        ),
        ("networking.k8s.io", "Ingress") => object
            .pointer("/spec/rules")
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .filter_map(|r| r.get("host").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn root(app: &Application) -> Node {
    let mut node = Node::new(
        crate::model::GROUP,
        "v1alpha1",
        "Application",
        app.namespace(),
        app.name(),
        0,
    );
    node.managed = true;
    node.exists = true;
    node.sync = Some(app.sync());
    node.health = Some((app.health(), app.status.health.message.clone()));
    node.info = app
        .synced_revisions()
        .iter()
        .map(|r| format!("rev {}", short_revision(r)))
        .collect::<Vec<_>>()
        .join(", ");
    node.created = app.metadata.creation_timestamp.clone();
    node
}

fn managed_node(resource: &ManagedResource, live: &Live) -> Node {
    let mut node = Node::new(
        &resource.group,
        &resource.version,
        &resource.kind,
        &resource.namespace,
        &resource.name,
        1,
    );
    node.managed = true;
    node.sync = Some(resource.sync());
    node.requires_pruning = resource.requires_pruning;
    node.hook = resource.hook;
    let object = live.get(
        &resource.group,
        &resource.kind,
        &resource.namespace,
        &resource.name,
    );
    node.exists = object.is_some();
    // Argo CD's own verdict when it persisted one, else the same checks on the live object.
    node.health = resource
        .health
        .as_ref()
        .filter(|h| !h.status.is_empty())
        .map(|h| (h.health(), h.message.clone()))
        .or_else(|| object.and_then(|o| health::assess(&resource.group, &resource.kind, o)))
        .or_else(|| {
            // Argo CD reports resources that should exist but don't as Missing.
            (!node.exists
                && !resource.requires_pruning
                && live.is_ready(&resource.group, &resource.kind, &resource.namespace))
            .then_some((Health::Missing, None))
        });
    if let Some(object) = object {
        node.info = info(&resource.group, &resource.kind, object);
        node.created = object
            .pointer("/metadata/creationTimestamp")
            .and_then(Value::as_str)
            .map(String::from);
    }
    node
}

fn add_children(
    out: &mut Vec<Node>,
    live: &Live,
    uid: &str,
    depth: usize,
    seen: &mut HashSet<String>,
) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let mut children: Vec<&(String, String, Arc<Value>)> = live.children(uid).iter().collect();
    children.sort_by(|a, b| {
        (kind_order(&a.1), text(&a.2, "/metadata/name"))
            .cmp(&(kind_order(&b.1), text(&b.2, "/metadata/name")))
    });
    let mut any = false;
    for (group, kind, object) in children {
        let name = text(object, "/metadata/name");
        let namespace = text(object, "/metadata/namespace");
        let mut node = Node::new(group, "", kind, namespace, name, depth);
        if !seen.insert(node.id.clone()) {
            continue;
        }
        any = true;
        node.live = true;
        node.exists = true;
        node.health = health::assess(group, kind, object);
        node.info = info(group, kind, object);
        node.created = object
            .pointer("/metadata/creationTimestamp")
            .and_then(Value::as_str)
            .map(String::from);
        let index = out.len();
        out.push(node);
        let child_uid = text(object, "/metadata/uid").to_string();
        if !child_uid.is_empty() && add_children(out, live, &child_uid, depth + 1, seen) {
            out[index].has_children = true;
        }
    }
    any
}

/// Kubernetes mode: the app, its managed resources, and their children from `live`.
pub fn build_kubernetes(app: &Application, live: &Live) -> Vec<Node> {
    let mut out = vec![root(app)];
    let mut resources: Vec<&ManagedResource> = app.status.resources.iter().collect();
    resources.sort_by(|a, b| {
        (kind_order(&a.kind), &a.kind, &a.name).cmp(&(kind_order(&b.kind), &b.kind, &b.name))
    });
    let mut seen: HashSet<String> = HashSet::new();
    for resource in resources {
        let node = managed_node(resource, live);
        if !seen.insert(node.id.clone()) {
            continue;
        }
        let uid = live
            .get(
                &resource.group,
                &resource.kind,
                &resource.namespace,
                &resource.name,
            )
            .map(|o| text(o, "/metadata/uid").to_string())
            .unwrap_or_default();
        let index = out.len();
        out.push(node);
        if !uid.is_empty() && add_children(&mut out, live, &uid, 2, &mut seen) {
            out[index].has_children = true;
        }
    }
    out[0].has_children = out.len() > 1;
    out
}

/// API mode: Argo CD's tree (every node with its parents), managed resources first.
pub fn build_api(app: &Application, tree: &ResourceTree) -> Vec<Node> {
    let managed: HashMap<String, &ManagedResource> = app
        .status
        .resources
        .iter()
        .map(|r| (node_id(&r.group, &r.kind, &r.namespace, &r.name), r))
        .collect();
    let by_uid: HashMap<&str, &crate::api::TreeNode> = tree
        .nodes
        .iter()
        .filter(|n| !n.node.uid.is_empty())
        .map(|n| (n.node.uid.as_str(), n))
        .collect();
    let mut children: HashMap<String, Vec<&crate::api::TreeNode>> = HashMap::new();
    let mut tops: Vec<&crate::api::TreeNode> = Vec::new();
    for node in &tree.nodes {
        let id = node_id(
            &node.node.group,
            &node.node.kind,
            &node.node.namespace,
            &node.node.name,
        );
        let parent = node
            .parent_refs
            .iter()
            .find(|p| by_uid.contains_key(p.uid.as_str()));
        match parent {
            Some(parent) if !managed.contains_key(&id) => {
                children.entry(parent.uid.clone()).or_default().push(node);
            }
            _ => tops.push(node),
        }
    }
    let order = |n: &&crate::api::TreeNode| {
        (
            kind_order(&n.node.kind),
            n.node.kind.clone(),
            n.node.name.clone(),
        )
    };
    tops.sort_by_key(order);
    for list in children.values_mut() {
        list.sort_by_key(order);
    }

    let mut out = vec![root(app)];
    let mut seen: HashSet<String> = HashSet::new();
    fn push(
        out: &mut Vec<Node>,
        node: &crate::api::TreeNode,
        depth: usize,
        managed: &HashMap<String, &ManagedResource>,
        children: &HashMap<String, Vec<&crate::api::TreeNode>>,
        seen: &mut HashSet<String>,
    ) {
        let n = &node.node;
        let mut row = Node::new(&n.group, &n.version, &n.kind, &n.namespace, &n.name, depth);
        if !seen.insert(format!("{}|{}", row.id, n.uid)) || depth > MAX_DEPTH {
            return;
        }
        row.exists = true;
        row.health = node
            .health
            .as_ref()
            .filter(|h| !h.status.is_empty())
            .map(|h| (h.health(), h.message.clone()));
        row.info = node
            .info
            .iter()
            .map(|i| i.value.clone())
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        row.created = node.created_at.clone();
        if let Some(resource) = managed.get(&row.id) {
            row.managed = true;
            row.sync = Some(resource.sync());
            row.requires_pruning = resource.requires_pruning;
            row.hook = resource.hook;
            if row.health.is_none() {
                row.health = resource
                    .health
                    .as_ref()
                    .filter(|h| !h.status.is_empty())
                    .map(|h| (h.health(), h.message.clone()));
            }
        }
        let index = out.len();
        out.push(row);
        if let Some(kids) = children.get(&n.uid) {
            for kid in kids {
                push(out, kid, depth + 1, managed, children, seen);
            }
            out[index].has_children = out.len() > index + 1;
        }
    }
    for node in tops {
        push(&mut out, node, 1, &managed, &children, &mut seen);
    }
    // Managed resources Argo CD didn't put in the tree (missing ones).
    for (id, resource) in &managed {
        if !out.iter().any(|n| &n.id == id) {
            let mut row = managed_node(resource, &Live::default());
            row.health = row.health.or(Some((Health::Missing, None)));
            out.push(row);
        }
    }
    out[0].has_children = out.len() > 1;
    out
}

/// Which rows the tree shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    All,
    OutOfSync,
    Unhealthy,
}

/// The rows to render: collapsed subtrees hidden; with a filter, matching rows and their
/// ancestors.
pub fn visible(nodes: &[Node], collapsed: &HashSet<String>, show: Show, query: &str) -> Vec<Node> {
    let query = query.trim();
    let filtering = show != Show::All || !query.is_empty();
    let keep: Vec<bool> = if filtering {
        let matches = |n: &Node| {
            let status = match show {
                Show::All => true,
                Show::OutOfSync => n.sync == Some(SyncStatus::OutOfSync) || n.requires_pruning,
                Show::Unhealthy => {
                    n.unhealthy() || matches!(n.health, Some((Health::Progressing, _)))
                }
            };
            status && (query.is_empty() || n.matches(query))
        };
        let mut keep = vec![false; nodes.len()];
        for (i, node) in nodes.iter().enumerate() {
            if matches(node) {
                keep[i] = true;
                // Ancestors: the nearest earlier nodes with smaller depth.
                let mut depth = node.depth;
                for j in (0..i).rev() {
                    if nodes[j].depth < depth {
                        keep[j] = true;
                        depth = nodes[j].depth;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
        }
        keep
    } else {
        vec![true; nodes.len()]
    };
    let mut out = Vec::new();
    let mut hidden_below: Option<usize> = None;
    for (i, node) in nodes.iter().enumerate() {
        if let Some(depth) = hidden_below {
            if node.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        if !keep[i] {
            continue;
        }
        out.push(node.clone());
        if collapsed.contains(&node.id) && !filtering {
            hidden_below = Some(node.depth);
        }
    }
    out
}

/// The list view: every resource once, managed first, by kind and name.
pub fn flat(nodes: &[Node]) -> Vec<Node> {
    let mut out: Vec<Node> = nodes
        .iter()
        .filter(|n| !n.is_app())
        .map(|n| Node {
            depth: 0,
            has_children: false,
            ..n.clone()
        })
        .collect();
    out.sort_by(|a, b| {
        (!a.managed, kind_order(&a.kind), &a.kind, &a.name).cmp(&(
            !b.managed,
            kind_order(&b.kind),
            &b.kind,
            &b.name,
        ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::guestbook;
    use serde_json::json;

    fn live() -> Live {
        let mut live = Live::default();
        live.insert(
            "apps",
            "Deployment",
            Arc::new(json!({"apiVersion": "apps/v1", "kind": "Deployment",
                "metadata": {"name": "guestbook-ui", "namespace": "guestbook", "uid": "d1", "generation": 1,
                             "annotations": {"deployment.kubernetes.io/revision": "2"}},
                "spec": {"replicas": 1},
                "status": {"observedGeneration": 1, "replicas": 1, "updatedReplicas": 1, "availableReplicas": 1, "readyReplicas": 1}})),
        );
        live.insert(
            "",
            "Service",
            Arc::new(json!({"apiVersion": "v1", "kind": "Service",
                "metadata": {"name": "guestbook-ui", "namespace": "guestbook", "uid": "s1"},
                "spec": {"type": "ClusterIP", "clusterIP": "10.96.1.2"}})),
        );
        live.insert(
            "apps",
            "ReplicaSet",
            Arc::new(json!({"apiVersion": "apps/v1", "kind": "ReplicaSet",
                "metadata": {"name": "guestbook-ui-abc", "namespace": "guestbook", "uid": "r1", "generation": 1,
                             "ownerReferences": [{"kind": "Deployment", "name": "guestbook-ui", "uid": "d1"}]},
                "spec": {"replicas": 1}, "status": {"observedGeneration": 1, "replicas": 1, "availableReplicas": 1}})),
        );
        live.insert(
            "",
            "Pod",
            Arc::new(json!({"apiVersion": "v1", "kind": "Pod",
                "metadata": {"name": "guestbook-ui-abc-x1", "namespace": "guestbook", "uid": "p1",
                             "ownerReferences": [{"kind": "ReplicaSet", "name": "guestbook-ui-abc", "uid": "r1"}]},
                "spec": {"containers": [{"name": "ui"}]},
                "status": {"phase": "Running", "podIP": "10.244.1.9",
                           "conditions": [{"type": "Ready", "status": "True"}],
                           "containerStatuses": [{"name": "ui", "ready": true, "state": {"running": {}}}]}})),
        );
        live
    }

    #[test]
    fn kubernetes_tree_follows_owner_references() {
        let app = Application::parse(&guestbook()).unwrap();
        let nodes = build_kubernetes(&app, &live());
        let shape: Vec<(usize, &str, &str)> = nodes
            .iter()
            .map(|n| (n.depth, n.kind.as_str(), n.name.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                (0, "Application", "guestbook"),
                (1, "Service", "guestbook-ui"),
                (1, "Deployment", "guestbook-ui"),
                (2, "ReplicaSet", "guestbook-ui-abc"),
                (3, "Pod", "guestbook-ui-abc-x1"),
            ]
        );
        let deployment = &nodes[2];
        assert!(deployment.managed && deployment.has_children && !deployment.live);
        assert_eq!(deployment.sync, Some(SyncStatus::OutOfSync));
        assert_eq!(deployment.health.as_ref().unwrap().0, Health::Healthy);
        assert_eq!(deployment.info, "1/1 ready · rev 2");
        let pod = &nodes[4];
        assert!(pod.live && !pod.managed);
        assert_eq!(pod.health.as_ref().unwrap().0, Health::Healthy);
        assert_eq!(pod.info, "Running · 10.244.1.9");
        assert_eq!(nodes[1].info, "ClusterIP 10.96.1.2");
    }

    #[test]
    fn missing_resources_are_missing() {
        let app = Application::parse(&guestbook()).unwrap();
        let mut live = Live::default();
        live.insert(
            "",
            "Service",
            Arc::new(json!({"kind": "Service", "metadata": {"name": "guestbook-ui", "namespace": "guestbook", "uid": "s1"}, "spec": {}})),
        );
        // Not listed yet: no verdict.
        let nodes = build_kubernetes(&app, &live);
        let deployment = nodes.iter().find(|n| n.kind == "Deployment").unwrap();
        assert!(!deployment.exists && deployment.health.is_none());
        live.mark_ready("apps", "Deployment", "guestbook");
        let nodes = build_kubernetes(&app, &live);
        let deployment = nodes.iter().find(|n| n.kind == "Deployment").unwrap();
        assert_eq!(deployment.health.as_ref().unwrap().0, Health::Missing);
    }

    #[test]
    fn collapsing_and_filtering() {
        let app = Application::parse(&guestbook()).unwrap();
        let nodes = build_kubernetes(&app, &live());
        let deployment_id = nodes[2].id.clone();
        let collapsed: HashSet<String> = [deployment_id].into();
        let rows = visible(&nodes, &collapsed, Show::All, "");
        assert_eq!(rows.len(), 3);
        // Filtering shows matches with their ancestors, ignoring collapse.
        let rows = visible(&nodes, &collapsed, Show::All, "x1");
        let names: Vec<&str> = rows.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(names, ["Application", "Deployment", "ReplicaSet", "Pod"]);
        let rows = visible(&nodes, &HashSet::new(), Show::OutOfSync, "");
        let names: Vec<&str> = rows.iter().map(|n| n.kind.as_str()).collect();
        assert_eq!(names, ["Application", "Deployment"]);
        let list = flat(&nodes);
        assert_eq!(list.len(), 4);
        assert!(list[0].managed && list.iter().all(|n| n.depth == 0));
    }

    #[test]
    fn api_tree_uses_parent_refs() {
        let app = Application::parse(&guestbook()).unwrap();
        let tree: ResourceTree = serde_json::from_value(json!({"nodes": [
            {"version": "v1", "kind": "Service", "namespace": "guestbook", "name": "guestbook-ui", "uid": "s1", "health": {"status": "Healthy"}},
            {"group": "apps", "version": "v1", "kind": "Deployment", "namespace": "guestbook", "name": "guestbook-ui", "uid": "d1", "health": {"status": "Healthy"}},
            {"group": "apps", "version": "v1", "kind": "ReplicaSet", "namespace": "guestbook", "name": "guestbook-ui-abc", "uid": "r1",
             "parentRefs": [{"group": "apps", "kind": "Deployment", "namespace": "guestbook", "name": "guestbook-ui", "uid": "d1"}],
             "info": [{"name": "Revision", "value": "Rev:2"}], "health": {"status": "Healthy"}},
            {"version": "v1", "kind": "Pod", "namespace": "guestbook", "name": "guestbook-ui-abc-x1", "uid": "p1",
             "parentRefs": [{"group": "apps", "kind": "ReplicaSet", "namespace": "guestbook", "name": "guestbook-ui-abc", "uid": "r1"}],
             "info": [{"name": "Status Reason", "value": "Running"}], "health": {"status": "Healthy"}}
        ]}))
        .unwrap();
        let nodes = build_api(&app, &tree);
        let shape: Vec<(usize, &str)> = nodes.iter().map(|n| (n.depth, n.kind.as_str())).collect();
        assert_eq!(
            shape,
            [
                (0, "Application"),
                (1, "Service"),
                (1, "Deployment"),
                (2, "ReplicaSet"),
                (3, "Pod")
            ]
        );
        assert_eq!(nodes[3].info, "Rev:2");
        assert!(nodes[2].managed && !nodes[4].managed);
    }

    #[test]
    fn info_never_shows_secret_values() {
        let secret = json!({"kind": "Secret", "data": {"password": "aHVudGVyMg=="}});
        assert_eq!(info("", "Secret", &secret), "1 key");
    }
}
