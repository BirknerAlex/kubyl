//! Background watches every connected cluster runs: namespaces and CRDs.
//!
//! Both run as futures on the Tokio runtime (via `spawn_kube`) and report through a channel;
//! [`crate::ConnectionManager`] batches the updates into its entity.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use futures::channel::mpsc::UnboundedSender;
use futures::{StreamExt as _, TryStreamExt as _};
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::core::PartialObjectMeta;
use kube::runtime::WatchStreamExt as _;
use kube::runtime::watcher::{self, Event, watcher};
use kube::{Api, Client};

/// What the namespace watch reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceUpdate {
    /// The full, sorted list.
    Names(Vec<String>),
    /// Listing namespaces is forbidden; use fallbacks.
    Forbidden,
}

fn is_forbidden(err: &watcher::Error) -> bool {
    match err {
        watcher::Error::InitialListFailed(kube::Error::Api(status))
        | watcher::Error::WatchStartFailed(kube::Error::Api(status)) => status.code == 403,
        watcher::Error::WatchError(status) => status.code == 403,
        _ => false,
    }
}

/// Watches namespaces until the receiver goes away or listing is forbidden.
pub async fn watch_namespaces(client: Client, tx: UnboundedSender<NamespaceUpdate>) {
    let api: Api<PartialObjectMeta<Namespace>> = Api::all(client);
    let mut names = BTreeSet::new();
    let mut initializing = BTreeSet::new();
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .boxed();
    loop {
        let event = match stream.try_next().await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(err) if is_forbidden(&err) => {
                tx.unbounded_send(NamespaceUpdate::Forbidden).ok();
                return;
            }
            Err(err) => {
                tracing::debug!("namespace watch error: {err}");
                continue;
            }
        };
        let changed = match event {
            Event::Init => {
                initializing.clear();
                false
            }
            Event::InitApply(ns) => {
                initializing.insert(ns.metadata.name.unwrap_or_default());
                false
            }
            Event::InitDone => {
                names = std::mem::take(&mut initializing);
                true
            }
            Event::Apply(ns) => names.insert(ns.metadata.name.unwrap_or_default()),
            Event::Delete(ns) => names.remove(&ns.metadata.name.unwrap_or_default()),
        };
        if changed
            && tx
                .unbounded_send(NamespaceUpdate::Names(names.iter().cloned().collect()))
                .is_err()
        {
            return;
        }
    }
}

/// Sends the CRD names (`<plural>.<group>`) once the initial list is done, then again whenever
/// a CRD is added or removed, or its spec changes in a way discovery sees (see [`CrdShape`]).
/// Every message after the first means "re-run discovery".
///
/// The watch only reads metadata (full CRDs with their schemas are megabytes on some
/// clusters). A new generation means the spec changed, but often in ways discovery doesn't
/// see: operators rewrite schemas or conversion webhook CA bundles every few minutes. Then
/// that one CRD is fetched and its [`CrdShape`] compared with the last one seen.
pub async fn watch_crds(client: Client, tx: UnboundedSender<Vec<String>>) {
    let full: Api<CustomResourceDefinition> = Api::all(client.clone());
    let api: Api<PartialObjectMeta<CustomResourceDefinition>> = Api::all(client);
    let mut stream = watcher(api, watcher::Config::default())
        .default_backoff()
        .boxed();
    // Name → generation.
    let mut generations = BTreeMap::new();
    let mut initializing = BTreeMap::new();
    // Shapes of the CRDs fetched so far, by name.
    let mut shapes = HashMap::new();
    let mut ready = false;
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(err) if is_forbidden(&err) => return,
            Err(err) => {
                tracing::debug!("CRD watch error: {err}");
                continue;
            }
        };
        let changed = match event {
            Event::Init => {
                initializing.clear();
                false
            }
            Event::InitApply(crd) => {
                let (name, generation) = name_and_generation(&crd.metadata);
                initializing.insert(name, generation);
                false
            }
            Event::InitDone => {
                let previous =
                    std::mem::replace(&mut generations, std::mem::take(&mut initializing));
                if !ready {
                    ready = true;
                    tx.unbounded_send(crd_names(&generations)).ok();
                    false
                } else if previous.keys().ne(generations.keys()) {
                    forget_changed_shapes(&mut shapes, &previous, &generations);
                    true
                } else {
                    // A relist with the same CRDs: check the ones whose spec changed meanwhile.
                    let bumped: Vec<String> = generations
                        .iter()
                        .filter(|(name, generation)| previous.get(*name) != Some(*generation))
                        .map(|(name, _)| name.clone())
                        .collect();
                    let mut changed = false;
                    for name in bumped {
                        changed |= shape_changed(&full, &name, &mut shapes).await;
                    }
                    changed
                }
            }
            Event::Apply(crd) => {
                let (name, generation) = name_and_generation(&crd.metadata);
                match generations.insert(name.clone(), generation) {
                    None => true,
                    Some(previous) if previous == generation => false,
                    Some(_) => shape_changed(&full, &name, &mut shapes).await,
                }
            }
            Event::Delete(crd) => {
                let name = crd.metadata.name.clone().unwrap_or_default();
                shapes.remove(&name);
                generations.remove(&name).is_some()
            }
        };
        if changed && tx.unbounded_send(crd_names(&generations)).is_err() {
            return;
        }
    }
}

/// After a relist that re-runs discovery anyway: keeps the shapes of CRDs that are still there
/// with the same generation. The others may have changed unseen, so their next change must not
/// be compared with a stale shape.
fn forget_changed_shapes<V>(
    shapes: &mut HashMap<String, V>,
    previous: &BTreeMap<String, i64>,
    generations: &BTreeMap<String, i64>,
) {
    shapes.retain(|name, _| {
        generations
            .get(name)
            .is_some_and(|generation| previous.get(name) == Some(generation))
    });
}

fn crd_names(generations: &BTreeMap<String, i64>) -> Vec<String> {
    generations.keys().cloned().collect()
}

/// Status-only updates don't bump the generation, so they don't re-run discovery. (A CRD that
/// becomes Established later is caught by the manager: it re-runs discovery while a CRD isn't
/// served.)
fn name_and_generation(meta: &kube::core::ObjectMeta) -> (String, i64) {
    (
        meta.name.clone().unwrap_or_default(),
        meta.generation.unwrap_or_default(),
    )
}

/// What discovery shows of a CRD: group, names, scope, and each version with whether it's
/// served and its subresources. Schemas, printer columns and conversion settings aren't in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CrdShape(serde_json::Value);

impl CrdShape {
    fn of(crd: &CustomResourceDefinition) -> Self {
        let spec = &crd.spec;
        let versions: Vec<serde_json::Value> = spec
            .versions
            .iter()
            .map(|v| {
                let subresources = v.subresources.as_ref();
                serde_json::json!({
                    "name": v.name,
                    "served": v.served,
                    "status": subresources.is_some_and(|s| s.status.is_some()),
                    "scale": subresources.is_some_and(|s| s.scale.is_some()),
                })
            })
            .collect();
        Self(serde_json::json!({
            "group": spec.group,
            "scope": spec.scope,
            "plural": spec.names.plural,
            "singular": spec.names.singular,
            "kind": spec.names.kind,
            "shortNames": spec.names.short_names,
            "categories": spec.names.categories,
            "versions": versions,
        }))
    }
}

/// Fetches `name` after a new generation and tells whether its [`CrdShape`] changed. The first
/// time a CRD is fetched there's nothing to compare with: that counts as changed. So does a
/// failed fetch.
async fn shape_changed(
    api: &Api<CustomResourceDefinition>,
    name: &str,
    shapes: &mut HashMap<String, CrdShape>,
) -> bool {
    match api.get(name).await {
        Ok(crd) => {
            let shape = CrdShape::of(&crd);
            let changed = shapes.get(name) != Some(&shape);
            if !changed {
                tracing::debug!(crd = name, "CRD spec changed, but not what discovery sees");
            }
            shapes.insert(name.to_string(), shape);
            changed
        }
        Err(err) => {
            tracing::debug!(crd = name, "couldn't read a changed CRD: {err}");
            shapes.remove(name);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crd(json: serde_json::Value) -> CustomResourceDefinition {
        serde_json::from_value(json).unwrap()
    }

    fn widget(schema_description: &str, served: bool) -> CustomResourceDefinition {
        crd(serde_json::json!({
            "apiVersion": "apiextensions.k8s.io/v1",
            "kind": "CustomResourceDefinition",
            "metadata": {"name": "widgets.example.com", "generation": 3},
            "spec": {
                "group": "example.com",
                "scope": "Namespaced",
                "names": {"plural": "widgets", "singular": "widget", "kind": "Widget"},
                "versions": [{
                    "name": "v1",
                    "served": served,
                    "storage": true,
                    "schema": {"openAPIV3Schema": {"type": "object", "description": schema_description}},
                    "subresources": {"status": {}},
                }],
                "conversion": {"strategy": "None"},
            },
        }))
    }

    #[test]
    fn a_relist_forgets_shapes_of_changed_and_removed_crds() {
        let generations = |pairs: &[(&str, i64)]| -> BTreeMap<String, i64> {
            pairs.iter().map(|(n, g)| (n.to_string(), *g)).collect()
        };
        let mut shapes: HashMap<String, ()> = ["same", "bumped", "removed"]
            .into_iter()
            .map(|n| (n.to_string(), ()))
            .collect();
        forget_changed_shapes(
            &mut shapes,
            &generations(&[("same", 1), ("bumped", 1), ("removed", 1)]),
            &generations(&[("same", 1), ("bumped", 2), ("added", 1)]),
        );
        assert_eq!(shapes.keys().collect::<Vec<_>>(), ["same"]);
    }

    #[test]
    fn schema_changes_keep_the_shape() {
        assert_eq!(
            CrdShape::of(&widget("old", true)),
            CrdShape::of(&widget("new", true))
        );
    }

    #[test]
    fn served_versions_change_the_shape() {
        assert_ne!(
            CrdShape::of(&widget("same", true)),
            CrdShape::of(&widget("same", false))
        );
    }
}
