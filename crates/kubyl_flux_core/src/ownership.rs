//! Which Flux object manages an object: kustomize-controller labels what it applies with
//! `kustomize.toolkit.fluxcd.io/name|namespace`, helm-controller with
//! `helm.toolkit.fluxcd.io/name|namespace`.

use serde_json::Value;

use crate::kinds::FluxKind;
use crate::model::{
    HELM_NAME_LABEL, HELM_NAMESPACE_LABEL, KUSTOMIZE_NAME_LABEL, KUSTOMIZE_NAMESPACE_LABEL,
    ObjectRef,
};

/// The Flux object that manages `object` (by its labels), if any. A HelmRelease's objects
/// can also carry kustomize labels when a Kustomization applied the HelmRelease itself: the
/// closer manager (the HelmRelease) wins.
pub fn managed_by(object: &Value) -> Option<ObjectRef> {
    let labels = object.pointer("/metadata/labels")?;
    let label = |key: &str| {
        labels
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
    };
    if let (Some(name), Some(namespace)) = (label(HELM_NAME_LABEL), label(HELM_NAMESPACE_LABEL)) {
        return Some(ObjectRef::new(FluxKind::HelmRelease, namespace, name));
    }
    if let (Some(name), Some(namespace)) = (
        label(KUSTOMIZE_NAME_LABEL),
        label(KUSTOMIZE_NAMESPACE_LABEL),
    ) {
        return Some(ObjectRef::new(FluxKind::Kustomization, namespace, name));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use serde_json::json;

    #[test]
    fn labels_name_the_manager() {
        let ks = managed_by(&fixtures::managed_deployment()).unwrap();
        assert_eq!(ks.kind, Some(FluxKind::Kustomization));
        assert_eq!(ks.key(), "flux-demo/podinfo");
        let hr = managed_by(&fixtures::helm_deployment()).unwrap();
        assert_eq!(hr.kind, Some(FluxKind::HelmRelease));
        assert_eq!(hr.key(), "flux-demo/podinfo-helm");
        // Both: the HelmRelease is the closer manager.
        let both = json!({"metadata": {"labels": {
            KUSTOMIZE_NAME_LABEL: "apps", KUSTOMIZE_NAMESPACE_LABEL: "flux-system",
            HELM_NAME_LABEL: "redis", HELM_NAMESPACE_LABEL: "data"}}});
        assert_eq!(managed_by(&both).unwrap().kind, Some(FluxKind::HelmRelease));
        // A name without a namespace isn't Flux's.
        let half = json!({"metadata": {"labels": {KUSTOMIZE_NAME_LABEL: "apps"}}});
        assert_eq!(managed_by(&half), None);
        assert_eq!(managed_by(&json!({"metadata": {}})), None);
    }
}
