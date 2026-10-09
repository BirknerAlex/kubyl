//! Which Argo CD app tracks an object (used by the details of any object and by the
//! Applications view).
//!
//! Tracking: the `argocd.argoproj.io/tracking-id` annotation (Argo CD 3.x default) names the app
//! and the object; it counts only when it matches the object (a copied manifest keeps the
//! annotation but isn't managed). The `app.kubernetes.io/instance` label (label tracking) is also
//! what Helm sets, so it counts only when an Application of that name exists.

use serde_json::Value;

use crate::model::{TRACKING_ANNOTATION, TRACKING_LABEL};

/// Which app manages an object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedBy {
    /// Set for apps outside Argo CD's own namespace (`<namespace>_<name>`).
    pub app_namespace: Option<String>,
    pub app_name: String,
    /// Found through the instance label (needs an Application of that name to count).
    pub via_label: bool,
}

fn split_app(value: &str) -> (Option<String>, String) {
    match value.split_once('_') {
        Some((ns, name)) => (Some(ns.to_string()), name.to_string()),
        None => (None, value.to_string()),
    }
}

/// The app that tracks `object`, if any.
pub fn managed_by(object: &Value) -> Option<ManagedBy> {
    managed_by_kind(object, None)
}

/// [`managed_by`] for an object that may not say its own kind: the API server leaves `kind` out of
/// the items of a list, and metadata-only watches say `PartialObjectMetadata`. `kind` is what the
/// caller knows the object to be (`Deployment`); `None` reads it from the object.
pub fn managed_by_kind(object: &Value, known_kind: Option<&str>) -> Option<ManagedBy> {
    let meta = object.get("metadata")?;
    if let Some(id) = meta
        .pointer("/annotations")
        .and_then(|a| a.get(TRACKING_ANNOTATION))
        .and_then(Value::as_str)
    {
        // `<app>:<group>/<kind>:<namespace>/<name>`.
        let mut parts = id.splitn(3, ':');
        let app = parts.next()?;
        let group_kind = parts.next()?;
        let ns_name = parts.next()?;
        let kind = group_kind.rsplit('/').next()?;
        let (ns, name) = ns_name.split_once('/')?;
        let object_kind = known_kind
            .or_else(|| object.get("kind").and_then(Value::as_str))
            .unwrap_or_default();
        let object_ns = meta
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let object_name = meta.get("name").and_then(Value::as_str).unwrap_or_default();
        if kind == object_kind
            && name == object_name
            && (ns == object_ns || object_ns.is_empty())
            && !app.is_empty()
        {
            let (app_namespace, app_name) = split_app(app);
            return Some(ManagedBy {
                app_namespace,
                app_name,
                via_label: false,
            });
        }
        return None;
    }
    let label = meta
        .pointer("/labels")
        .and_then(|l| l.get(TRACKING_LABEL))
        .and_then(Value::as_str)?;
    let (app_namespace, app_name) = split_app(label);
    Some(ManagedBy {
        app_namespace,
        app_name,
        via_label: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tracking_annotation_must_match_the_object() {
        let deployment = json!({"kind": "Deployment", "metadata": {"name": "guestbook-ui", "namespace": "guestbook",
            "annotations": {TRACKING_ANNOTATION: "guestbook:apps/Deployment:guestbook/guestbook-ui"}}});
        assert_eq!(
            managed_by(&deployment),
            Some(ManagedBy {
                app_namespace: None,
                app_name: "guestbook".into(),
                via_label: false
            })
        );
        // Apps in any namespace: `<namespace>_<name>`.
        let service = json!({"kind": "Service", "metadata": {"name": "ui", "namespace": "team",
            "annotations": {TRACKING_ANNOTATION: "argocd-apps_guestbook-team:/Service:team/ui"}}});
        assert_eq!(
            managed_by(&service),
            Some(ManagedBy {
                app_namespace: Some("argocd-apps".into()),
                app_name: "guestbook-team".into(),
                via_label: false
            })
        );
        // A copy under another name isn't managed.
        let copy = json!({"kind": "Deployment", "metadata": {"name": "copy", "namespace": "guestbook",
            "annotations": {TRACKING_ANNOTATION: "guestbook:apps/Deployment:guestbook/guestbook-ui"}}});
        assert_eq!(managed_by(&copy), None);
        // Cluster-scoped objects have no namespace.
        let ns = json!({"kind": "Namespace", "metadata": {"name": "guestbook",
            "annotations": {TRACKING_ANNOTATION: "guestbook:/Namespace:/guestbook"}}});
        assert_eq!(managed_by(&ns).unwrap().app_name, "guestbook");
        let label = json!({"kind": "Service", "metadata": {"name": "x", "labels": {TRACKING_LABEL: "web"}}});
        assert!(managed_by(&label).unwrap().via_label);
        assert_eq!(
            managed_by(&json!({"kind": "Service", "metadata": {"name": "x"}})),
            None
        );
    }

    #[test]
    fn the_caller_can_name_the_kind_of_an_object_without_one() {
        // A list item has no `kind`; a metadata-only watch says PartialObjectMetadata.
        let tracked = json!({"metadata": {"name": "ui", "namespace": "guestbook",
            "annotations": {TRACKING_ANNOTATION: "guestbook:apps/Deployment:guestbook/ui"}}});
        assert_eq!(managed_by(&tracked), None);
        assert_eq!(
            managed_by_kind(&tracked, Some("Deployment"))
                .unwrap()
                .app_name,
            "guestbook"
        );
        // The given kind decides, not the object's own claim.
        let partial =
            json!({"kind": "PartialObjectMetadata", "metadata": tracked["metadata"].clone()});
        assert_eq!(
            managed_by_kind(&partial, Some("Deployment"))
                .unwrap()
                .app_name,
            "guestbook"
        );
        assert_eq!(managed_by_kind(&partial, Some("Service")), None);
    }
}
