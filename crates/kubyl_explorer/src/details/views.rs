//! The resource views of phase 24 in the details: which related stores each kind needs, the
//! header state, and the sections. The kinds' own modules render them: [`super::devices`]
//! (DRA, a pod's claims, a node's devices), [`super::admission`] (policies, bindings,
//! webhooks), [`super::gateway`] (Gateway API, a Service's routes and EndpointSlices,
//! EndpointSlices) and [`super::vpa`].

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AnyElement, App, Context, EntityId};
use kubyl_core::{ClusterId, Gvr, ResourceRef, Tone};
use kubyl_kube::ConnectionManager;
use kubyl_kube::discovery::ApiResourceInfo;
use kubyl_resources::admission;
use kubyl_resources::dra::{self, Claim};
use kubyl_resources::format::str_at;
use kubyl_resources::gateway::{self, Gateway, Route};
use kubyl_resources::vpa::{self, Vpa};
use kubyl_resources::{StoreKey, object_key};
use kubyl_ui::Colors;
use serde_json::Value;

use super::{DetailsContent, Target};
use crate::catalog;

/// The state shown in the header of the kinds without their own header.
pub(super) fn header_state(target: &Target, object: &Value) -> Option<(String, Tone)> {
    let group = target.gvr.group.as_str();
    Some(match (group, target.kind.as_str()) {
        (dra::GROUP, "ResourceClaim") => {
            let state = Claim::parse(object).state();
            let tone = dra::claim_tone(&state);
            (state, tone)
        }
        (gateway::GROUP, "Gateway") => Gateway::parse(object).state(),
        (gateway::GROUP, "GatewayClass") => gateway::gateway_class(object).1,
        (gateway::GROUP, kind) if gateway::is_route(group, kind) => Route::parse(object).state(),
        (vpa::GROUP, "VerticalPodAutoscaler") => {
            let parsed = Vpa::parse(object);
            match gateway::condition(&parsed.conditions, "RecommendationProvided") {
                Some(c) if c.is_true() => ("RecommendationProvided".into(), Tone::Good),
                Some(_) => ("No recommendation".into(), Tone::Warning),
                None => ("No recommendation yet".into(), Tone::Warning),
            }
        }
        _ => return None,
    })
}

/// Facts next to the header state (`failure Fail`, `mode Off`).
pub(super) fn header_chips(target: &Target, object: &Value) -> Vec<String> {
    match (target.gvr.group.as_str(), target.kind.as_str()) {
        (admission::GROUP, "ValidatingAdmissionPolicy" | "MutatingAdmissionPolicy") => {
            let policy = admission::Policy::parse(object);
            let mut chips = vec![format!("failure {}", policy.failure_policy)];
            if target.kind == "MutatingAdmissionPolicy" {
                chips.push(format!("{} mutations", policy.mutations.len()));
                chips.push(format!("reinvocation {}", policy.reinvocation_policy));
            } else {
                chips.push(format!("{} validations", policy.validations.len()));
            }
            if let Some(param) = policy.param_label() {
                chips.push(format!("params {param}"));
            }
            chips
        }
        (
            admission::GROUP,
            "ValidatingAdmissionPolicyBinding" | "MutatingAdmissionPolicyBinding",
        ) => admission::Binding::parse(object).validation_actions,
        (vpa::GROUP, "VerticalPodAutoscaler") => {
            vec![format!("mode {}", Vpa::parse(object).update_mode)]
        }
        (dra::GROUP, "ResourceSlice") => vec![str_at(object, "/spec/driver").to_string()],
        _ => Vec::new(),
    }
}

/// What [`DetailsContent::load_views_related`] reads from the object itself to decide what to
/// watch: when it changes for the same target, the watches are redone.
pub(super) fn views_inputs(target: &Target, object: &Value) -> String {
    match (target.gvr.group.as_str(), target.kind.as_str()) {
        (gateway::GROUP, "Gateway") => format!("{:?}", gateway::attachable_routes(object)),
        (admission::GROUP, kind) if admission::is_binding(kind) => {
            let binding = admission::Binding::parse(object);
            let selector = binding
                .resources
                .is_some_and(|m| m.namespace_selector.is_some());
            format!("{} {selector}", binding.policy)
        }
        (vpa::GROUP, "VerticalPodAutoscaler") => format!("{:?}", Vpa::parse(object).target),
        ("", "Pod") => (!kubyl_resources::format::array_at(object, "/spec/resourceClaims")
            .is_empty())
        .to_string(),
        _ => String::new(),
    }
}

/// What the sections derive from related stores (a Gateway's attached routes, the claims
/// holding each device…), cached until one of those stores changes, so a render doesn't redo
/// the joins.
#[derive(Default)]
pub(super) struct Derived(RefCell<HashMap<&'static str, Built>>);

/// A derived value with the stamp it was built at.
type Built = (Stamp, Rc<dyn Any>);

/// The stores (and their generations) a derived value was built from.
type Stamp = Vec<(EntityId, u64)>;

impl DetailsContent {
    /// `build`'s value, rebuilt only when one of the named stores (by name prefix: `routes:`
    /// covers `routes:HTTPRoute`…) changed or appeared since it was built.
    pub(super) fn derived<T: 'static>(
        &self,
        key: &'static str,
        stores: &[&str],
        cx: &App,
        build: impl FnOnce(&Self) -> T,
    ) -> Rc<T> {
        let mut names: Vec<&String> = self
            .related
            .named
            .keys()
            .filter(|name| {
                stores
                    .iter()
                    .any(|s| name.as_str() == *s || (s.ends_with(':') && name.starts_with(s)))
            })
            .collect();
        names.sort();
        let stamp: Stamp = names
            .iter()
            .map(|name| {
                let store = &self.related.named[*name];
                (store.entity().entity_id(), store.read(cx).generation())
            })
            .collect();
        let cached = self
            .related
            .derived
            .0
            .borrow()
            .get(key)
            .filter(|(built, _)| *built == stamp)
            .and_then(|(_, value)| value.clone().downcast::<T>().ok());
        if let Some(value) = cached {
            return value;
        }
        let value = Rc::new(build(self));
        self.related
            .derived
            .0
            .borrow_mut()
            .insert(key, (stamp, value.clone() as Rc<dyn Any>));
        value
    }

    /// Whether the named store has loaded its first list.
    pub(super) fn named_ready(&self, name: &str, cx: &App) -> bool {
        self.related
            .named
            .get(name)
            .is_some_and(|store| store.read(cx).status().is_ready())
    }

    /// The served resource `(group, plural)` on `cluster` (preferred version), if any.
    pub(super) fn served(
        &self,
        cluster: &ClusterId,
        group: &str,
        resource: &str,
        cx: &App,
    ) -> Option<ApiResourceInfo> {
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(cluster)?;
        catalog::find(&discovery, group, resource).cloned()
    }

    /// A reference to an object of a served kind on the target's cluster.
    pub(super) fn reference(
        &self,
        target: &Target,
        group: &str,
        resource: &str,
        namespace: Option<String>,
        name: impl Into<String>,
        cx: &App,
    ) -> ResourceRef {
        let gvr = self
            .served(&target.cluster, group, resource, cx)
            .map(|info| info.gvr)
            .unwrap_or_else(|| Gvr::new(group, "v1", resource));
        ResourceRef::object(target.cluster.clone(), gvr, namespace, name.into())
    }

    /// Watches `(group, plural)` as the named store (when served): in `namespace`, or the
    /// whole cluster with `None`. `fields`: a field selector.
    pub(super) fn watch_named(
        &mut self,
        name: impl Into<String>,
        target: &Target,
        (group, resource): (&str, &str),
        namespace: Option<String>,
        fields: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(info) = self.served(&target.cluster, group, resource, cx) else {
            return false;
        };
        let namespace = if info.namespaced { namespace } else { None };
        let mut key = StoreKey::new(target.cluster.clone(), info.gvr, namespace);
        if let Some(fields) = fields {
            key = key.fields(fields);
        }
        let handle = self.acquire(key, cx);
        self.related.named.insert(name.into(), handle);
        true
    }

    /// The objects of a named store, sorted by namespace and name (empty until it loaded).
    pub(super) fn named(&self, name: &str, cx: &App) -> Vec<Arc<Value>> {
        let Some(store) = self.related.named.get(name) else {
            return Vec::new();
        };
        let mut objects: Vec<Arc<Value>> = store.read(cx).objects().values().cloned().collect();
        objects.sort_by(|a, b| {
            (
                kubyl_resources::format::namespace(a),
                kubyl_resources::format::name(a),
            )
                .cmp(&(
                    kubyl_resources::format::namespace(b),
                    kubyl_resources::format::name(b),
                ))
        });
        objects
    }

    /// Whether the named store exists (the kind is served).
    pub(super) fn has_named(&self, name: &str) -> bool {
        self.related.named.contains_key(name)
    }

    /// One object of a named store.
    pub(super) fn named_get(
        &self,
        name: &str,
        namespace: Option<&str>,
        object: &str,
        cx: &App,
    ) -> Option<Arc<Value>> {
        self.related
            .named
            .get(name)?
            .read(cx)
            .get(&object_key(namespace, object))
            .cloned()
    }

    /// Redoes the resource-view watches when the object changed what they depend on (a Gateway's
    /// listeners, a binding's policy, a VPA's target), e.g. after an edit in the YAML tab. The
    /// new stores are acquired before the old ones are dropped, so unchanged watches stay.
    pub(super) fn refresh_views_related(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Some(object)) = (self.target.clone(), self.object.clone()) else {
            return;
        };
        let Some(loaded) = &self.related.views_inputs else {
            return;
        };
        if *loaded == views_inputs(&target, &object) {
            return;
        }
        let old = std::mem::take(&mut self.related.named);
        self.related.derived = Derived::default();
        self.load_views_related(&target, &object, cx);
        drop(old);
    }

    /// Starts the watches the resource-view sections of `target` need.
    pub(super) fn load_views_related(
        &mut self,
        target: &Target,
        object: &Value,
        cx: &mut Context<Self>,
    ) {
        self.related.views_inputs = Some(views_inputs(target, object));
        let group = target.gvr.group.as_str();
        match (group, target.kind.as_str()) {
            ("", "Pod" | "Node") | (dra::GROUP, _) => self.load_devices(target, object, cx),
            (admission::GROUP, _) => self.load_admission(target, object, cx),
            ("", "Service") | (gateway::GROUP, _) => self.load_gateway(target, object, cx),
            (vpa::GROUP, "VerticalPodAutoscaler") => self.load_vpa(target, object, cx),
            _ => {}
        }
    }

    /// The resource-view sections of `target`.
    pub(super) fn render_views(
        &mut self,
        object: &Value,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let group = target.gvr.group.as_str();
        match (group, target.kind.as_str()) {
            ("", "Pod" | "Node") | (dra::GROUP, _) => {
                self.render_devices(object, target, colors, cx)
            }
            (admission::GROUP, _) => self.render_admission(object, target, colors, cx),
            ("", "Service") | (gateway::GROUP, _) | ("discovery.k8s.io", "EndpointSlice") => {
                self.render_gateway(object, target, colors, cx)
            }
            (vpa::GROUP, "VerticalPodAutoscaler") => self.render_vpa(object, target, colors, cx),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(group: &str, resource: &str, kind: &str) -> Target {
        Target {
            cluster: ClusterId::new("c"),
            gvr: Gvr::new(group, "v1", resource),
            kind: kind.into(),
            namespace: Some("ns".into()),
            name: "x".into(),
        }
    }

    #[test]
    fn watches_are_redone_when_their_inputs_change() {
        let gw = target(gateway::GROUP, "gateways", "Gateway");
        let mut object = json!({"spec": {"listeners": [{"name": "http", "protocol": "HTTP"}]}});
        let before = views_inputs(&gw, &object);
        // A label or the status changing doesn't matter.
        object["metadata"] = json!({"labels": {"a": "b"}});
        object["status"] = json!({"conditions": []});
        assert_eq!(views_inputs(&gw, &object), before);
        // A listener taking routes from every namespace does.
        object["spec"]["listeners"][0]["allowedRoutes"] = json!({"namespaces": {"from": "All"}});
        assert_ne!(views_inputs(&gw, &object), before);

        let binding = target(
            admission::GROUP,
            "validatingadmissionpolicybindings",
            "ValidatingAdmissionPolicyBinding",
        );
        let a = json!({"spec": {"policyName": "a"}});
        let b = json!({"spec": {"policyName": "b"}});
        assert_ne!(views_inputs(&binding, &a), views_inputs(&binding, &b));

        let vpa = target(
            vpa::GROUP,
            "verticalpodautoscalers",
            "VerticalPodAutoscaler",
        );
        let web = json!({"spec": {"targetRef": {"apiVersion": "apps/v1", "kind": "Deployment", "name": "web"}}});
        let api = json!({"spec": {"targetRef": {"apiVersion": "apps/v1", "kind": "Deployment", "name": "api"}}});
        assert_ne!(views_inputs(&vpa, &web), views_inputs(&vpa, &api));
        // Kinds whose watches don't depend on the object never redo them.
        let claim = target(dra::GROUP, "resourceclaims", "ResourceClaim");
        assert_eq!(views_inputs(&claim, &a), views_inputs(&claim, &b));
    }
}
