//! Builds the tree from cached objects, the way k9s's XRay does: Namespace → workload (or
//! Service) → Pods → Containers, with the Secrets, ConfigMaps, volume claims and service
//! account a pod refers to. Workloads find their pods by label selector; roots without pods are
//! left out.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::filter::selector_matches;
use crate::{Health, Node, Root};

/// The cached objects of one cluster (already limited to the namespace being looked at).
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub pods: Vec<Arc<Value>>,
    pub services: Vec<Arc<Value>>,
    pub deployments: Vec<Arc<Value>>,
    pub replica_sets: Vec<Arc<Value>>,
    pub stateful_sets: Vec<Arc<Value>>,
    pub daemon_sets: Vec<Arc<Value>>,
    pub config_maps: Vec<Arc<Value>>,
    pub secrets: Vec<Arc<Value>>,
    pub claims: Vec<Arc<Value>>,
    pub service_accounts: Vec<Arc<Value>>,
}

fn s<'a>(v: &'a Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer)?.as_str().filter(|x| !x.is_empty())
}

fn name(v: &Value) -> String {
    s(v, "/metadata/name").unwrap_or_default().to_string()
}

fn namespace(v: &Value) -> Option<String> {
    s(v, "/metadata/namespace").map(str::to_string)
}

fn list<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn int(v: &Value, pointer: &str) -> i64 {
    v.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

/// Builds the tree for `root`: Namespace nodes with the workloads (or Services) below them.
/// `labels`: a label selector the starting objects must satisfy.
pub fn build(root: Root, inputs: &Inputs, labels: Option<&str>) -> Vec<Node> {
    let ctx = Ctx { inputs };
    let wanted =
        |o: &&Arc<Value>| labels.is_none_or(|l| selector_matches(l, o.pointer("/metadata/labels")));
    let nodes: Vec<Node> = match root {
        Root::Pods => inputs
            .pods
            .iter()
            .filter(wanted)
            .map(|p| ctx.pod(p))
            .collect(),
        Root::Services => inputs
            .services
            .iter()
            .filter(wanted)
            .map(|v| ctx.service(v))
            .collect(),
        Root::Deployments => inputs
            .deployments
            .iter()
            .filter(wanted)
            .map(|d| {
                let (a, r, u) = (
                    int(d, "/status/availableReplicas"),
                    int(d, "/spec/replicas"),
                    int(d, "/status/unavailableReplicas"),
                );
                let toast = a != r || u != 0;
                ctx.workload(
                    d,
                    "Deployment",
                    "deployments",
                    toast,
                    format!("{a}/{r}/{u}"),
                )
            })
            .collect(),
        Root::ReplicaSets => inputs
            .replica_sets
            .iter()
            .filter(wanted)
            .map(|o| {
                let (a, r) = (int(o, "/status/replicas"), int(o, "/spec/replicas"));
                ctx.workload(o, "ReplicaSet", "replicasets", a != r, format!("{a}/{r}"))
            })
            .collect(),
        Root::StatefulSets => inputs
            .stateful_sets
            .iter()
            .filter(wanted)
            .map(|o| {
                let (a, r) = (int(o, "/status/currentReplicas"), int(o, "/spec/replicas"));
                ctx.workload(o, "StatefulSet", "statefulsets", a != r, format!("{a}/{r}"))
            })
            .collect(),
        Root::DaemonSets => inputs
            .daemon_sets
            .iter()
            .filter(wanted)
            .map(|o| {
                let (a, d) = (
                    int(o, "/status/numberAvailable"),
                    int(o, "/status/desiredNumberScheduled"),
                );
                ctx.workload(o, "DaemonSet", "daemonsets", a != d, format!("{a}/{d}"))
            })
            .collect(),
    };
    let mut by_namespace: BTreeMap<String, Vec<Node>> = BTreeMap::new();
    for node in nodes.into_iter().filter(|n| !n.children.is_empty()) {
        by_namespace
            .entry(node.namespace.clone().unwrap_or_default())
            .or_default()
            .push(node);
    }
    by_namespace
        .into_iter()
        .map(|(ns, mut children)| {
            children.sort_by(|a, b| a.name.cmp(&b.name));
            Node {
                kind: "Namespace",
                group: "",
                resource: "",
                namespace: None,
                name: ns,
                health: Health::Neutral,
                info: String::new(),
                children,
            }
        })
        .collect()
}

struct Ctx<'a> {
    inputs: &'a Inputs,
}

/// Whether `labels` satisfy a `LabelSelector` (`matchLabels` and `matchExpressions`).
fn selects(selector: &Value, labels: Option<&Value>) -> bool {
    let label = |k: &str| labels.and_then(|l| l.get(k)).and_then(Value::as_str);
    if let Some(m) = selector.get("matchLabels").and_then(Value::as_object)
        && !m.iter().all(|(k, v)| label(k) == v.as_str())
    {
        return false;
    }
    list(selector, "/matchExpressions").iter().all(|e| {
        let key = s(e, "/key").unwrap_or_default();
        let values: Vec<&str> = list(e, "/values")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        match s(e, "/operator") {
            Some("In") => label(key).is_some_and(|v| values.contains(&v)),
            Some("NotIn") => label(key).is_none_or(|v| !values.contains(&v)),
            Some("Exists") => label(key).is_some(),
            Some("DoesNotExist") => label(key).is_none(),
            _ => false,
        }
    })
}

impl Ctx<'_> {
    fn pods_matching(
        &self,
        ns: &Option<String>,
        matches: impl Fn(Option<&Value>) -> bool,
    ) -> Vec<Node> {
        let mut pods: Vec<Node> = self
            .inputs
            .pods
            .iter()
            .filter(|p| &namespace(p) == ns && matches(p.pointer("/metadata/labels")))
            .map(|p| self.pod(p))
            .collect();
        pods.sort_by(|a, b| a.name.cmp(&b.name));
        pods
    }

    fn workload(
        &self,
        o: &Value,
        kind: &'static str,
        resource: &'static str,
        toast: bool,
        info: String,
    ) -> Node {
        let selector = o.pointer("/spec/selector").cloned().unwrap_or(Value::Null);
        let ns = namespace(o);
        // An empty selector would match every pod of the namespace: treat it as none.
        let empty = selector
            .get("matchLabels")
            .and_then(Value::as_object)
            .is_none_or(|m| m.is_empty())
            && list(&selector, "/matchExpressions").is_empty();
        let children = if empty {
            Vec::new()
        } else {
            self.pods_matching(&ns, |labels| selects(&selector, labels))
        };
        Node {
            kind,
            group: if kind == "ReplicaSet"
                || kind == "Deployment"
                || kind == "StatefulSet"
                || kind == "DaemonSet"
            {
                "apps"
            } else {
                ""
            },
            resource,
            namespace: ns,
            name: name(o),
            health: if toast { Health::Bad } else { Health::Ok },
            info,
            children,
        }
    }

    fn service(&self, svc: &Value) -> Node {
        let selector: Vec<(&str, &str)> = svc
            .pointer("/spec/selector")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.as_str(), v.as_str()?)))
                    .collect()
            })
            .unwrap_or_default();
        let ns = namespace(svc);
        let children = if selector.is_empty() {
            Vec::new()
        } else {
            self.pods_matching(&ns, |labels| {
                selector
                    .iter()
                    .all(|(k, v)| labels.and_then(|l| l.get(*k)).and_then(Value::as_str) == Some(v))
            })
        };
        Node {
            kind: "Service",
            group: "",
            resource: "services",
            namespace: ns,
            name: name(svc),
            health: Health::Ok,
            info: String::new(),
            children,
        }
    }

    /// A reference to an object that may be cached. A missing one is bad unless optional.
    fn reference(
        &self,
        kind: &'static str,
        resource: &'static str,
        known: &[Arc<Value>],
        ns: &Option<String>,
        target: &str,
        optional: bool,
    ) -> Node {
        let exists = known
            .iter()
            .any(|o| name(o) == target && &namespace(o) == ns);
        let (health, info) = match (exists, optional) {
            (true, _) => (Health::Ok, String::new()),
            (false, true) => (Health::Neutral, String::new()),
            (false, false) => (Health::Bad, "missing".to_string()),
        };
        Node {
            kind,
            group: "",
            resource,
            namespace: ns.clone(),
            name: target.to_string(),
            health,
            info,
            children: Vec::new(),
        }
    }

    fn secret_ref(&self, ns: &Option<String>, target: &str, optional: bool) -> Node {
        self.reference(
            "Secret",
            "secrets",
            &self.inputs.secrets,
            ns,
            target,
            optional,
        )
    }

    fn config_map_ref(&self, ns: &Option<String>, target: &str, optional: bool) -> Node {
        self.reference(
            "ConfigMap",
            "configmaps",
            &self.inputs.config_maps,
            ns,
            target,
            optional,
        )
    }

    fn pod(&self, p: &Value) -> Node {
        let ns = namespace(p);
        let mut children: Vec<Node> = Vec::new();
        for key in ["/spec/initContainers", "/spec/containers"] {
            for c in list(p, key) {
                children.push(self.container(c, &ns));
            }
        }
        let mut refs: Vec<Node> = Vec::new();
        let mut add = |node: Node| {
            if !refs
                .iter()
                .any(|n| n.kind == node.kind && n.name == node.name)
            {
                refs.push(node);
            }
        };
        for v in list(p, "/spec/volumes") {
            let optional =
                |pointer: &str| v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false);
            if let Some(n) = s(v, "/secret/secretName") {
                add(self.secret_ref(&ns, n, optional("/secret/optional")));
            } else if let Some(n) = s(v, "/configMap/name") {
                add(self.config_map_ref(&ns, n, optional("/configMap/optional")));
            } else if let Some(n) = s(v, "/persistentVolumeClaim/claimName") {
                add(self.reference(
                    "PersistentVolumeClaim",
                    "persistentvolumeclaims",
                    &self.inputs.claims,
                    &ns,
                    n,
                    false,
                ));
            }
        }
        children.extend(refs);
        if let Some(sa) = s(p, "/spec/serviceAccountName") {
            children.push(self.service_account(sa, &ns, p));
        }

        let statuses = list(p, "/status/containerStatuses");
        let ready = statuses
            .iter()
            .filter(|c| c.get("ready").and_then(Value::as_bool) == Some(true))
            .count();
        let completed = s(p, "/status/phase") == Some("Succeeded");
        Node {
            kind: "Pod",
            group: "",
            resource: "pods",
            namespace: ns,
            name: name(p),
            health: if completed {
                Health::Neutral
            } else if ready == statuses.len() {
                Health::Ok
            } else {
                Health::Bad
            },
            info: format!("{ready}/{}", statuses.len()),
            children,
        }
    }

    /// A container with the ConfigMaps and Secrets its environment refers to.
    fn container(&self, c: &Value, ns: &Option<String>) -> Node {
        let mut refs: Vec<Node> = Vec::new();
        let mut add = |node: Node| {
            if !refs
                .iter()
                .any(|n| n.kind == node.kind && n.name == node.name)
            {
                refs.push(node);
            }
        };
        let optional =
            |v: &Value, pointer: &str| v.pointer(pointer).and_then(Value::as_bool).unwrap_or(false);
        for e in list(c, "/env") {
            if let Some(n) = s(e, "/valueFrom/secretKeyRef/name") {
                add(self.secret_ref(ns, n, optional(e, "/valueFrom/secretKeyRef/optional")));
            }
            if let Some(n) = s(e, "/valueFrom/configMapKeyRef/name") {
                add(self.config_map_ref(ns, n, optional(e, "/valueFrom/configMapKeyRef/optional")));
            }
        }
        for e in list(c, "/envFrom") {
            if let Some(n) = s(e, "/configMapRef/name") {
                add(self.config_map_ref(ns, n, optional(e, "/configMapRef/optional")));
            }
            if let Some(n) = s(e, "/secretRef/name") {
                add(self.secret_ref(ns, n, optional(e, "/secretRef/optional")));
            }
        }
        Node {
            kind: "Container",
            group: "",
            resource: "",
            namespace: None,
            name: s(c, "/name").unwrap_or_default().to_string(),
            health: Health::Neutral,
            info: String::new(),
            children: refs,
        }
    }

    fn service_account(&self, sa_name: &str, ns: &Option<String>, pod: &Value) -> Node {
        let Some(sa) = self
            .inputs
            .service_accounts
            .iter()
            .find(|o| name(o) == sa_name && &namespace(o) == ns)
        else {
            return self.reference("ServiceAccount", "serviceaccounts", &[], ns, sa_name, false);
        };
        // The pod's own setting wins over the account's.
        let automount = pod
            .pointer("/spec/automountServiceAccountToken")
            .and_then(Value::as_bool)
            .or_else(|| {
                sa.pointer("/automountServiceAccountToken")
                    .and_then(Value::as_bool)
            });
        let mut children: Vec<Node> = Vec::new();
        for key in ["/secrets", "/imagePullSecrets"] {
            for r in list(sa, key) {
                if let Some(n) = s(r, "/name")
                    && !children.iter().any(|c| c.name == n)
                {
                    children.push(self.secret_ref(ns, n, false));
                }
            }
        }
        Node {
            kind: "ServiceAccount",
            group: "",
            resource: "serviceaccounts",
            namespace: ns.clone(),
            name: sa_name.to_string(),
            health: Health::Ok,
            info: automount.map_or(String::new(), |a| format!("automount={a}")),
            children,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn arc(v: Value) -> Arc<Value> {
        Arc::new(v)
    }

    fn inputs() -> Inputs {
        Inputs {
            deployments: vec![arc(json!({
                "metadata": {"name": "web", "namespace": "a", "uid": "d1"},
                "spec": {"replicas": 2, "selector": {"matchLabels": {"app": "web"}}},
                "status": {"availableReplicas": 1, "unavailableReplicas": 1}
            }))],
            pods: vec![
                arc(json!({
                    "metadata": {"name": "web-1-x", "namespace": "a", "labels": {"app": "web"}},
                    "spec": {"serviceAccountName": "web",
                        "containers": [{"name": "app",
                            "envFrom": [{"secretRef": {"name": "creds"}},
                                {"configMapRef": {"name": "opt", "optional": true}}]}],
                        "volumes": [{"configMap": {"name": "cfg"}}]},
                    "status": {"phase": "Running", "containerStatuses": [{"ready": true}]}
                })),
                arc(json!({
                    "metadata": {"name": "web-1-y", "namespace": "a", "labels": {"app": "web"}},
                    "spec": {"containers": [{"name": "app"}]},
                    "status": {"phase": "Running", "containerStatuses": [{"ready": false}]}
                })),
                arc(json!({
                    "metadata": {"name": "other", "namespace": "a", "labels": {"app": "x"}},
                    "spec": {"containers": [{"name": "app"}]},
                    "status": {"phase": "Running", "containerStatuses": [{"ready": true}]}
                })),
            ],
            services: vec![
                arc(json!({
                    "metadata": {"name": "web", "namespace": "a"},
                    "spec": {"selector": {"app": "web"}}
                })),
                arc(json!({
                    "metadata": {"name": "lonely", "namespace": "a"},
                    "spec": {"selector": {"app": "none"}}
                })),
            ],
            config_maps: vec![arc(json!({"metadata": {"name": "cfg", "namespace": "a"}}))],
            service_accounts: vec![arc(json!({
                "metadata": {"name": "web", "namespace": "a"},
                "automountServiceAccountToken": false,
                "imagePullSecrets": [{"name": "pull"}]
            }))],
            ..Inputs::default()
        }
    }

    #[test]
    fn deployment_tree_goes_straight_to_pods() {
        let tree = build(Root::Deployments, &inputs(), None);
        assert_eq!(tree.len(), 1);
        let ns = &tree[0];
        assert_eq!((ns.kind, ns.name.as_str()), ("Namespace", "a"));
        let d = &ns.children[0];
        assert_eq!(d.info, "1/2/1");
        assert_eq!(d.health, Health::Bad);
        let pods: Vec<_> = d
            .children
            .iter()
            .map(|p| (p.name.as_str(), p.info.as_str()))
            .collect();
        assert_eq!(pods, [("web-1-x", "1/1"), ("web-1-y", "0/1")]);
        assert_eq!(d.children[1].health, Health::Bad);
        let ok = &d.children[0];
        let kinds: Vec<_> = ok
            .children
            .iter()
            .map(|c| (c.kind, c.name.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("Container", "app"),
                ("ConfigMap", "cfg"),
                ("ServiceAccount", "web")
            ]
        );
        // Env refs hang under the container; an optional ConfigMap isn't flagged.
        let container = &ok.children[0];
        assert_eq!(container.children.len(), 2);
        assert_eq!(container.children[0].info, "missing");
        assert_eq!(container.children[1].info, "");
        assert_eq!(container.children[1].health, Health::Neutral);
        // The service account shows automount and its pull secrets.
        let sa = &ok.children[2];
        assert_eq!(sa.info, "automount=false");
        assert_eq!(sa.children[0].name, "pull");
    }

    #[test]
    fn services_without_pods_are_left_out() {
        let tree = build(Root::Services, &inputs(), None);
        let names: Vec<_> = tree[0].children.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["web"]);
        assert_eq!(tree[0].children[0].children.len(), 2);
    }

    #[test]
    fn empty_workload_selector_matches_nothing() {
        let mut i = inputs();
        i.deployments = vec![arc(json!({
            "metadata": {"name": "odd", "namespace": "a"},
            "spec": {"replicas": 1, "selector": {}}
        }))];
        assert!(build(Root::Deployments, &i, None).is_empty());
    }

    #[test]
    fn selector_expressions() {
        let sel =
            json!({"matchExpressions": [{"key": "tier", "operator": "NotIn", "values": ["db"]}]});
        assert!(selects(&sel, Some(&json!({"tier": "web"}))));
        assert!(selects(&sel, None));
        assert!(!selects(&sel, Some(&json!({"tier": "db"}))));
    }

    #[test]
    fn parses_names() {
        assert_eq!(Root::parse("dp"), Some(Root::Deployments));
        assert_eq!(Root::parse("STS"), Some(Root::StatefulSets));
        assert_eq!(Root::parse("cm"), None);
    }
}
