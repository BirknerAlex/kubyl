//! What a Kustomization or HelmRelease applied: `status.inventory.entries`, each an id
//! `<namespace>_<name>_<group>_<kind>` (cli-utils' `ObjMetadata`: cluster-scoped objects have an
//! empty namespace, the core group is empty, and a `:` in a name, as in RBAC names, is written
//! `__`) with the version it was applied at.
//!
//! Large inventories (thousands of objects) are only parsed here; the views resolve the entries
//! against Kubyl's caches lazily, for the rows they show.

use std::sync::Arc;

use kubyl_base::Tone;
use kubyl_resources_core::status::pod_status;
use serde_json::Value;

use crate::kinds::FluxKind;
use crate::model::{FluxObject, State};

/// One applied object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Entry {
    pub namespace: String,
    pub name: String,
    pub group: String,
    pub kind: String,
    pub version: String,
}

impl Entry {
    /// Parses an inventory id; `version` is the entry's `v`.
    pub fn parse(id: &str, version: &str) -> Option<Entry> {
        let (namespace, rest) = id.split_once('_')?;
        let (rest, kind) = rest.rsplit_once('_')?;
        let (name, group) = rest.rsplit_once('_')?;
        if name.is_empty() || kind.is_empty() {
            return None;
        }
        Some(Entry {
            namespace: namespace.to_string(),
            name: name.replace("__", ":"),
            group: group.to_string(),
            kind: kind.to_string(),
            version: version.to_string(),
        })
    }

    /// `apps/v1`, `v1`.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    pub fn namespace(&self) -> Option<&str> {
        (!self.namespace.is_empty()).then_some(self.namespace.as_str())
    }

    /// `Deployment flux-podinfo/podinfo`.
    pub fn label(&self) -> String {
        match self.namespace() {
            Some(ns) => format!("{} {ns}/{}", self.kind, self.name),
            None => format!("{} {}", self.kind, self.name),
        }
    }
}

/// The inventory of a Kustomization or HelmRelease (`None` when it has none: never applied, or
/// a HelmRelease from before helm-controller kept one).
pub fn entries(object: &Value) -> Option<Vec<Entry>> {
    let entries = object.pointer("/status/inventory/entries")?.as_array()?;
    let mut list: Vec<Entry> = entries
        .iter()
        .filter_map(|e| {
            Entry::parse(
                e.get("id")?.as_str()?,
                e.get("v").and_then(Value::as_str).unwrap_or_default(),
            )
        })
        .collect();
    list.sort_by(|a, b| {
        order(&a.kind)
            .cmp(&order(&b.kind))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.namespace.cmp(&b.namespace))
            .then_with(|| a.name.cmp(&b.name))
    });
    Some(list)
}

/// Workloads first, then what they need, like Kubyl's resource trees.
fn order(kind: &str) -> u8 {
    match kind {
        "Namespace" => 0,
        "CustomResourceDefinition" => 1,
        "Deployment" | "StatefulSet" | "DaemonSet" | "CronJob" | "Job" => 2,
        "Kustomization" | "HelmRelease" => 3,
        "Service" | "Ingress" | "HTTPRoute" => 4,
        "ConfigMap" | "Secret" => 5,
        _ => 6,
    }
}

/// Whether kustomize-controller leaves the object in place when its Kustomization is deleted
/// (or prunes): `kustomize.toolkit.fluxcd.io/prune: disabled`, `…/reconcile: disabled` or
/// `…/ssa: Ignore` as a label or an annotation (what its finalizer excludes). `object` needs
/// only its metadata.
pub fn kept_on_delete(object: &Value) -> bool {
    const EXCLUDED: [(&str, &str); 3] = [
        ("kustomize.toolkit.fluxcd.io/prune", "disabled"),
        ("kustomize.toolkit.fluxcd.io/reconcile", "disabled"),
        ("kustomize.toolkit.fluxcd.io/ssa", "Ignore"),
    ];
    ["/metadata/labels", "/metadata/annotations"]
        .iter()
        .filter_map(|p| object.pointer(p))
        .any(|map| {
            EXCLUDED
                .iter()
                .any(|(key, value)| map.get(*key).and_then(Value::as_str) == Some(*value))
        })
}

/// Counts per kind (`3 Deployments, 2 Services`), for the delete summary.
pub fn summary(entries: &[Entry]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for entry in entries {
        match counts.iter_mut().find(|(k, _)| *k == entry.kind) {
            Some((_, n)) => *n += 1,
            None => counts.push((entry.kind.clone(), 1)),
        }
    }
    counts
        .into_iter()
        .map(|(kind, n)| {
            if n == 1 {
                format!("1 {kind}")
            } else {
                format!("{n} {}", plural(&kind))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(kind: &str) -> String {
    if kind.ends_with('s') {
        format!("{kind}es")
    } else if let Some(stem) = kind.strip_suffix('y') {
        format!("{stem}ies")
    } else {
        format!("{kind}s")
    }
}

/// How an applied object is doing, from Kubyl's live caches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeHealth {
    pub tone: Tone,
    /// `Healthy`, `Progressing`, `Degraded`, `Missing`, a Flux state, a pod's reason.
    pub label: String,
    /// `2/2 ready`, `Running · 10.244.0.12`.
    pub info: String,
}

impl NodeHealth {
    pub fn missing() -> Self {
        NodeHealth {
            tone: Tone::Warning,
            label: "Missing".into(),
            info: "not in the cluster".into(),
        }
    }

    pub fn is_healthy(&self) -> bool {
        !matches!(self.tone, Tone::Bad | Tone::Warning)
    }
}

fn int(object: &Value, pointer: &str) -> i64 {
    object.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

/// The health of a live object of `group`/`kind` (workloads, pods, Flux objects, Services,
/// claims); others are present and nothing more is known.
pub fn health(group: &str, kind: &str, object: &Arc<Value>) -> NodeHealth {
    let ready = |ready: i64, wanted: i64| NodeHealth {
        tone: if ready >= wanted {
            Tone::Good
        } else if ready == 0 && wanted > 0 {
            Tone::Bad
        } else {
            Tone::Info
        },
        label: if ready >= wanted {
            "Healthy".into()
        } else if ready == 0 && wanted > 0 {
            "Degraded".into()
        } else {
            "Progressing".into()
        },
        info: format!("{ready}/{wanted} ready"),
    };
    match (group, kind) {
        ("apps", "Deployment" | "StatefulSet" | "ReplicaSet") => ready(
            int(object, "/status/readyReplicas"),
            object
                .pointer("/spec/replicas")
                .and_then(Value::as_i64)
                .unwrap_or(1),
        ),
        ("apps", "DaemonSet") => ready(
            int(object, "/status/numberReady"),
            int(object, "/status/desiredNumberScheduled"),
        ),
        ("", "Pod") => {
            let status = pod_status(object);
            let ip = object
                .pointer("/status/podIP")
                .and_then(Value::as_str)
                .map(|ip| format!(" · {ip}"))
                .unwrap_or_default();
            NodeHealth {
                tone: status.tone(),
                label: status.reason.clone(),
                info: format!("{} ready{ip}", status.ready_label()),
            }
        }
        ("", "PersistentVolumeClaim") => {
            let phase = object
                .pointer("/status/phase")
                .and_then(Value::as_str)
                .unwrap_or("Pending");
            NodeHealth {
                tone: if phase == "Bound" {
                    Tone::Good
                } else {
                    Tone::Info
                },
                label: phase.into(),
                info: String::new(),
            }
        }
        ("", "Service") => NodeHealth {
            tone: Tone::Good,
            label: "Healthy".into(),
            info: object
                .pointer("/spec/type")
                .and_then(Value::as_str)
                .map(|t| {
                    let ip = object
                        .pointer("/spec/clusterIP")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    format!("{t} {ip}").trim().to_string()
                })
                .unwrap_or_default(),
        },
        _ => {
            if let Some(kind) = FluxKind::from_group_kind(group, kind)
                && let Some(flux) = FluxObject::parse_as(kind, object)
            {
                let state = flux.state();
                return NodeHealth {
                    tone: match state {
                        State::Ready => Tone::Good,
                        State::Failed | State::Stalled => Tone::Bad,
                        State::Reconciling => Tone::Info,
                        State::Suspended | State::Unknown => Tone::Muted,
                    },
                    label: state.label().into(),
                    info: flux
                        .revision()
                        .map(|r| crate::model::short_revision(&r))
                        .unwrap_or_default(),
                };
            }
            NodeHealth {
                tone: Tone::Neutral,
                label: String::new(),
                info: String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;

    #[test]
    fn inventory_ids() {
        let deployment = Entry::parse("flux-podinfo_podinfo_apps_Deployment", "v1").unwrap();
        assert_eq!(deployment.namespace(), Some("flux-podinfo"));
        assert_eq!(deployment.api_version(), "apps/v1");
        assert_eq!(deployment.label(), "Deployment flux-podinfo/podinfo");
        let service = Entry::parse("flux-podinfo_podinfo__Service", "v1").unwrap();
        assert_eq!(service.group, "");
        assert_eq!(service.api_version(), "v1");
        let namespace = Entry::parse("_flux-chain__Namespace", "v1").unwrap();
        assert_eq!(namespace.namespace(), None);
        assert_eq!(namespace.name, "flux-chain");
        // RBAC names with colons.
        let role = Entry::parse(
            "_system__controller__podinfo_rbac.authorization.k8s.io_ClusterRole",
            "v1",
        )
        .unwrap();
        assert_eq!(role.name, "system:controller:podinfo");
        assert_eq!(role.group, "rbac.authorization.k8s.io");
        // RBAC and other path-segment names may contain `_` (Kubernetes names never end in one).
        let binding = Entry::parse(
            "team-a_read_only_rbac.authorization.k8s.io_RoleBinding",
            "v1",
        )
        .unwrap();
        assert_eq!(binding.name, "read_only");
        assert_eq!(binding.group, "rbac.authorization.k8s.io");
        assert_eq!(binding.kind, "RoleBinding");
        let config = Entry::parse("apps_my_config__ConfigMap", "v1").unwrap();
        assert_eq!(
            (config.name.as_str(), config.group.as_str()),
            ("my_config", "")
        );
        let role = Entry::parse("_sys__a_b_rbac.authorization.k8s.io_ClusterRole", "v1").unwrap();
        assert_eq!(role.name, "sys:a_b");
        assert_eq!(Entry::parse("garbage", "v1"), None);
        assert_eq!(Entry::parse("ns__apps_", "v1"), None);
    }

    #[test]
    fn health_of_applied_objects() {
        let deployment =
            Arc::new(serde_json::json!({"spec": {"replicas": 2}, "status": {"readyReplicas": 1}}));
        let h = health("apps", "Deployment", &deployment);
        assert_eq!((h.tone, h.info.as_str()), (Tone::Info, "1/2 ready"));
        let down = Arc::new(serde_json::json!({"spec": {"replicas": 2}, "status": {}}));
        assert!(!health("apps", "Deployment", &down).is_healthy());
        let ks = Arc::new(fixtures::kustomization_failed());
        let h = health("kustomize.toolkit.fluxcd.io", "Kustomization", &ks);
        assert_eq!((h.tone, h.label.as_str()), (Tone::Bad, "Failed"));
        let cm = Arc::new(serde_json::json!({"data": {}}));
        assert!(health("", "ConfigMap", &cm).is_healthy());
        assert!(!NodeHealth::missing().is_healthy());
    }

    #[test]
    fn objects_marked_to_stay() {
        let marked = |meta: Value| kept_on_delete(&serde_json::json!({"metadata": meta}));
        assert!(marked(
            serde_json::json!({"annotations": {"kustomize.toolkit.fluxcd.io/prune": "disabled"}})
        ));
        assert!(marked(
            serde_json::json!({"labels": {"kustomize.toolkit.fluxcd.io/reconcile": "disabled"}})
        ));
        assert!(marked(
            serde_json::json!({"annotations": {"kustomize.toolkit.fluxcd.io/ssa": "Ignore"}})
        ));
        assert!(!marked(
            serde_json::json!({"annotations": {"kustomize.toolkit.fluxcd.io/prune": "enabled"}})
        ));
        assert!(!marked(serde_json::json!({})));
    }

    #[test]
    fn inventories_sorted_and_summarized() {
        let list = entries(&fixtures::kustomization_ready()).unwrap();
        let kinds: Vec<&str> = list.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["Deployment", "Service", "HorizontalPodAutoscaler"]);
        assert_eq!(
            summary(&list),
            "1 Deployment, 1 Service, 1 HorizontalPodAutoscaler"
        );
        let many = vec![
            Entry::parse("a_x__Service", "v1").unwrap(),
            Entry::parse("a_y__Service", "v1").unwrap(),
            Entry::parse("a_p_networking.k8s.io_NetworkPolicy", "v1").unwrap(),
            Entry::parse("a_q_networking.k8s.io_NetworkPolicy", "v1").unwrap(),
            Entry::parse("a_c__ConfigMap", "v1").unwrap(),
        ];
        assert_eq!(summary(&many), "2 Services, 2 NetworkPolicies, 1 ConfigMap");
        assert!(entries(&fixtures::kustomization_failed()).is_none());
        assert_eq!(entries(&fixtures::helm_release_v2()).unwrap().len(), 2);
    }
}
