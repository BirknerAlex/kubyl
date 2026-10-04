//! Desired vs live per resource (API mode): both sides rendered the way the YAML editor
//! renders objects (keys sorted, `managedFields`, `status` and server-set metadata dropped,
//! Secret values and OpenShift Routes' inline TLS keys masked on both sides), then diffed with
//! phase 04's line diff.

use kubyl_yaml_core::diff::{self, LineDiff};
use kubyl_yaml_core::render::{self, RenderOptions};
use serde_json::Value;

use crate::api::ResourceDiff;

/// Lines of context around changes.
const CONTEXT: usize = 3;

/// `object` as comparable text: server fields stripped, Secrets masked. Empty for `None`.
pub fn comparable(object: Option<&Value>) -> String {
    let Some(object) = object else {
        return String::new();
    };
    let mut object = object.clone();
    render::strip_server_fields(&mut object);
    if let Some(annotations) = object
        .pointer_mut("/metadata/annotations")
        .and_then(Value::as_object_mut)
    {
        // Argo CD's own bookkeeping, not the user's change.
        annotations.remove("argocd.argoproj.io/tracking-id");
        annotations.remove("kubectl.kubernetes.io/last-applied-configuration");
        if annotations.is_empty()
            && let Some(meta) = object.get_mut("metadata").and_then(Value::as_object_mut)
        {
            meta.remove("annotations");
        }
    }
    render::render(&object, RenderOptions::default()).0
}

/// Live → desired for one resource. A resource that doesn't exist yet is all additions; one
/// that should be pruned is all removals.
pub fn resource_diff(item: &ResourceDiff) -> LineDiff {
    let live = comparable(item.live().as_ref());
    let desired = comparable(item.desired().as_ref());
    diff::diff(&live, &desired, CONTEXT)
}

/// Whether the item is a core Secret (its values are masked on both sides).
pub fn is_secret(item: &ResourceDiff) -> bool {
    item.group.is_empty() && item.kind == "Secret"
}

/// What the diff masks, for a note next to it: `values masked` (Secrets), `TLS key masked`
/// (Routes with an inline key on either side).
pub fn masked_note(item: &ResourceDiff) -> Option<&'static str> {
    if is_secret(item) {
        return Some("values masked");
    }
    let route_key = kubyl_resources_core::route::is_route(&item.group, &item.kind)
        && [item.live(), item.desired()]
            .iter()
            .flatten()
            .any(kubyl_resources_core::route::has_key_material);
    route_key.then_some("TLS key masked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(live: Value, desired: Value) -> ResourceDiff {
        ResourceDiff {
            kind: "Deployment".into(),
            group: "apps".into(),
            name: "web".into(),
            namespace: "web".into(),
            normalized_live_state: live.to_string(),
            predicted_live_state: desired.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn shows_the_changed_field_only() {
        let live = json!({"apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "web", "resourceVersion": "9", "uid": "u",
                         "annotations": {"argocd.argoproj.io/tracking-id": "web:apps/Deployment:web/web"}},
            "spec": {"replicas": 3, "template": {"spec": {"containers": [{"name": "web", "image": "nginx"}]}}},
            "status": {"readyReplicas": 3}});
        let desired = json!({"apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "web"},
            "spec": {"replicas": 1, "template": {"spec": {"containers": [{"name": "web", "image": "nginx"}]}}}});
        let diff = resource_diff(&item(live, desired));
        assert_eq!(diff.summary, ["~ spec.replicas"]);
        assert_eq!((diff.added, diff.removed), (1, 1));
    }

    #[test]
    fn missing_and_extra_resources() {
        let desired = json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "c"}, "data": {"a": "1"}});
        let missing = ResourceDiff {
            predicted_live_state: String::new(),
            target_state: desired.to_string(),
            live_state: "null".into(),
            ..Default::default()
        };
        let diff = resource_diff(&missing);
        assert!(diff.removed == 0 && diff.added > 0);
        let extra = ResourceDiff {
            normalized_live_state: desired.to_string(),
            target_state: "null".into(),
            ..Default::default()
        };
        let diff = resource_diff(&extra);
        assert!(diff.added == 0 && diff.removed > 0);
    }

    #[test]
    fn secrets_stay_masked() {
        let secret = |value: &str| json!({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "db"}, "data": {"password": value}});
        // "aHVudGVyMg==" is "hunter2", "c2VjcmV0" is "secret".
        let text = comparable(Some(&secret("aHVudGVyMg==")));
        assert!(!text.contains("hunter2") && !text.contains("aHVudGVyMg=="));
        let diff = resource_diff(&ResourceDiff {
            kind: "Secret".into(),
            normalized_live_state: secret("aHVudGVyMg==").to_string(),
            predicted_live_state: secret("c2VjcmV0").to_string(),
            ..Default::default()
        });
        for hunk in &diff.hunks {
            for line in &hunk.lines {
                assert!(!line.text.contains("hunter2") && !line.text.contains("secret"));
            }
        }
    }

    #[test]
    fn route_keys_stay_masked() {
        let route = |key: &str, host: &str| {
            json!({"apiVersion": "route.openshift.io/v1", "kind": "Route",
            "metadata": {"name": "shop", "annotations": {"kubectl.kubernetes.io/last-applied-configuration": key}},
            "spec": {"host": host, "to": {"kind": "Service", "name": "shop"},
                     "tls": {"termination": "reencrypt", "key": key}}})
        };
        let item = ResourceDiff {
            group: "route.openshift.io".into(),
            kind: "Route".into(),
            normalized_live_state: route("OLD-PRIVATE-KEY", "a.example.com").to_string(),
            predicted_live_state: route("NEW-PRIVATE-KEY", "b.example.com").to_string(),
            ..Default::default()
        };
        let diff = resource_diff(&item);
        assert_eq!(diff.summary, ["~ spec.host"]);
        for hunk in &diff.hunks {
            for line in &hunk.lines {
                assert!(!line.text.contains("PRIVATE-KEY"), "{}", line.text);
            }
        }
        assert_eq!(masked_note(&item), Some("TLS key masked"));
        assert_eq!(
            masked_note(&ResourceDiff {
                kind: "Secret".into(),
                ..Default::default()
            }),
            Some("values masked")
        );
        assert_eq!(masked_note(&ResourceDiff::default()), None);

        // The key was taken out of `spec`, but the live last-applied copy still holds it.
        let stale = serde_json::json!({"apiVersion": "route.openshift.io/v1", "kind": "Route",
            "metadata": {"name": "shop", "annotations": {"kubectl.kubernetes.io/last-applied-configuration":
                "{\"spec\":{\"tls\":{\"key\":\"STALE-PRIVATE-KEY\"}}}"}},
            "spec": {"host": "a.example.com", "to": {"kind": "Service", "name": "shop"},
                     "tls": {"termination": "edge"}}});
        let item = ResourceDiff {
            group: "route.openshift.io".into(),
            kind: "Route".into(),
            normalized_live_state: stale.to_string(),
            predicted_live_state: route("NEW-PRIVATE-KEY", "b.example.com").to_string(),
            ..Default::default()
        };
        for hunk in &resource_diff(&item).hunks {
            for line in &hunk.lines {
                assert!(!line.text.contains("PRIVATE-KEY"), "{}", line.text);
            }
        }
        assert_eq!(masked_note(&item), Some("TLS key masked"));
    }
}
