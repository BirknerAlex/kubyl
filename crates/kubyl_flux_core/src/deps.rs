//! `spec.dependsOn` as a graph: what an object waits for and what waits for it.
//!
//! A dependency holds its dependents back the way kustomize-controller and helm-controller check
//! it ([`FluxObject::ready_for_dependents`]): it exists, its controller has seen its latest
//! generation and Ready is True. A suspended or reconciling dependency that is Ready doesn't
//! block. A dependency with a `readyExpr` (CEL, Flux 2.7+) is checked by that expression, which
//! Kubyl doesn't evaluate: it never counts as blocking here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::model::{FluxObject, ObjectRef, State};

/// A dependency as the details show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// `namespace/name`.
    pub key: String,
    /// The dependency's state; `None` when it doesn't exist (or isn't loaded).
    pub state: Option<State>,
    /// It passes Flux's dependency check (exists, generation observed, Ready=True).
    pub ready: bool,
    /// The reference has a `readyExpr`: the controller decides with that expression.
    pub custom_check: bool,
}

impl Dependency {
    /// The object waits for it.
    pub fn blocks(&self) -> bool {
        !self.ready && !self.custom_check
    }
}

#[derive(Clone, Debug)]
struct Node {
    state: State,
    ready: bool,
}

/// The dependency graph of objects of one kind (dependsOn only names objects of the same kind).
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// key → its dependencies' keys, and whether each has a `readyExpr`.
    edges: BTreeMap<String, Vec<(String, bool)>>,
    nodes: BTreeMap<String, Node>,
}

impl Graph {
    pub fn new<'a>(objects: impl IntoIterator<Item = &'a FluxObject>) -> Self {
        let mut graph = Graph::default();
        for object in objects {
            graph.nodes.insert(
                object.key(),
                Node {
                    state: object.state(),
                    ready: object.ready_for_dependents(),
                },
            );
            graph.edges.insert(object.key(), edges_of(object));
        }
        graph
    }

    /// What `key` depends on, with their states.
    pub fn dependencies(&self, key: &str) -> Vec<Dependency> {
        self.edges
            .get(key)
            .into_iter()
            .flatten()
            .map(|(dep, custom_check)| {
                let node = self.nodes.get(dep);
                Dependency {
                    key: dep.clone(),
                    state: node.map(|n| n.state),
                    ready: node.is_some_and(|n| n.ready),
                    custom_check: *custom_check,
                }
            })
            .collect()
    }

    /// What depends on `key` (directly).
    pub fn dependents(&self, key: &str) -> Vec<String> {
        self.edges
            .iter()
            .filter(|(_, deps)| deps.iter().any(|(d, _)| d == key))
            .map(|(k, _)| k.clone())
            .collect()
    }

    /// The first dependency (depth-first through the chain) that holds `key` back: "waiting for
    /// X". Whether the object itself waits is [`Graph::waiting`].
    pub fn waiting_for(&self, key: &str) -> Option<Dependency> {
        let mut seen = BTreeSet::new();
        self.waiting_inner(key, &mut seen)
    }

    /// What `object` waits for, only while it isn't Ready or suspended itself (a Ready object's
    /// dependency may have failed since: the object doesn't wait for anything).
    pub fn waiting(&self, object: &FluxObject) -> Option<Dependency> {
        if matches!(object.state(), State::Ready | State::Suspended) {
            return None;
        }
        self.waiting_for(&object.key())
    }

    fn waiting_inner(&self, key: &str, seen: &mut BTreeSet<String>) -> Option<Dependency> {
        if !seen.insert(key.to_string()) {
            return None;
        }
        for dep in self.dependencies(key) {
            if dep.blocks() {
                // The root cause: a dependency that waits itself points further down.
                return self.waiting_inner(&dep.key, seen).or(Some(dep));
            }
        }
        None
    }
}

/// `dependsOn` keys of `object`, each with whether it has a `readyExpr`.
fn edges_of(object: &FluxObject) -> Vec<(String, bool)> {
    object
        .raw
        .pointer("/spec/dependsOn")
        .and_then(Value::as_array)
        .map(|deps| {
            deps.iter()
                .filter_map(|d| {
                    let reference = ObjectRef::parse(d, &object.namespace, Some(object.kind))?;
                    let custom = d
                        .get("readyExpr")
                        .and_then(Value::as_str)
                        .is_some_and(|e| !e.is_empty());
                    Some((reference.key(), custom))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use serde_json::json;
    use std::sync::Arc;

    fn ks(name: &str, deps: &[&str], ready: bool) -> FluxObject {
        let deps: Vec<_> = deps.iter().map(|d| json!({"name": d})).collect();
        let object = json!({"apiVersion": "kustomize.toolkit.fluxcd.io/v1", "kind": "Kustomization",
            "metadata": {"name": name, "namespace": "flux-demo"},
            "spec": {"dependsOn": deps},
            "status": {"conditions": [{"type": "Ready", "status": if ready { "True" } else { "False" },
                "message": "m"}]}});
        FluxObject::parse(&Arc::new(object)).unwrap()
    }

    #[test]
    fn waiting_for_the_root_cause() {
        let objects = [
            ks("infra", &[], true),
            ks("apps", &["infra"], true),
            ks("broken", &[], false),
            ks("apps-late", &["broken"], false),
            ks("frontend", &["apps-late", "apps"], false),
        ];
        let graph = Graph::new(&objects);
        assert_eq!(graph.waiting_for("flux-demo/apps"), None);
        assert_eq!(
            graph.waiting_for("flux-demo/apps-late").unwrap().key,
            "flux-demo/broken"
        );
        // Through the chain to the object that actually fails.
        assert_eq!(
            graph.waiting_for("flux-demo/frontend").unwrap().key,
            "flux-demo/broken"
        );
        assert_eq!(graph.dependents("flux-demo/infra"), ["flux-demo/apps"]);
        let deps = graph.dependencies("flux-demo/frontend");
        assert_eq!(deps.len(), 2);
        assert!(deps[0].blocks() && !deps[1].blocks());
    }

    #[test]
    fn missing_dependencies_and_cycles() {
        let objects = [
            ks("a", &["b"], false),
            ks("b", &["a"], false),
            ks("c", &["gone"], false),
        ];
        let graph = Graph::new(&objects);
        let missing = graph.waiting_for("flux-demo/c").unwrap();
        assert_eq!(missing.state, None);
        assert!(missing.blocks());
        // A cycle doesn't loop forever.
        assert!(graph.waiting_for("flux-demo/a").is_some());
        let fixture = FluxObject::parse(&Arc::new(fixtures::kustomization_waiting())).unwrap();
        assert_eq!(
            Graph::new([&fixture])
                .waiting_for("flux-demo/apps-late")
                .unwrap()
                .key,
            "flux-demo/broken"
        );
    }

    /// Flux checks only the dependency's Ready condition and observed generation.
    #[test]
    fn suspended_or_reconciling_dependencies_that_are_ready_dont_block() {
        let mut suspended = ks("infra", &[], true);
        suspended.suspended = true;
        let mut pending = ks("db", &[], true);
        pending.requested_at = Some("2026-10-07T11:00:00Z".into());
        assert_eq!(suspended.state(), State::Suspended);
        assert_eq!(pending.state(), State::Reconciling);
        let app = ks("app", &["infra", "db"], false);
        let graph = Graph::new([&suspended, &pending, &app]);
        assert_eq!(graph.waiting_for("flux-demo/app"), None);
        // A newer generation the controller hasn't seen yet blocks, though Ready is True.
        let mut behind = ks("infra", &[], true);
        behind.generation = Some(3);
        behind.observed_generation = Some(2);
        let graph = Graph::new([&behind, &app]);
        assert_eq!(
            graph.waiting_for("flux-demo/app").unwrap().key,
            "flux-demo/infra"
        );
        // No conditions yet: not ready.
        let fresh = FluxObject::parse(&Arc::new(
            json!({"apiVersion": "kustomize.toolkit.fluxcd.io/v1",
            "kind": "Kustomization", "metadata": {"name": "infra", "namespace": "flux-demo"}}),
        ))
        .unwrap();
        assert!(
            Graph::new([&fresh, &app])
                .waiting_for("flux-demo/app")
                .is_some()
        );
    }

    #[test]
    fn ready_objects_and_ready_expressions_dont_wait() {
        let broken = ks("broken", &[], false);
        // Ready itself (the dependency failed later): not waiting.
        let ready = ks("apps", &["broken"], true);
        let graph = Graph::new([&broken, &ready]);
        assert!(graph.waiting_for("flux-demo/apps").is_some());
        assert_eq!(graph.waiting(&ready), None);
        // A readyExpr decides instead of Ready: Kubyl doesn't evaluate CEL.
        let custom = FluxObject::parse(&Arc::new(json!({"apiVersion": "kustomize.toolkit.fluxcd.io/v1",
            "kind": "Kustomization", "metadata": {"name": "custom", "namespace": "flux-demo"},
            "spec": {"dependsOn": [{"name": "broken", "readyExpr": "dep.status.observedGeneration > 0"}]},
            "status": {"conditions": [{"type": "Ready", "status": "False", "message": "m"}]}})))
        .unwrap();
        let graph = Graph::new([&broken, &custom]);
        let deps = graph.dependencies("flux-demo/custom");
        assert!(deps[0].custom_check && !deps[0].blocks());
        assert_eq!(graph.waiting(&custom), None);
    }
}
