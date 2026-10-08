//! Column sets of phase 24: device resources (DRA), admission policies and bindings,
//! EndpointSlices, the Gateway API, VerticalPodAutoscalers, Leases, PriorityClasses and
//! RuntimeClasses.

use std::collections::HashMap;

use jiff::Timestamp;
use kubyl_base::columns::{CellValue, ColumnDef};
use kubyl_base::types::Tone;
use serde_json::Value;

use super::{CellLink, Kind, age_column, fixed, muted, name_column, number, status, text};
use crate::admission::{self, Binding, Policy};
use crate::dra::{self, Claim, Class, Slice};
use crate::format::{array_at, human_duration, int_at, seconds_since, str_at, timestamp};
use crate::gateway::{self, Gateway, Route};
use crate::status::status_tone;
use crate::vpa::{self, Vpa};

type Entry = (
    &'static str,
    &'static str,
    fn() -> Vec<ColumnDef>,
    super::CellFn,
    super::LinkFn,
);

pub(super) fn kinds() -> Vec<(&'static str, &'static str, Kind)> {
    let mut entries: Vec<Entry> = vec![
        (
            dra::GROUP,
            "ResourceClaim",
            claim_columns,
            claim_cell,
            no_link,
        ),
        (
            dra::GROUP,
            "ResourceClaimTemplate",
            template_columns,
            template_cell,
            no_link,
        ),
        (
            dra::GROUP,
            "DeviceClass",
            class_columns,
            class_cell,
            no_link,
        ),
        (
            dra::GROUP,
            "ResourceSlice",
            slice_columns,
            slice_cell,
            slice_link,
        ),
        (
            admission::GROUP,
            "ValidatingAdmissionPolicy",
            vap_columns,
            policy_cell,
            no_link,
        ),
        (
            admission::GROUP,
            "MutatingAdmissionPolicy",
            map_columns,
            policy_cell,
            no_link,
        ),
        (
            admission::GROUP,
            "ValidatingAdmissionPolicyBinding",
            vapb_columns,
            binding_cell,
            binding_link,
        ),
        (
            admission::GROUP,
            "MutatingAdmissionPolicyBinding",
            mapb_columns,
            binding_cell,
            binding_link,
        ),
        (
            "discovery.k8s.io",
            "EndpointSlice",
            endpointslice_columns,
            endpointslice_cell,
            endpointslice_link,
        ),
        (
            gateway::GROUP,
            "Gateway",
            gateway_columns,
            gateway_cell,
            gateway_link,
        ),
        (
            gateway::GROUP,
            "GatewayClass",
            gatewayclass_columns,
            gatewayclass_cell,
            no_link,
        ),
        (
            vpa::GROUP,
            "VerticalPodAutoscaler",
            vpa_columns,
            vpa_cell,
            vpa_link,
        ),
        (
            "coordination.k8s.io",
            "Lease",
            lease_columns,
            lease_cell,
            no_link,
        ),
        (
            "scheduling.k8s.io",
            "PriorityClass",
            priorityclass_columns,
            priorityclass_cell,
            no_link,
        ),
        (
            "node.k8s.io",
            "RuntimeClass",
            runtimeclass_columns,
            runtimeclass_cell,
            no_link,
        ),
    ];
    for (kind, _) in gateway::ROUTE_KINDS {
        entries.push((gateway::GROUP, kind, route_columns, route_cell, no_link));
    }
    entries
        .into_iter()
        .map(|(group, kind, columns, cell, link)| {
            (
                group,
                kind,
                Kind {
                    columns,
                    cell,
                    link,
                },
            )
        })
        .collect()
}

fn no_link(_: &Value, _: &str) -> Option<CellLink> {
    None
}

fn state(label: impl Into<String>) -> CellValue {
    let label = label.into();
    let tone = status_tone(&label);
    status(label, tone)
}

fn list(items: &[impl AsRef<str>]) -> CellValue {
    let items: Vec<&str> = items.iter().map(AsRef::as_ref).collect();
    if items.is_empty() {
        muted("<none>")
    } else {
        text(items.join(", "))
    }
}

/// What a [`super::RelatedColumn`] needs from the related objects, built once per change of
/// their store and looked up per cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum RelatedIndex {
    /// Nothing loaded (or a column without an index).
    #[default]
    Empty,
    /// The number of bindings per policy.
    Bindings(HashMap<String, usize>),
    /// The claims holding each device.
    Holders(dra::DeviceHolders),
    /// The claims of each pod.
    Pods(dra::PodClaims),
}

impl RelatedIndex {
    /// The index for `kind`'s `column` from the related objects.
    pub fn build<'a>(
        kind: &str,
        column: &str,
        related: impl IntoIterator<Item = &'a Value>,
    ) -> Self {
        match (kind, column) {
            ("ValidatingAdmissionPolicy" | "MutatingAdmissionPolicy", "bindings") => {
                let mut counts: HashMap<String, usize> = HashMap::new();
                for binding in related {
                    *counts
                        .entry(str_at(binding, "/spec/policyName").to_string())
                        .or_default() += 1;
                }
                Self::Bindings(counts)
            }
            ("ResourceSlice", "allocated") => Self::Holders(dra::DeviceHolders::index(related)),
            ("ResourceClaim", "health") => Self::Pods(dra::PodClaims::index(related)),
            _ => Self::Empty,
        }
    }
}

/// The cells [`super::RELATED_COLUMNS`] describes.
pub(super) fn related_cell(
    kind: &str,
    column: &str,
    object: &Value,
    index: &RelatedIndex,
) -> CellValue {
    match (kind, column, index) {
        (
            "ValidatingAdmissionPolicy" | "MutatingAdmissionPolicy",
            "bindings",
            RelatedIndex::Bindings(counts),
        ) => match counts.get(crate::format::name(object)).copied() {
            None | Some(0) => CellValue::Tinted {
                label: "0".into(),
                tone: Tone::Warning,
            },
            Some(count) => number(count as i64),
        },
        ("ResourceSlice", "allocated", RelatedIndex::Holders(holders)) => {
            let held = dra::allocations(&Slice::parse(object), holders).len();
            if held == 0 {
                muted("0")
            } else {
                number(held as i64)
            }
        }
        ("ResourceClaim", "health", RelatedIndex::Pods(pods)) => {
            match dra::claim_health(object, pods).label() {
                Some((label, tone)) => CellValue::Tinted {
                    label: label.into(),
                    tone,
                },
                None => CellValue::Empty,
            }
        }
        _ => CellValue::Empty,
    }
}

// ----- Device resources -----

fn claim_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("state", "State", 196.0),
        fixed("classes", "Device classes", 170.0).mono(),
        fixed("health", "Device health", 110.0),
        age_column(),
        fixed("devices", "Devices", 200.0).mono().wide(),
        fixed("node", "Node", 140.0).mono().wide(),
    ]
}

fn claim_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let claim = Claim::parse(object);
    match column {
        "state" => state(claim.state()),
        "classes" => list(&claim.device_classes()),
        "devices" => {
            let devices: Vec<&str> = claim
                .allocated()
                .iter()
                .map(|d| d.device.as_str())
                .collect();
            list(&devices)
        }
        "node" => match claim.allocation.as_ref().and_then(|a| a.node.clone()) {
            Some(node) => muted(node),
            None => CellValue::Empty,
        },
        _ => CellValue::Empty,
    }
}

fn template_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("classes", "Device classes", 200.0).mono(),
        fixed("requests", "Requests", 220.0),
        age_column(),
    ]
}

fn template_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let requests = dra::requests(object.pointer("/spec/spec/devices").unwrap_or(&Value::Null));
    match column {
        "classes" => list(&dra::device_classes(&requests)),
        "requests" => {
            let items: Vec<String> = requests
                .iter()
                .map(|r| format!("{}: {}", r.name, r.amount()))
                .collect();
            list(&items)
        }
        _ => CellValue::Empty,
    }
}

fn class_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("drivers", "Drivers", 200.0).mono(),
        fixed("extended", "Extended resource", 180.0).mono(),
        fixed("selectors", "Selectors", 80.0).mono().align_end(),
        age_column(),
    ]
}

fn class_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let class = Class::parse(object);
    match column {
        "drivers" => list(&class.drivers()),
        "extended" => match class.extended_resource_name {
            Some(name) => text(name),
            None => muted("<none>"),
        },
        "selectors" => number(class.selectors.len() as i64),
        _ => CellValue::Empty,
    }
}

fn slice_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("driver", "Driver", 150.0).mono(),
        fixed("pool", "Pool", 140.0).mono(),
        fixed("node", "Node", 150.0).mono(),
        fixed("devices", "Devices", 70.0).mono().align_end(),
        fixed("allocated", "Allocated", 80.0).mono().align_end(),
        age_column(),
    ]
}

fn slice_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let slice = Slice::parse(object);
    match column {
        "driver" => muted(slice.driver),
        "pool" => muted(slice.pool),
        "node" => match slice.node_label().as_str() {
            "" => muted("<none>"),
            label => text(label),
        },
        "devices" => number(slice.devices.len() as i64),
        _ => CellValue::Empty,
    }
}

fn slice_link(object: &Value, column: &str) -> Option<CellLink> {
    if column != "node" {
        return None;
    }
    let node = str_at(object, "/spec/nodeName");
    (!node.is_empty()).then(|| CellLink {
        group: String::new(),
        version: Some("v1".into()),
        resource: "nodes".into(),
        namespace: None,
        name: node.to_string(),
    })
}

// ----- Admission policies -----

fn vap_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("validations", "Validations", 90.0).mono().align_end(),
        fixed("failure", "Failure policy", 110.0),
        fixed("params", "Params", 130.0),
        fixed("bindings", "Bindings", 80.0).mono().align_end(),
        age_column(),
    ]
}

fn map_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("mutations", "Mutations", 84.0).mono().align_end(),
        fixed("failure", "Failure policy", 110.0),
        fixed("reinvocation", "Reinvocation", 100.0),
        fixed("bindings", "Bindings", 80.0).mono().align_end(),
        age_column(),
        fixed("params", "Params", 130.0).wide(),
    ]
}

fn policy_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let policy = Policy::parse(object);
    match column {
        "validations" => {
            if policy.warnings.is_empty() {
                number(policy.validations.len() as i64)
            } else {
                CellValue::Tinted {
                    label: format!(
                        "{} · {} warning{}",
                        policy.validations.len(),
                        policy.warnings.len(),
                        if policy.warnings.len() == 1 { "" } else { "s" }
                    )
                    .into(),
                    tone: Tone::Warning,
                }
            }
        }
        "mutations" => number(policy.mutations.len() as i64),
        "failure" => match policy.failure_policy.as_str() {
            "Ignore" => muted("Ignore"),
            other => text(other),
        },
        "reinvocation" => text(policy.reinvocation_policy),
        "params" => match policy.param_label() {
            Some(label) => text(label),
            None => muted("<none>"),
        },
        _ => CellValue::Empty,
    }
}

fn vapb_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("policy", "Policy", 220.0).mono(),
        fixed("actions", "Validation actions", 140.0),
        fixed("params", "Param ref", 180.0).mono(),
        age_column(),
    ]
}

fn mapb_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("policy", "Policy", 220.0).mono(),
        fixed("params", "Param ref", 180.0).mono(),
        age_column(),
    ]
}

fn binding_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let binding = Binding::parse(object);
    match column {
        "policy" => text(binding.policy),
        "actions" => {
            let deny = binding.validation_actions.iter().any(|a| a == "Deny");
            let label = binding.validation_actions.join(", ");
            if deny {
                CellValue::Tinted {
                    label: label.into(),
                    tone: Tone::Bad,
                }
            } else {
                list(&binding.validation_actions)
            }
        }
        "params" => match binding.param_ref {
            Some(param) => text(param.label()),
            None => muted("<none>"),
        },
        _ => CellValue::Empty,
    }
}

fn binding_link(object: &Value, column: &str) -> Option<CellLink> {
    if column != "policy" {
        return None;
    }
    let policy = str_at(object, "/spec/policyName");
    let kind = str_at(object, "/kind");
    // Watch caches may drop `kind`: a validating binding has validation actions.
    let kind = if kind.is_empty() && object.pointer("/spec/validationActions").is_none() {
        "MutatingAdmissionPolicyBinding"
    } else {
        kind
    };
    (!policy.is_empty()).then(|| CellLink {
        group: admission::GROUP.into(),
        version: None,
        resource: admission::policy_resource(kind).into(),
        namespace: None,
        name: policy.to_string(),
    })
}

// ----- Network -----

fn endpointslice_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("service", "Service", 180.0).mono(),
        fixed("address_type", "Address type", 100.0),
        fixed("endpoints", "Ready", 70.0).mono(),
        fixed("ports", "Ports", 160.0).mono(),
        age_column(),
        fixed("addresses", "Addresses", 220.0).mono().wide(),
    ]
}

/// `(ready, total)` endpoints; one without a ready condition counts as ready.
pub fn endpoint_counts(slice: &Value) -> (i64, i64) {
    let endpoints = array_at(slice, "/endpoints");
    let ready = endpoints
        .iter()
        .filter(|e| e.pointer("/conditions/ready").and_then(Value::as_bool) != Some(false))
        .count();
    (ready as i64, endpoints.len() as i64)
}

/// `http 80/TCP, 9090/TCP`.
pub fn endpoint_ports(slice: &Value) -> Vec<String> {
    array_at(slice, "/ports")
        .iter()
        .map(|p| {
            let protocol = match str_at(p, "/protocol") {
                "" => "TCP",
                p => p,
            };
            let port = match p.get("port").and_then(Value::as_i64) {
                Some(port) => port.to_string(),
                None => "*".into(),
            };
            match str_at(p, "/name") {
                "" => format!("{port}/{protocol}"),
                name => format!("{name} {port}/{protocol}"),
            }
        })
        .collect()
}

fn endpointslice_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "service" => match str_at(object, "/metadata/labels/kubernetes.io~1service-name") {
            "" => muted("<none>"),
            service => text(service),
        },
        "address_type" => text(str_at(object, "/addressType")),
        "endpoints" => {
            let (ready, total) = endpoint_counts(object);
            super::ratio(ready, total)
        }
        "ports" => list(&endpoint_ports(object)),
        "addresses" => {
            let addresses: Vec<&str> = array_at(object, "/endpoints")
                .iter()
                .flat_map(|e| array_at(e, "/addresses"))
                .filter_map(Value::as_str)
                .collect();
            list(&addresses)
        }
        _ => CellValue::Empty,
    }
}

fn endpointslice_link(object: &Value, column: &str) -> Option<CellLink> {
    if column != "service" {
        return None;
    }
    let service = str_at(object, "/metadata/labels/kubernetes.io~1service-name");
    (!service.is_empty()).then(|| CellLink {
        group: String::new(),
        version: Some("v1".into()),
        resource: "services".into(),
        namespace: crate::format::namespace(object).map(String::from),
        name: service.to_string(),
    })
}

fn gateway_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("class", "Class", 120.0).mono(),
        fixed("addresses", "Addresses", 150.0).mono(),
        fixed("programmed", "Programmed", 150.0),
        fixed("listeners", "Listeners", 76.0).mono().align_end(),
        fixed("routes", "Routes", 66.0).mono().align_end(),
        age_column(),
    ]
}

fn gateway_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let gw = Gateway::parse(object);
    match column {
        "class" => text(gw.class),
        "addresses" => {
            if gw.addresses.is_empty() {
                muted("<pending>")
            } else {
                text(gw.addresses.join(","))
            }
        }
        "programmed" => {
            let (label, tone) = gw.state();
            status(label, tone)
        }
        "listeners" => number(gw.listeners.len() as i64),
        "routes" => match gw.attached_routes() {
            Some(count) => number(count),
            None => CellValue::Empty,
        },
        _ => CellValue::Empty,
    }
}

fn gateway_link(object: &Value, column: &str) -> Option<CellLink> {
    if column != "class" {
        return None;
    }
    let class = str_at(object, "/spec/gatewayClassName");
    (!class.is_empty()).then(|| CellLink {
        group: gateway::GROUP.into(),
        version: None,
        resource: "gatewayclasses".into(),
        namespace: None,
        name: class.to_string(),
    })
}

fn gatewayclass_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("controller", "Controller", 280.0).mono(),
        fixed("accepted", "Accepted", 150.0),
        age_column(),
    ]
}

fn gatewayclass_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let (controller, (label, tone)) = gateway::gateway_class(object);
    match column {
        "controller" => muted(controller),
        "accepted" => status(label, tone),
        _ => CellValue::Empty,
    }
}

fn route_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("parents", "Parents", 170.0).mono(),
        fixed("hostnames", "Hostnames", 200.0).mono(),
        fixed("backends", "Backends", 190.0).mono(),
        fixed("accepted", "Accepted", 150.0),
        age_column(),
    ]
}

fn route_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let route = Route::parse(object);
    match column {
        "parents" => {
            let parents: Vec<String> = route
                .parents
                .iter()
                .map(|p| p.label(&route.namespace))
                .collect();
            list(&parents)
        }
        "hostnames" => {
            if route.hostnames.is_empty() {
                muted("*")
            } else {
                text(route.hostnames.join(","))
            }
        }
        "backends" => {
            let mut backends: Vec<String> = Vec::new();
            for backend in route.backends() {
                let label = backend.label(&route.namespace);
                if !backends.contains(&label) {
                    backends.push(label);
                }
            }
            list(&backends)
        }
        "accepted" => {
            let (label, tone) = route.state();
            status(label, tone)
        }
        _ => CellValue::Empty,
    }
}

// ----- Workloads -----

fn vpa_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("target", "Target", 190.0).mono(),
        fixed("mode", "Update mode", 130.0),
        fixed("recommendation", "Recommendation (CPU / memory)", 240.0).mono(),
        age_column(),
        fixed("provided", "Provided", 90.0).wide(),
    ]
}

fn vpa_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    let vpa = Vpa::parse(object);
    match column {
        "target" => match vpa.target_label() {
            Some(label) => text(label),
            None => muted("<none>"),
        },
        "mode" => match vpa.update_mode.as_str() {
            "Off" => muted("Off"),
            mode => text(mode),
        },
        "recommendation" => match vpa.recommendation_label().as_str() {
            "" => CellValue::Empty,
            label => text(label),
        },
        "provided" => match crate::gateway::condition(&vpa.conditions, "RecommendationProvided") {
            Some(c) => text(c.status.clone()),
            None => CellValue::Empty,
        },
        _ => CellValue::Empty,
    }
}

fn vpa_link(object: &Value, column: &str) -> Option<CellLink> {
    if column != "target" {
        return None;
    }
    let target = Vpa::parse(object).target?;
    let (group, version) = target.group_version();
    Some(CellLink {
        group: group.to_string(),
        version: Some(version.to_string()),
        resource: target.resource()?.to_string(),
        namespace: crate::format::namespace(object).map(String::from),
        name: target.name.clone(),
    })
}

// ----- Cluster -----

fn lease_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("holder", "Holder", 300.0).mono(),
        fixed("renewed", "Renewed", 80.0).mono(),
        fixed("duration", "Duration", 76.0).mono().wide(),
        age_column(),
    ]
}

fn lease_cell(object: &Value, column: &str, now: Timestamp) -> CellValue {
    match column {
        "holder" => match str_at(object, "/spec/holderIdentity") {
            "" => muted("<none>"),
            holder => text(holder),
        },
        "renewed" => match timestamp(str_at(object, "/spec/renewTime")) {
            Some(time) => {
                let since = seconds_since(time, now);
                let duration = int_at(object, "/spec/leaseDurationSeconds");
                let label = format!("{} ago", human_duration(since));
                // A lease past its duration is free for others to take.
                if duration > 0 && since > duration {
                    CellValue::Tinted {
                        label: label.into(),
                        tone: Tone::Warning,
                    }
                } else {
                    muted(label)
                }
            }
            None => CellValue::Empty,
        },
        "duration" => match object.pointer("/spec/leaseDurationSeconds") {
            Some(seconds) => muted(format!("{}s", seconds.as_i64().unwrap_or_default())),
            None => CellValue::Empty,
        },
        _ => CellValue::Empty,
    }
}

fn priorityclass_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("value", "Value", 110.0).mono().align_end(),
        fixed("global_default", "Global default", 110.0),
        fixed("preemption", "Preemption", 170.0),
        age_column(),
    ]
}

fn priorityclass_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "value" => number(int_at(object, "/value")),
        "global_default" => {
            if object.get("globalDefault").and_then(Value::as_bool) == Some(true) {
                status("True", Tone::Info)
            } else {
                muted("False")
            }
        }
        "preemption" => match str_at(object, "/preemptionPolicy") {
            "" => text("PreemptLowerPriority"),
            "Never" => muted("Never"),
            policy => text(policy),
        },
        _ => CellValue::Empty,
    }
}

fn runtimeclass_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("handler", "Handler", 140.0).mono(),
        fixed("overhead", "Overhead", 170.0).mono(),
        fixed("node_selector", "Node selector", 180.0).mono(),
        age_column(),
    ]
}

fn runtimeclass_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "handler" => text(str_at(object, "/handler")),
        "overhead" => {
            let pairs = crate::format::map_pairs(object.pointer("/overhead/podFixed"));
            if pairs.is_empty() {
                muted("<none>")
            } else {
                text(pairs.join(", "))
            }
        }
        "node_selector" => {
            let pairs = crate::format::map_pairs(object.pointer("/scheduling/nodeSelector"));
            if pairs.is_empty() {
                muted("<none>")
            } else {
                muted(pairs.join(","))
            }
        }
        _ => CellValue::Empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kind(group: &str, name: &str) -> Kind {
        kinds()
            .into_iter()
            .find(|(g, k, _)| *g == group && *k == name)
            .map(|(_, _, kind)| kind)
            .unwrap_or_else(|| panic!("no columns for {name}"))
    }

    fn ids(kind: &Kind) -> Vec<String> {
        kind.columns().iter().map(|c| c.id.to_string()).collect()
    }

    /// A related cell through the index the list builds.
    fn related(kind: &str, column: &str, object: &Value, objects: &[&Value]) -> CellValue {
        let index = RelatedIndex::build(kind, column, objects.iter().copied());
        related_cell(kind, column, object, &index)
    }

    fn tinted(label: &str, tone: Tone) -> CellValue {
        CellValue::Tinted {
            label: label.into(),
            tone,
        }
    }

    #[test]
    fn device_resources() {
        let claims = kind(dra::GROUP, "ResourceClaim");
        assert_eq!(
            ids(&claims),
            [
                "name", "state", "classes", "health", "age", "devices", "node"
            ]
        );
        for (version, request) in [
            (
                "v1",
                json!({"name": "gpus", "exactly": {"deviceClassName": "gpu.example.com", "count": 2}}),
            ),
            (
                "v1beta2",
                json!({"name": "gpus", "exactly": {"deviceClassName": "gpu.example.com", "count": 2}}),
            ),
            (
                "v1beta1",
                json!({"name": "gpus", "deviceClassName": "gpu.example.com", "count": 2}),
            ),
        ] {
            let claim = json!({"apiVersion": format!("resource.k8s.io/{version}"),
                "metadata": {"name": "shared-gpu", "namespace": "ns"},
                "spec": {"devices": {"requests": [request]}},
                "status": {"allocation": {"devices": {"results": [
                    {"request": "gpus", "driver": "gpu.example.com", "pool": "worker", "device": "gpu-0"},
                    {"request": "gpus", "driver": "gpu.example.com", "pool": "worker", "device": "gpu-1"}]},
                    "nodeSelector": {"nodeSelectorTerms": [{"matchFields": [
                        {"key": "metadata.name", "operator": "In", "values": ["worker"]}]}]}},
                    "reservedFor": [{"resource": "pods", "name": "gpu-shared"}]}});
            assert_eq!(
                claims.cell(&claim, "state"),
                status("Allocated, reserved by 1", Tone::Good),
                "{version}"
            );
            assert_eq!(claims.cell(&claim, "classes"), text("gpu.example.com"));
            assert_eq!(claims.cell(&claim, "devices"), text("gpu-0, gpu-1"));
            assert_eq!(claims.cell(&claim, "node"), muted("worker"));
            // Filled in by the list from the namespace's pods.
            assert_eq!(claims.cell(&claim, "health"), CellValue::Empty);
            let pod = json!({"metadata": {"name": "gpu-shared", "namespace": "ns"},
                "spec": {"resourceClaims": [{"name": "gpus", "resourceClaimName": "shared-gpu"}]},
                "status": {"containerStatuses": [{"allocatedResourcesStatus": [
                    {"name": "claim:gpus", "resources": [{"health": "Healthy"}]}]}]}});
            assert_eq!(
                related("ResourceClaim", "health", &claim, &[&pod]),
                tinted("2 healthy", Tone::Good)
            );
            // The same pod name in another namespace isn't the claim's.
            let mut elsewhere = pod.clone();
            elsewhere["metadata"]["namespace"] = json!("other");
            assert_eq!(
                related("ResourceClaim", "health", &claim, &[&elsewhere]),
                CellValue::Empty
            );
        }
        let pending =
            json!({"spec": {"devices": {"requests": [{"name": "a", "deviceClassName": "x"}]}}});
        assert_eq!(
            claims.cell(&pending, "state"),
            status("Pending", Tone::Warning)
        );
        assert_eq!(claims.cell(&pending, "devices"), muted("<none>"));

        let templates = kind(dra::GROUP, "ResourceClaimTemplate");
        let template = json!({"spec": {"spec": {"devices": {"requests": [
            {"name": "gpu", "exactly": {"deviceClassName": "gpu.example.com"}}]}}}});
        assert_eq!(
            templates.cell(&template, "classes"),
            text("gpu.example.com")
        );
        assert_eq!(
            templates.cell(&template, "requests"),
            text("gpu: exactly 1")
        );

        let classes = kind(dra::GROUP, "DeviceClass");
        let class = json!({"spec": {"selectors": [{"cel": {"expression": "device.driver == 'gpu.example.com'"}}]}});
        assert_eq!(classes.cell(&class, "drivers"), text("gpu.example.com"));
        assert_eq!(classes.cell(&class, "extended"), muted("<none>"));
        assert_eq!(classes.cell(&class, "selectors"), text("1"));

        let slices = kind(dra::GROUP, "ResourceSlice");
        let slice = json!({"spec": {"driver": "gpu.example.com", "nodeName": "worker",
            "pool": {"name": "worker"}, "devices": [{"name": "gpu-0"}, {"name": "gpu-1"}, {"name": "gpu-2"}]}});
        assert_eq!(slices.cell(&slice, "node"), text("worker"));
        assert_eq!(slices.cell(&slice, "devices"), text("3"));
        assert_eq!(slices.link(&slice, "node").unwrap().resource, "nodes");
        assert!(slices.link(&slice, "driver").is_none());
        let claim = json!({"metadata": {"name": "c", "namespace": "ns"},
            "status": {"allocation": {"devices": {"results": [
                {"driver": "gpu.example.com", "pool": "worker", "device": "gpu-1"}]}}}});
        assert_eq!(
            related("ResourceSlice", "allocated", &slice, &[&claim]),
            text("1")
        );
        // A device two claims share counts once.
        let admin = json!({"metadata": {"name": "admin", "namespace": "ops"},
            "status": {"allocation": {"devices": {"results": [
                {"driver": "gpu.example.com", "pool": "worker", "device": "gpu-1", "adminAccess": true}]}}}});
        assert_eq!(
            related("ResourceSlice", "allocated", &slice, &[&claim, &admin]),
            text("1")
        );
        assert_eq!(
            related("ResourceSlice", "allocated", &slice, &[]),
            muted("0")
        );
        assert!(super::super::is_related(
            dra::GROUP,
            "ResourceSlice",
            "allocated"
        ));
        assert!(!super::super::is_related(
            dra::GROUP,
            "ResourceSlice",
            "devices"
        ));
    }

    #[test]
    fn admission_policies_and_bindings() {
        let vap = kind(admission::GROUP, "ValidatingAdmissionPolicy");
        let policy = json!({"metadata": {"name": "p"}, "spec": {"failurePolicy": "Ignore",
            "validations": [{"expression": "true"}, {"expression": "false"}],
            "paramKind": {"apiVersion": "v1", "kind": "ConfigMap"}},
            "status": {"typeChecking": {"expressionWarnings": [{"fieldRef": "x", "warning": "y"}]}}});
        assert_eq!(
            vap.cell(&policy, "validations"),
            tinted("2 · 1 warning", Tone::Warning)
        );
        assert_eq!(vap.cell(&policy, "failure"), muted("Ignore"));
        assert_eq!(vap.cell(&policy, "params"), text("ConfigMap"));
        let binding = json!({"metadata": {"name": "b"}, "spec": {"policyName": "p", "validationActions": ["Deny"]}});
        assert_eq!(
            related(
                "ValidatingAdmissionPolicy",
                "bindings",
                &policy,
                &[&binding]
            ),
            text("1")
        );
        assert_eq!(
            related("ValidatingAdmissionPolicy", "bindings", &policy, &[]),
            tinted("0", Tone::Warning)
        );

        let map = kind(admission::GROUP, "MutatingAdmissionPolicy");
        let mutating = json!({"spec": {"mutations": [{"patchType": "JSONPatch"}], "reinvocationPolicy": "IfNeeded"}});
        assert_eq!(map.cell(&mutating, "mutations"), text("1"));
        assert_eq!(map.cell(&mutating, "failure"), text("Fail"));
        assert_eq!(map.cell(&mutating, "reinvocation"), text("IfNeeded"));

        let vapb = kind(admission::GROUP, "ValidatingAdmissionPolicyBinding");
        assert_eq!(vapb.cell(&binding, "policy"), text("p"));
        assert_eq!(vapb.cell(&binding, "actions"), tinted("Deny", Tone::Bad));
        assert_eq!(vapb.cell(&binding, "params"), muted("<none>"));
        let link = vapb.link(&binding, "policy").unwrap();
        assert_eq!(link.resource, "validatingadmissionpolicies");
        assert_eq!(link.version, None);
        let mapb = kind(admission::GROUP, "MutatingAdmissionPolicyBinding");
        let mutating_binding = json!({"kind": "MutatingAdmissionPolicyBinding",
            "spec": {"policyName": "m", "paramRef": {"name": "cfg", "namespace": "ns"}}});
        assert_eq!(mapb.cell(&mutating_binding, "params"), text("ns/cfg"));
        assert_eq!(
            mapb.link(&mutating_binding, "policy").unwrap().resource,
            "mutatingadmissionpolicies"
        );
    }

    #[test]
    fn endpoint_slices_and_the_gateway_api() {
        let slices = kind("discovery.k8s.io", "EndpointSlice");
        let slice = json!({"metadata": {"name": "web-x8f2k", "namespace": "ns",
            "labels": {"kubernetes.io/service-name": "web"}},
            "addressType": "IPv4",
            "endpoints": [{"addresses": ["10.0.0.1"], "conditions": {"ready": true}},
                          {"addresses": ["10.0.0.2"], "conditions": {"ready": false}},
                          {"addresses": ["10.0.0.3"]}],
            "ports": [{"name": "http", "port": 80, "protocol": "TCP"}, {"port": 9090}]});
        assert_eq!(slices.cell(&slice, "service"), text("web"));
        assert_eq!(slices.cell(&slice, "endpoints"), tinted("2/3", Tone::Bad));
        assert_eq!(slices.cell(&slice, "ports"), text("http 80/TCP, 9090/TCP"));
        let link = slices.link(&slice, "service").unwrap();
        assert_eq!(
            (
                link.resource.as_str(),
                link.namespace.as_deref(),
                link.name.as_str()
            ),
            ("services", Some("ns"), "web")
        );

        let gateways = kind(gateway::GROUP, "Gateway");
        let gw = json!({"spec": {"gatewayClassName": "kubyl-fake", "listeners": [{"name": "http"}]},
            "status": {"addresses": [{"value": "172.18.0.240"}],
                "conditions": [{"type": "Programmed", "status": "True"}],
                "listeners": [{"name": "http", "attachedRoutes": 2}]}});
        assert_eq!(gateways.cell(&gw, "addresses"), text("172.18.0.240"));
        assert_eq!(
            gateways.cell(&gw, "programmed"),
            status("Programmed", Tone::Good)
        );
        assert_eq!(gateways.cell(&gw, "routes"), text("2"));
        assert_eq!(
            gateways.link(&gw, "class").unwrap().resource,
            "gatewayclasses"
        );

        let classes = kind(gateway::GROUP, "GatewayClass");
        let class = json!({"spec": {"controllerName": "kubyl.dev/fake"}});
        assert_eq!(
            classes.cell(&class, "accepted"),
            status("Pending", Tone::Warning)
        );

        for (route_kind, _) in gateway::ROUTE_KINDS {
            let routes = kind(gateway::GROUP, route_kind);
            assert_eq!(
                ids(&routes),
                [
                    "name",
                    "parents",
                    "hostnames",
                    "backends",
                    "accepted",
                    "age"
                ]
            );
        }
        let routes = kind(gateway::GROUP, "HTTPRoute");
        let route = json!({"kind": "HTTPRoute", "metadata": {"namespace": "ns"},
            "spec": {"parentRefs": [{"name": "gw", "sectionName": "http"}],
                "rules": [{"backendRefs": [{"name": "web", "port": 80}, {"name": "web", "port": 80}]}]},
            "status": {"parents": [{"parentRef": {"name": "gw", "sectionName": "http"},
                "conditions": [{"type": "Accepted", "status": "True"}]}]}});
        assert_eq!(routes.cell(&route, "parents"), text("gw/http"));
        assert_eq!(routes.cell(&route, "hostnames"), muted("*"));
        assert_eq!(routes.cell(&route, "backends"), text("web:80"));
        assert_eq!(
            routes.cell(&route, "accepted"),
            status("Accepted", Tone::Good)
        );
    }

    #[test]
    fn vpas_and_cluster_kinds() {
        let vpas = kind(vpa::GROUP, "VerticalPodAutoscaler");
        let object = json!({"metadata": {"namespace": "ns"},
            "spec": {"targetRef": {"apiVersion": "apps/v1", "kind": "Deployment", "name": "web"},
                     "updatePolicy": {"updateMode": "Off"}},
            "status": {"recommendation": {"containerRecommendations": [
                {"containerName": "nginx", "target": {"cpu": "80m", "memory": "96Mi"}}]}}});
        assert_eq!(vpas.cell(&object, "target"), text("Deployment/web"));
        assert_eq!(vpas.cell(&object, "mode"), muted("Off"));
        assert_eq!(
            vpas.cell(&object, "recommendation"),
            text("nginx 80m / 96Mi")
        );
        let link = vpas.link(&object, "target").unwrap();
        assert_eq!(
            (
                link.group.as_str(),
                link.version.as_deref(),
                link.resource.as_str()
            ),
            ("apps", Some("v1"), "deployments")
        );

        let leases = kind("coordination.k8s.io", "Lease");
        let now = Timestamp::now();
        let renewed = now
            .checked_sub(jiff::SignedDuration::from_secs(90))
            .unwrap();
        let lease = json!({"spec": {"holderIdentity": "node-1", "leaseDurationSeconds": 40,
            "renewTime": renewed.to_string()}});
        assert_eq!(leases.cell(&lease, "holder"), text("node-1"));
        // Past its 40 s duration (the label's seconds depend on how long the test takes).
        match leases.cell(&lease, "renewed") {
            CellValue::Tinted { label, tone } => {
                assert_eq!(tone, Tone::Warning);
                assert!(label.ends_with("s ago"), "{label}");
            }
            other => panic!("{other:?}"),
        }
        let fresh = json!({"spec": {"holderIdentity": "node-1", "leaseDurationSeconds": 40,
            "renewTime": now.to_string()}});
        assert!(matches!(
            leases.cell(&fresh, "renewed"),
            CellValue::Tinted {
                tone: Tone::Neutral,
                ..
            }
        ));

        let classes = kind("scheduling.k8s.io", "PriorityClass");
        let class = json!({"value": 2000000000, "globalDefault": true});
        assert_eq!(classes.cell(&class, "value"), text("2000000000"));
        assert_eq!(
            classes.cell(&class, "global_default"),
            status("True", Tone::Info)
        );
        assert_eq!(
            classes.cell(&class, "preemption"),
            text("PreemptLowerPriority")
        );

        let runtimes = kind("node.k8s.io", "RuntimeClass");
        let runtime = json!({"handler": "runsc", "overhead": {"podFixed": {"cpu": "250m", "memory": "120Mi"}}});
        assert_eq!(runtimes.cell(&runtime, "handler"), text("runsc"));
        assert_eq!(
            runtimes.cell(&runtime, "overhead"),
            text("cpu=250m, memory=120Mi")
        );
        assert_eq!(runtimes.cell(&runtime, "node_selector"), muted("<none>"));
    }
}
