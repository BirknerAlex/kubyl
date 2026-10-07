//! Describe text of the kinds of phase 24: device resources, admission policies and bindings,
//! the Gateway API, VerticalPodAutoscalers, EndpointSlices, Leases, PriorityClasses and
//! RuntimeClasses. CEL expressions are printed as written, indented under their field.

use serde_json::Value;

use super::{Out, or_none};
use crate::admission::{self, Binding, Match, Policy};
use crate::dra::{self, Claim, Class, Condition, Slice};
use crate::format::{array_at, int_at, map_pairs, str_at};
use crate::gateway::{self, Gateway, Route};
use crate::vpa::Vpa;

/// Describes `object` when its kind is one of these; `false` leaves it to the generic printer.
pub(super) fn describe(out: &mut Out, group: &str, kind: &str, object: &Value) -> bool {
    match (group, kind) {
        (dra::GROUP, "ResourceClaim") => claim(out, object),
        (dra::GROUP, "ResourceClaimTemplate") => requests(
            out,
            &dra::requests(object.pointer("/spec/spec/devices").unwrap_or(&Value::Null)),
        ),
        (dra::GROUP, "DeviceClass") => class(out, object),
        (dra::GROUP, "ResourceSlice") => slice(out, object),
        (admission::GROUP, "ValidatingAdmissionPolicy" | "MutatingAdmissionPolicy") => {
            policy(out, object)
        }
        (
            admission::GROUP,
            "ValidatingAdmissionPolicyBinding" | "MutatingAdmissionPolicyBinding",
        ) => binding(out, object),
        (admission::GROUP, "ValidatingWebhookConfiguration" | "MutatingWebhookConfiguration") => {
            webhooks(out, object)
        }
        (gateway::GROUP, "Gateway") => gateway_(out, object),
        (gateway::GROUP, "GatewayClass") => {
            let (controller, (state, _)) = gateway::gateway_class(object);
            out.field(0, "Controller", controller);
            out.field(0, "Accepted", state);
            let description = str_at(object, "/spec/description");
            if !description.is_empty() {
                out.field(0, "Description", description);
            }
        }
        (gateway::GROUP, kind) if gateway::is_route(gateway::GROUP, kind) => route(out, object),
        (crate::vpa::GROUP, "VerticalPodAutoscaler") => vpa(out, object),
        ("discovery.k8s.io", "EndpointSlice") => endpoint_slice(out, object),
        ("coordination.k8s.io", "Lease") => {
            out.field(0, "Holder", or_none(str_at(object, "/spec/holderIdentity")));
            out.field(0, "Renew Time", or_none(str_at(object, "/spec/renewTime")));
            out.field(
                0,
                "Acquire Time",
                or_none(str_at(object, "/spec/acquireTime")),
            );
            if let Some(seconds) = object.pointer("/spec/leaseDurationSeconds") {
                out.field(0, "Lease Duration", format!("{seconds}s"));
            }
            if let Some(transitions) = object.pointer("/spec/leaseTransitions") {
                out.field(0, "Transitions", transitions.to_string());
            }
        }
        ("scheduling.k8s.io", "PriorityClass") => {
            out.field(0, "Value", int_at(object, "/value").to_string());
            out.field(
                0,
                "GlobalDefault",
                (object.get("globalDefault").and_then(Value::as_bool) == Some(true)).to_string(),
            );
            out.field(
                0,
                "PreemptionPolicy",
                match str_at(object, "/preemptionPolicy") {
                    "" => "PreemptLowerPriority",
                    p => p,
                },
            );
            out.field(0, "Description", or_none(str_at(object, "/description")));
        }
        ("node.k8s.io", "RuntimeClass") => {
            out.field(0, "Handler", str_at(object, "/handler"));
            out.list(
                0,
                "Overhead",
                &map_pairs(object.pointer("/overhead/podFixed")),
            );
            out.list(
                0,
                "Node Selector",
                &map_pairs(object.pointer("/scheduling/nodeSelector")),
            );
        }
        _ => return false,
    }
    true
}

/// A CEL expression: one line after the key, or the lines below it, indented.
fn expression(out: &mut Out, indent: usize, key: &str, expression: &str) {
    let lines: Vec<&str> = expression.lines().collect();
    if lines.len() <= 1 {
        out.field(indent, key, expression);
    } else {
        out.line(indent, format!("{key}:"));
        for line in lines {
            out.line(indent + 2, line.trim_end());
        }
    }
}

fn conditions(out: &mut Out, indent: usize, conditions: &[Condition]) {
    if conditions.is_empty() {
        return;
    }
    out.line(indent, "Conditions:");
    out.line(indent + 2, format!("{:<24}{:<8}Reason", "Type", "Status"));
    for c in conditions {
        out.line(
            indent + 2,
            format!("{:<24}{:<8}{}", c.kind, c.status, c.reason),
        );
        if !c.message.is_empty() {
            out.line(indent + 4, &c.message);
        }
    }
}

fn requests(out: &mut Out, requests: &[dra::Request]) {
    out.line(0, "Requests:");
    if requests.is_empty() {
        out.line(2, "<none>");
    }
    for r in requests {
        request(out, 2, r);
    }
}

fn request(out: &mut Out, indent: usize, r: &dra::Request) {
    out.line(indent, format!("{}:", r.name));
    if !r.device_class.is_empty() {
        out.field(indent + 2, "Device Class", &r.device_class);
    }
    out.field(indent + 2, "Amount", r.amount());
    for (ix, selector) in r.selectors.iter().enumerate() {
        expression(out, indent + 2, &format!("Selector {}", ix + 1), selector);
    }
    if r.admin_access {
        out.field(indent + 2, "Admin Access", "true");
    }
    if !r.tolerations.is_empty() {
        out.field(indent + 2, "Tolerations", r.tolerations.join(", "));
    }
    for alternative in &r.alternatives {
        request(out, indent + 2, alternative);
    }
}

fn claim(out: &mut Out, object: &Value) {
    let claim = Claim::parse(object);
    out.field(0, "State", claim.state());
    requests(out, &claim.requests);
    match &claim.allocation {
        None => out.field(0, "Allocation", "<none>"),
        Some(allocation) => {
            out.line(0, "Allocation:");
            if let Some(node) = &allocation.node {
                out.field(2, "Node", node);
            } else if !allocation.node_selector.is_empty() {
                out.field(2, "Node Selector", allocation.node_selector.join(" or "));
            }
            if let Some(time) = &allocation.time {
                out.field(2, "Allocated", time);
            }
            out.line(2, "Devices:");
            for d in &allocation.devices {
                out.line(
                    4,
                    format!(
                        "{} → {}/{}/{}{}",
                        d.request,
                        d.driver,
                        d.pool,
                        d.device,
                        if d.admin_access { " (admin)" } else { "" }
                    ),
                );
            }
        }
    }
    let reserved: Vec<String> = claim
        .reserved_for
        .iter()
        .map(|c| format!("{}/{}", c.resource, c.name))
        .collect();
    out.list(0, "Reserved For", &reserved);
    if !claim.devices.is_empty() {
        out.line(0, "Device Status:");
        for d in &claim.devices {
            out.line(2, format!("{}/{}/{}:", d.driver, d.pool, d.device));
            if !d.data.is_empty() {
                out.field(4, "Data", d.data.join(", "));
            }
            if !d.network.is_empty() {
                out.field(4, "Network", d.network.join(", "));
            }
            conditions(out, 4, &d.conditions);
        }
    }
}

fn class(out: &mut Out, object: &Value) {
    let class = Class::parse(object);
    out.list(0, "Drivers", &class.drivers());
    if let Some(name) = &class.extended_resource_name {
        out.field(0, "Extended Resource", name);
    }
    out.line(0, "Selectors:");
    if class.selectors.is_empty() {
        out.line(2, "<none>");
    }
    for (ix, selector) in class.selectors.iter().enumerate() {
        expression(out, 2, &format!("CEL {}", ix + 1), selector);
    }
    for (driver, parameters) in &class.config {
        out.line(0, format!("Config ({driver}):"));
        for line in parameters.lines() {
            out.line(2, line);
        }
    }
}

fn slice(out: &mut Out, object: &Value) {
    let slice = Slice::parse(object);
    out.field(0, "Driver", &slice.driver);
    out.field(
        0,
        "Pool",
        format!(
            "{} (generation {}, {} slice{})",
            slice.pool,
            slice.generation,
            slice.slice_count,
            if slice.slice_count == 1 { "" } else { "s" }
        ),
    );
    out.field(0, "Node", or_none(&slice.node_label()));
    out.line(0, format!("Devices ({}):", slice.devices.len()));
    for d in &slice.devices {
        out.line(2, format!("{}:", d.name));
        let attributes: Vec<String> = d
            .attributes
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        if !attributes.is_empty() {
            out.field(4, "Attributes", attributes.join(", "));
        }
        let capacity: Vec<String> = d.capacity.iter().map(|(k, v)| format!("{k}={v}")).collect();
        if !capacity.is_empty() {
            out.field(4, "Capacity", capacity.join(", "));
        }
        if !d.taints.is_empty() {
            let taints: Vec<String> = d.taints.iter().map(dra::Taint::label).collect();
            out.field(4, "Taints", taints.join(", "));
        }
        if let Some(node) = &d.node {
            out.field(4, "Node", node);
        }
    }
}

fn match_resources(out: &mut Out, title: &str, m: &Option<Match>) {
    let Some(m) = m else {
        return;
    };
    out.line(0, format!("{title}:"));
    out.list(2, "Resource Rules", &m.rules);
    if !m.excluded.is_empty() {
        out.list(2, "Excluded", &m.excluded);
    }
    if let Some(selector) = &m.namespace_selector {
        out.field(2, "Namespaces", admission::selector_text(selector));
    }
    if let Some(selector) = &m.object_selector {
        out.field(2, "Objects", admission::selector_text(selector));
    }
    if !m.match_policy.is_empty() {
        out.field(2, "Match Policy", &m.match_policy);
    }
}

fn named(out: &mut Out, title: &str, items: &[admission::Named]) {
    if items.is_empty() {
        return;
    }
    out.line(0, format!("{title}:"));
    for item in items {
        expression(out, 2, &item.name, &item.expression);
    }
}

fn policy(out: &mut Out, object: &Value) {
    let policy = Policy::parse(object);
    out.field(0, "Failure Policy", &policy.failure_policy);
    if !policy.reinvocation_policy.is_empty() {
        out.field(0, "Reinvocation Policy", &policy.reinvocation_policy);
    }
    out.field(
        0,
        "Param Kind",
        policy.param_label().unwrap_or_else(|| "<none>".into()),
    );
    match_resources(out, "Match Constraints", &policy.constraints);
    named(out, "Match Conditions", &policy.match_conditions);
    named(out, "Variables", &policy.variables);
    if !policy.validations.is_empty() {
        out.line(0, "Validations:");
        for v in &policy.validations {
            expression(out, 2, "Expression", &v.expression);
            if !v.message.is_empty() {
                out.field(4, "Message", &v.message);
            }
            if !v.message_expression.is_empty() {
                expression(out, 4, "Message Expression", &v.message_expression);
            }
            if !v.reason.is_empty() {
                out.field(4, "Reason", &v.reason);
            }
        }
    }
    if !policy.mutations.is_empty() {
        out.line(0, "Mutations:");
        for m in &policy.mutations {
            expression(out, 2, &m.patch_type, &m.expression);
        }
    }
    if !policy.audit_annotations.is_empty() {
        out.line(0, "Audit Annotations:");
        for (key, value) in &policy.audit_annotations {
            expression(out, 2, key, value);
        }
    }
    if !policy.warnings.is_empty() {
        out.line(0, "Type Checking Warnings:");
        for (field, warning) in &policy.warnings {
            out.field(2, field, warning);
        }
    }
}

fn binding(out: &mut Out, object: &Value) {
    let binding = Binding::parse(object);
    out.field(0, "Policy", &binding.policy);
    if !binding.validation_actions.is_empty() {
        out.field(
            0,
            "Validation Actions",
            binding.validation_actions.join(", "),
        );
    }
    match &binding.param_ref {
        Some(param) => {
            out.field(0, "Param Ref", param.label());
            if !param.not_found.is_empty() {
                out.field(2, "Not Found Action", &param.not_found);
            }
        }
        None => out.field(0, "Param Ref", "<none>"),
    }
    match_resources(out, "Match Resources", &binding.resources);
}

fn webhooks(out: &mut Out, object: &Value) {
    out.line(0, "Webhooks:");
    for hook in admission::webhooks(object) {
        out.line(2, format!("{}:", hook.name));
        out.field(4, "Client", &hook.client);
        out.field(4, "Failure Policy", or_none(&hook.failure_policy));
        out.field(4, "Side Effects", or_none(&hook.side_effects));
        if hook.timeout > 0 {
            out.field(4, "Timeout", format!("{}s", hook.timeout));
        }
        out.list(4, "Rules", &hook.rules);
        if let Some(selector) = &hook.namespace_selector {
            out.field(4, "Namespaces", admission::selector_text(selector));
        }
        if let Some(selector) = &hook.object_selector {
            out.field(4, "Objects", admission::selector_text(selector));
        }
        if !hook.match_conditions.is_empty() {
            out.line(4, "Match Conditions:");
            for c in &hook.match_conditions {
                expression(out, 6, &c.name, &c.expression);
            }
        }
    }
}

fn gateway_(out: &mut Out, object: &Value) {
    let gw = Gateway::parse(object);
    out.field(0, "Class", &gw.class);
    out.field(0, "State", gw.state().0);
    out.list(0, "Addresses", &gw.addresses);
    out.line(0, "Listeners:");
    for l in &gw.listeners {
        out.line(2, format!("{}:", l.name));
        out.field(4, "Protocol", format!("{} :{}", l.protocol, l.port));
        if !l.hostname.is_empty() {
            out.field(4, "Hostname", &l.hostname);
        }
        out.field(4, "Allowed Routes", &l.allowed_namespaces);
        if let Some(tls) = &l.tls {
            out.field(4, "TLS", tls);
        }
        if let Some(attached) = l.attached_routes {
            out.field(4, "Attached Routes", attached.to_string());
        }
        out.field(4, "State", l.state().0);
    }
    conditions(out, 0, &gw.conditions);
}

fn route(out: &mut Out, object: &Value) {
    let route = Route::parse(object);
    out.field(0, "State", route.state().0);
    if !route.hostnames.is_empty() {
        out.field(0, "Hostnames", route.hostnames.join(", "));
    }
    out.line(0, "Parents:");
    for parent in &route.parents {
        let state = route
            .status_of(parent)
            .map(|s| s.state().0)
            .unwrap_or_else(|| "Pending".into());
        out.field(2, &parent.label(&route.namespace), state);
    }
    out.line(0, "Rules:");
    for (ix, rule) in route.rules.iter().enumerate() {
        let title = if rule.name.is_empty() {
            format!("Rule {}", ix + 1)
        } else {
            rule.name.clone()
        };
        out.line(2, format!("{title}:"));
        if !rule.matches.is_empty() {
            out.list(4, "Matches", &rule.matches);
        }
        if !rule.filters.is_empty() {
            out.field(4, "Filters", rule.filters.join(", "));
        }
        let backends: Vec<String> = rule
            .backends
            .iter()
            .map(|b| format!("{} (weight {})", b.label(&route.namespace), b.weight))
            .collect();
        out.list(4, "Backends", &backends);
    }
}

fn vpa(out: &mut Out, object: &Value) {
    let vpa = Vpa::parse(object);
    out.field(
        0,
        "Target",
        vpa.target_label().unwrap_or_else(|| "<none>".into()),
    );
    out.field(0, "Update Mode", &vpa.update_mode);
    if !vpa.policies.is_empty() {
        let policies: Vec<String> = vpa
            .policies
            .iter()
            .map(|(c, m)| format!("{c}: {m}"))
            .collect();
        out.list(0, "Container Policies", &policies);
    }
    out.line(0, "Recommendation:");
    if vpa.recommendations.is_empty() {
        out.line(2, "<none>");
    }
    for r in &vpa.recommendations {
        out.line(2, format!("{}:", r.container));
        for (key, bound) in [
            ("Lower Bound", &r.lower),
            ("Target", &r.target),
            ("Upper Bound", &r.upper),
            ("Uncapped Target", &r.uncapped),
        ] {
            if let Some(bound) = bound {
                out.field(4, key, bound.label());
            }
        }
    }
    conditions(out, 0, &vpa.conditions);
}

fn endpoint_slice(out: &mut Out, object: &Value) {
    out.field(
        0,
        "Service",
        or_none(str_at(
            object,
            "/metadata/labels/kubernetes.io~1service-name",
        )),
    );
    out.field(0, "AddressType", str_at(object, "/addressType"));
    out.list(0, "Ports", &crate::columns::endpoint_ports(object));
    out.line(0, "Endpoints:");
    for e in array_at(object, "/endpoints") {
        let addresses: Vec<&str> = array_at(e, "/addresses")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        out.line(2, format!("- Addresses: {}", addresses.join(", ")));
        let flag = |key: &str| match e.pointer(&format!("/conditions/{key}")) {
            Some(Value::Bool(b)) => b.to_string(),
            _ => "<unset>".into(),
        };
        out.line(
            4,
            format!(
                "Conditions: ready={} serving={} terminating={}",
                flag("ready"),
                flag("serving"),
                flag("terminating")
            ),
        );
        let target = e.get("targetRef");
        if let Some(target) = target {
            out.line(
                4,
                format!(
                    "TargetRef: {}/{}",
                    str_at(target, "/kind"),
                    str_at(target, "/name")
                ),
            );
        }
        let node = str_at(e, "/nodeName");
        if !node.is_empty() {
            out.line(4, format!("NodeName: {node}"));
        }
        let zone = str_at(e, "/zone");
        if !zone.is_empty() {
            out.line(4, format!("Zone: {zone}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::describe_as;
    use jiff::Timestamp;
    use serde_json::json;

    /// The text with runs of spaces collapsed (the alignment isn't what these tests check).
    fn squash(text: &str) -> String {
        let mut out = String::new();
        for (ix, part) in text.split(' ').filter(|p| !p.is_empty()).enumerate() {
            if ix > 0 {
                out.push(' ');
            }
            out.push_str(part);
        }
        out.replace("\n ", "\n")
    }

    #[test]
    fn claims_and_slices() {
        let claim = json!({"apiVersion": "resource.k8s.io/v1beta1", "kind": "ResourceClaim",
            "metadata": {"name": "shared-gpu", "namespace": "ns"},
            "spec": {"devices": {"requests": [{"name": "gpus", "deviceClassName": "gpu.example.com", "count": 2,
                "selectors": [{"cel": {"expression": "device.attributes['gpu.example.com'].model == 'A'"}}]}]}},
            "status": {"allocation": {"devices": {"results": [
                {"request": "gpus", "driver": "gpu.example.com", "pool": "worker", "device": "gpu-0"}]},
                "nodeSelector": {"nodeSelectorTerms": [{"matchFields": [
                    {"key": "metadata.name", "operator": "In", "values": ["worker"]}]}]}},
                "reservedFor": [{"resource": "pods", "name": "gpu-shared"}]}});
        let text = describe_as(
            "resource.k8s.io",
            "ResourceClaim",
            &claim,
            &[],
            Timestamp::now(),
        );
        assert!(
            squash(&text).contains("State: Allocated, reserved by 1"),
            "{text}"
        );
        assert!(
            squash(&text).contains("Device Class: gpu.example.com"),
            "{text}"
        );
        assert!(
            squash(&text).contains("Selector 1: device.attributes['gpu.example.com'].model == 'A'"),
            "{text}"
        );
        assert!(
            squash(&text).contains("gpus → gpu.example.com/worker/gpu-0"),
            "{text}"
        );
        assert!(
            squash(&text).contains("Reserved For: pods/gpu-shared"),
            "{text}"
        );

        let slice = json!({"kind": "ResourceSlice", "metadata": {"name": "s"},
            "spec": {"driver": "gpu.example.com", "nodeName": "worker",
                "pool": {"name": "worker", "generation": 1, "resourceSliceCount": 1},
                "devices": [{"name": "gpu-0", "attributes": {"model": {"string": "A"}},
                    "capacity": {"memory": {"value": "80Gi"}}}]}});
        let text = describe_as(
            "resource.k8s.io",
            "ResourceSlice",
            &slice,
            &[],
            Timestamp::now(),
        );
        assert!(
            squash(&text).contains("Pool: worker (generation 1, 1 slice)"),
            "{text}"
        );
        assert!(squash(&text).contains("Attributes: model=A"), "{text}");
        assert!(squash(&text).contains("Capacity: memory=80Gi"), "{text}");
    }

    #[test]
    fn policies_print_their_cel() {
        let policy = json!({"kind": "ValidatingAdmissionPolicy", "metadata": {"name": "p"},
            "spec": {"matchConstraints": {"resourceRules": [{"apiGroups": ["apps"], "apiVersions": ["v1"],
                "resources": ["deployments"], "operations": ["CREATE"]}]},
                "validations": [{"expression": "object.spec.replicas <= 5\n  && true", "reason": "Invalid"}]}});
        let text = describe_as(
            "admissionregistration.k8s.io",
            "ValidatingAdmissionPolicy",
            &policy,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("Failure Policy: Fail"), "{text}");
        assert!(
            squash(&text).contains("Resource Rules: apps/v1 deployments · CREATE"),
            "{text}"
        );
        assert!(
            squash(&text).contains("Expression:\nobject.spec.replicas <= 5\n&& true"),
            "{text}"
        );
        let binding = json!({"kind": "ValidatingAdmissionPolicyBinding", "metadata": {"name": "b"},
            "spec": {"policyName": "p", "validationActions": ["Deny"],
                "matchResources": {"namespaceSelector": {"matchLabels": {"team": "a"}}}}});
        let text = describe_as(
            "admissionregistration.k8s.io",
            "ValidatingAdmissionPolicyBinding",
            &binding,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("Policy: p"), "{text}");
        assert!(squash(&text).contains("Namespaces: team=a"), "{text}");
        let hooks = json!({"metadata": {"name": "h"}, "webhooks": [{"name": "w.example.com",
            "clientConfig": {"url": "https://x.example.com/hook?token=abc"},
            "matchConditions": [{"name": "c", "expression": "request.namespace != 'x'"}]}]});
        let text = describe_as(
            "admissionregistration.k8s.io",
            "ValidatingWebhookConfiguration",
            &hooks,
            &[],
            Timestamp::now(),
        );
        assert!(
            squash(&text).contains("c: request.namespace != 'x'"),
            "{text}"
        );
        assert!(!squash(&text).contains("token=abc"), "{text}");
    }

    #[test]
    fn gateway_api_vpa_and_cluster_kinds() {
        let gw = json!({"kind": "Gateway", "metadata": {"name": "g", "namespace": "ns"},
            "spec": {"gatewayClassName": "c", "listeners": [{"name": "http", "protocol": "HTTP", "port": 80}]},
            "status": {"listeners": [{"name": "http", "attachedRoutes": 1}]}});
        let text = describe_as(
            "gateway.networking.k8s.io",
            "Gateway",
            &gw,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("Protocol: HTTP :80"), "{text}");
        assert!(squash(&text).contains("Attached Routes: 1"), "{text}");
        let route = json!({"kind": "HTTPRoute", "metadata": {"name": "r", "namespace": "ns"},
            "spec": {"parentRefs": [{"name": "g"}], "rules": [{"backendRefs": [{"name": "web", "port": 80}]}]}});
        let text = describe_as(
            "gateway.networking.k8s.io",
            "HTTPRoute",
            &route,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("g: Pending"), "{text}");
        assert!(
            squash(&text).contains("Backends: web:80 (weight 1)"),
            "{text}"
        );
        let vpa = json!({"kind": "VerticalPodAutoscaler", "metadata": {"name": "v"},
            "spec": {"targetRef": {"kind": "Deployment", "name": "web", "apiVersion": "apps/v1"}},
            "status": {"recommendation": {"containerRecommendations": [{"containerName": "app",
                "target": {"cpu": "80m", "memory": "96Mi"}}]}}});
        let text = describe_as(
            "autoscaling.k8s.io",
            "VerticalPodAutoscaler",
            &vpa,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("Target: Deployment/web"), "{text}");
        assert!(squash(&text).contains("Target: 80m / 96Mi"), "{text}");
        let slice = json!({"metadata": {"name": "s", "labels": {"kubernetes.io/service-name": "web"}},
            "addressType": "IPv4", "ports": [{"name": "http", "port": 80}],
            "endpoints": [{"addresses": ["10.0.0.1"], "conditions": {"ready": true},
                "targetRef": {"kind": "Pod", "name": "web-1"}}]});
        let text = describe_as(
            "discovery.k8s.io",
            "EndpointSlice",
            &slice,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("Service: web"), "{text}");
        assert!(squash(&text).contains("TargetRef: Pod/web-1"), "{text}");
        let class =
            json!({"metadata": {"name": "high"}, "value": 1000, "preemptionPolicy": "Never"});
        let text = describe_as(
            "scheduling.k8s.io",
            "PriorityClass",
            &class,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("PreemptionPolicy: Never"), "{text}");
        let runtime = json!({"metadata": {"name": "rc"}, "handler": "runsc"});
        let text = describe_as(
            "node.k8s.io",
            "RuntimeClass",
            &runtime,
            &[],
            Timestamp::now(),
        );
        assert!(squash(&text).contains("Handler: runsc"), "{text}");
    }
}
