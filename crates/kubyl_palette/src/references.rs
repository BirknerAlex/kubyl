//! What an object refers to: its owners, the node a pod runs on, the secrets and config maps
//! it mounts, the pods a selector picks, a binding's role… The palette's References mode
//! (`shift-o` in lists) jumps to them.

use std::collections::BTreeSet;

use kubyl_core::Gvk;
use serde_json::Value;

/// Where a reference points.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefTarget {
    /// One object. `namespace: None` for cluster-scoped kinds (or when unknown: the view
    /// decides from discovery).
    Object {
        gvk: Gvk,
        namespace: Option<String>,
        name: String,
    },
    /// A list filtered with the explorer's filter language (`app=web`, `spec.nodeName=n1`).
    Filtered {
        gvk: Gvk,
        /// `None`: all namespaces.
        namespace: Option<String>,
        filter: String,
    },
}

/// One reference, with a label for the palette (`Owner`, `Volume`, `Selects`…).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub relation: &'static str,
    pub target: RefTarget,
}

impl Reference {
    /// `Deployment web`, `Pods app=web`.
    pub fn title(&self) -> String {
        match &self.target {
            RefTarget::Object { gvk, name, .. } => format!("{} {name}", gvk.kind),
            RefTarget::Filtered { gvk, filter, .. } => format!("{}s {filter}", gvk.kind),
        }
    }
}

fn core(kind: &str) -> Gvk {
    Gvk::new("", "v1", kind)
}

fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer)?.as_str().filter(|s| !s.is_empty())
}

fn array_at<'a>(value: &'a Value, pointer: &str) -> &'a [Value] {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `apps/v1` + `Deployment` → the GVK.
fn gvk_of(api_version: &str, kind: &str) -> Gvk {
    match api_version.split_once('/') {
        Some((group, version)) => Gvk::new(group, version, kind),
        None => Gvk::new("", api_version, kind),
    }
}

/// `matchLabels` (and `key=value` selectors of Services) as a filter: `app=web,tier=db`.
fn selector_filter(labels: &Value) -> Option<String> {
    let map = labels.as_object()?;
    if map.is_empty() {
        return None;
    }
    Some(
        map.iter()
            .filter_map(|(k, v)| Some(format!("{k}={}", v.as_str()?)))
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Every reference of `object`. `kind` is the object's kind (`Pod`).
pub fn references(kind: &str, object: &Value) -> Vec<Reference> {
    let namespace = str_at(object, "/metadata/namespace").map(String::from);
    let mut out = BTreeSet::new();
    let mut add = |relation: &'static str, target: RefTarget| {
        out.insert(Reference { relation, target });
    };
    let object_ref = |gvk: Gvk, ns: Option<String>, name: &str| RefTarget::Object {
        gvk,
        namespace: ns,
        name: name.to_string(),
    };

    for owner in array_at(object, "/metadata/ownerReferences") {
        if let (Some(api), Some(kind), Some(name)) = (
            str_at(owner, "/apiVersion"),
            str_at(owner, "/kind"),
            str_at(owner, "/name"),
        ) {
            add(
                "Owner",
                object_ref(gvk_of(api, kind), namespace.clone(), name),
            );
        }
    }

    // Pod specs: of the pod itself, or the template of a workload.
    let pod_spec = match kind {
        "Pod" => object.pointer("/spec"),
        "CronJob" => object.pointer("/spec/jobTemplate/spec/template/spec"),
        _ => object.pointer("/spec/template/spec"),
    };
    if let Some(spec) = pod_spec {
        pod_references(spec, &namespace, &mut add);
    }

    let selects = |selector: Option<&Value>| selector.and_then(selector_filter);
    let selector = match kind {
        "Service" => selects(object.pointer("/spec/selector")),
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" => {
            selects(object.pointer("/spec/selector/matchLabels"))
        }
        "PodDisruptionBudget" => selects(object.pointer("/spec/selector/matchLabels")),
        _ => None,
    };
    if let Some(filter) = selector {
        add(
            "Selects",
            RefTarget::Filtered {
                gvk: core("Pod"),
                namespace: namespace.clone(),
                filter,
            },
        );
    }

    match kind {
        "Node" => {
            if let Some(name) = str_at(object, "/metadata/name") {
                add(
                    "Runs",
                    RefTarget::Filtered {
                        gvk: core("Pod"),
                        namespace: None,
                        filter: format!("spec.nodeName={name}"),
                    },
                );
            }
        }
        "PersistentVolumeClaim" => {
            if let Some(pv) = str_at(object, "/spec/volumeName") {
                add("Volume", object_ref(core("PersistentVolume"), None, pv));
            }
            if let Some(class) = str_at(object, "/spec/storageClassName") {
                add(
                    "Storage class",
                    object_ref(
                        Gvk::new("storage.k8s.io", "v1", "StorageClass"),
                        None,
                        class,
                    ),
                );
            }
        }
        "PersistentVolume" => {
            if let (Some(ns), Some(name)) = (
                str_at(object, "/spec/claimRef/namespace"),
                str_at(object, "/spec/claimRef/name"),
            ) {
                add(
                    "Claim",
                    object_ref(core("PersistentVolumeClaim"), Some(ns.into()), name),
                );
            }
            if let Some(class) = str_at(object, "/spec/storageClassName") {
                add(
                    "Storage class",
                    object_ref(
                        Gvk::new("storage.k8s.io", "v1", "StorageClass"),
                        None,
                        class,
                    ),
                );
            }
        }
        "Ingress" => {
            for rule in array_at(object, "/spec/rules") {
                for path in array_at(rule, "/http/paths") {
                    if let Some(service) = str_at(path, "/backend/service/name") {
                        add(
                            "Backend",
                            object_ref(core("Service"), namespace.clone(), service),
                        );
                    }
                }
            }
            if let Some(service) = str_at(object, "/spec/defaultBackend/service/name") {
                add(
                    "Backend",
                    object_ref(core("Service"), namespace.clone(), service),
                );
            }
            for tls in array_at(object, "/spec/tls") {
                if let Some(secret) = str_at(tls, "/secretName") {
                    add("TLS", object_ref(core("Secret"), namespace.clone(), secret));
                }
            }
            if let Some(class) = str_at(object, "/spec/ingressClassName") {
                add(
                    "Class",
                    object_ref(
                        Gvk::new("networking.k8s.io", "v1", "IngressClass"),
                        None,
                        class,
                    ),
                );
            }
        }
        "RoleBinding" | "ClusterRoleBinding" => {
            if let (Some(kind), Some(name)) = (
                str_at(object, "/roleRef/kind"),
                str_at(object, "/roleRef/name"),
            ) {
                let ns = (kind == "Role").then(|| namespace.clone()).flatten();
                add(
                    "Role",
                    object_ref(Gvk::new("rbac.authorization.k8s.io", "v1", kind), ns, name),
                );
            }
            for subject in array_at(object, "/subjects") {
                if str_at(subject, "/kind") == Some("ServiceAccount")
                    && let Some(name) = str_at(subject, "/name")
                {
                    let ns = str_at(subject, "/namespace")
                        .map(String::from)
                        .or_else(|| namespace.clone());
                    add("Subject", object_ref(core("ServiceAccount"), ns, name));
                }
            }
        }
        "HorizontalPodAutoscaler" => {
            if let (Some(api), Some(kind), Some(name)) = (
                str_at(object, "/spec/scaleTargetRef/apiVersion"),
                str_at(object, "/spec/scaleTargetRef/kind"),
                str_at(object, "/spec/scaleTargetRef/name"),
            ) {
                add(
                    "Scales",
                    object_ref(gvk_of(api, kind), namespace.clone(), name),
                );
            }
        }
        "Event" => {
            let regarding = object
                .pointer("/regarding")
                .or_else(|| object.pointer("/involvedObject"));
            if let Some(regarding) = regarding
                && let (Some(api), Some(kind), Some(name)) = (
                    str_at(regarding, "/apiVersion"),
                    str_at(regarding, "/kind"),
                    str_at(regarding, "/name"),
                )
            {
                let ns = str_at(regarding, "/namespace").map(String::from);
                add("About", object_ref(gvk_of(api, kind), ns, name));
            }
        }
        _ => {}
    }
    out.into_iter().collect()
}

fn pod_references(
    spec: &Value,
    namespace: &Option<String>,
    add: &mut impl FnMut(&'static str, RefTarget),
) {
    let object = |kind: &str, name: &str| RefTarget::Object {
        gvk: core(kind),
        namespace: namespace.clone(),
        name: name.to_string(),
    };
    if let Some(node) = str_at(spec, "/nodeName") {
        add(
            "Node",
            RefTarget::Object {
                gvk: core("Node"),
                namespace: None,
                name: node.to_string(),
            },
        );
    }
    if let Some(sa) = str_at(spec, "/serviceAccountName") {
        add("Service account", object("ServiceAccount", sa));
    }
    for secret in array_at(spec, "/imagePullSecrets") {
        if let Some(name) = str_at(secret, "/name") {
            add("Pull secret", object("Secret", name));
        }
    }
    for volume in array_at(spec, "/volumes") {
        if let Some(name) = str_at(volume, "/secret/secretName") {
            add("Volume", object("Secret", name));
        }
        if let Some(name) = str_at(volume, "/configMap/name") {
            add("Volume", object("ConfigMap", name));
        }
        if let Some(name) = str_at(volume, "/persistentVolumeClaim/claimName") {
            add("Volume", object("PersistentVolumeClaim", name));
        }
        for source in array_at(volume, "/projected/sources") {
            if let Some(name) = str_at(source, "/secret/name") {
                add("Volume", object("Secret", name));
            }
            if let Some(name) = str_at(source, "/configMap/name") {
                add("Volume", object("ConfigMap", name));
            }
        }
    }
    let containers = array_at(spec, "/containers")
        .iter()
        .chain(array_at(spec, "/initContainers"));
    for container in containers {
        for from in array_at(container, "/envFrom") {
            if let Some(name) = str_at(from, "/secretRef/name") {
                add("Env", object("Secret", name));
            }
            if let Some(name) = str_at(from, "/configMapRef/name") {
                add("Env", object("ConfigMap", name));
            }
        }
        for env in array_at(container, "/env") {
            if let Some(name) = str_at(env, "/valueFrom/secretKeyRef/name") {
                add("Env", object("Secret", name));
            }
            if let Some(name) = str_at(env, "/valueFrom/configMapKeyRef/name") {
                add("Env", object("ConfigMap", name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn titles(refs: &[Reference]) -> Vec<(String, String)> {
        refs.iter()
            .map(|r| (r.relation.to_string(), r.title()))
            .collect()
    }

    #[test]
    fn pod_references() {
        let pod = json!({
            "metadata": {
                "namespace": "web",
                "ownerReferences": [{"apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "web-7d"}]
            },
            "spec": {
                "nodeName": "node-1",
                "serviceAccountName": "web",
                "volumes": [
                    {"name": "tls", "secret": {"secretName": "web-tls"}},
                    {"name": "cfg", "configMap": {"name": "web-config"}},
                    {"name": "data", "persistentVolumeClaim": {"claimName": "web-data"}}
                ],
                "containers": [{
                    "envFrom": [{"secretRef": {"name": "web-env"}}],
                    "env": [{"name": "X", "valueFrom": {"configMapKeyRef": {"name": "flags", "key": "x"}}}]
                }]
            }
        });
        let refs = references("Pod", &pod);
        let titles = titles(&refs);
        for expected in [
            ("Owner", "ReplicaSet web-7d"),
            ("Node", "Node node-1"),
            ("Service account", "ServiceAccount web"),
            ("Volume", "Secret web-tls"),
            ("Volume", "ConfigMap web-config"),
            ("Volume", "PersistentVolumeClaim web-data"),
            ("Env", "Secret web-env"),
            ("Env", "ConfigMap flags"),
        ] {
            assert!(
                titles.contains(&(expected.0.to_string(), expected.1.to_string())),
                "{expected:?} missing from {titles:?}"
            );
        }
        let owner = refs.iter().find(|r| r.relation == "Owner").unwrap();
        assert_eq!(
            owner.target,
            RefTarget::Object {
                gvk: Gvk::new("apps", "v1", "ReplicaSet"),
                namespace: Some("web".into()),
                name: "web-7d".into()
            }
        );
        let node = refs.iter().find(|r| r.relation == "Node").unwrap();
        assert!(matches!(
            &node.target,
            RefTarget::Object {
                namespace: None,
                ..
            }
        ));
    }

    #[test]
    fn selectors_become_filters() {
        let deployment = json!({
            "metadata": {"namespace": "web"},
            "spec": {
                "selector": {"matchLabels": {"app": "web", "tier": "front"}},
                "template": {"spec": {"containers": [], "volumes": [{"secret": {"secretName": "s"}}]}}
            }
        });
        let refs = references("Deployment", &deployment);
        assert!(refs.contains(&Reference {
            relation: "Selects",
            target: RefTarget::Filtered {
                gvk: Gvk::new("", "v1", "Pod"),
                namespace: Some("web".into()),
                filter: "app=web,tier=front".into()
            }
        }));
        assert!(titles(&refs).contains(&("Volume".into(), "Secret s".into())));

        let node = json!({"metadata": {"name": "n1"}});
        let refs = references("Node", &node);
        assert_eq!(
            refs[0].target,
            RefTarget::Filtered {
                gvk: Gvk::new("", "v1", "Pod"),
                namespace: None,
                filter: "spec.nodeName=n1".into()
            }
        );
    }

    #[test]
    fn bindings_ingresses_and_events() {
        let binding = json!({
            "metadata": {"namespace": "ns"},
            "roleRef": {"kind": "ClusterRole", "name": "view"},
            "subjects": [{"kind": "ServiceAccount", "name": "ci"}]
        });
        let titles_ = titles(&references("RoleBinding", &binding));
        assert!(titles_.contains(&("Role".into(), "ClusterRole view".into())));
        assert!(titles_.contains(&("Subject".into(), "ServiceAccount ci".into())));

        let ingress = json!({
            "metadata": {"namespace": "ns"},
            "spec": {
                "rules": [{"http": {"paths": [{"backend": {"service": {"name": "web"}}}]}}],
                "tls": [{"secretName": "web-tls"}]
            }
        });
        let titles_ = titles(&references("Ingress", &ingress));
        assert!(titles_.contains(&("Backend".into(), "Service web".into())));
        assert!(titles_.contains(&("TLS".into(), "Secret web-tls".into())));

        let event = json!({
            "involvedObject": {"apiVersion": "v1", "kind": "Pod", "name": "p", "namespace": "ns"}
        });
        assert_eq!(
            titles(&references("Event", &event)),
            vec![("About".to_string(), "Pod p".to_string())]
        );
    }
}
