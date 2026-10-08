//! Dynamic Resource Allocation (`resource.k8s.io`): ResourceClaims, ResourceClaimTemplates,
//! DeviceClasses and ResourceSlices.
//!
//! Every reader takes the object as the API serves it in any of `v1` (Kubernetes 1.34+),
//! `v1beta2` and `v1beta1`: a request's fields sit under `exactly` since `v1beta2` and directly
//! on the request before, a device's attributes and capacity under `basic` in `v1beta1`, and a
//! capacity is `{value: <quantity>}` (or a bare quantity in older alphas).

use std::collections::HashMap;

use serde_json::Value;

use crate::format::{array_at, int_at, str_at};
use kubyl_base::types::Tone;

/// The API group.
pub const GROUP: &str = "resource.k8s.io";

/// One device request of a claim (or one alternative of `firstAvailable`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub name: String,
    pub device_class: String,
    /// `ExactCount` (the default) or `All`.
    pub mode: String,
    pub count: i64,
    /// CEL expressions of the request's selectors.
    pub selectors: Vec<String>,
    pub admin_access: bool,
    /// `tolerations` (`key=value:Effect`).
    pub tolerations: Vec<String>,
    /// The alternatives of a `firstAvailable` request, in order of preference.
    pub alternatives: Vec<Request>,
}

impl Request {
    fn parse(request: &Value) -> Self {
        // `v1beta2` and `v1`: the fields are under `exactly`; `v1beta1`: on the request.
        let body = request.get("exactly").unwrap_or(request);
        let mode = match str_at(body, "/allocationMode") {
            "" => "ExactCount",
            mode => mode,
        };
        let count = body.get("count").and_then(Value::as_i64).unwrap_or(1);
        Self {
            name: str_at(request, "/name").to_string(),
            device_class: str_at(body, "/deviceClassName").to_string(),
            mode: mode.to_string(),
            count,
            selectors: cel_selectors(array_at(body, "/selectors")),
            admin_access: body.get("adminAccess").and_then(Value::as_bool) == Some(true),
            tolerations: array_at(body, "/tolerations")
                .iter()
                .map(toleration)
                .collect(),
            alternatives: array_at(request, "/firstAvailable")
                .iter()
                .map(Request::parse)
                .collect(),
        }
    }

    /// `exactly 2`, `all`, `first of 2`.
    pub fn amount(&self) -> String {
        if !self.alternatives.is_empty() {
            return format!("first of {}", self.alternatives.len());
        }
        match self.mode.as_str() {
            "All" => "all".into(),
            _ => format!("exactly {}", self.count),
        }
    }

    /// The device classes it may use (its own, or its alternatives').
    pub fn classes(&self) -> Vec<&str> {
        if self.device_class.is_empty() {
            self.alternatives
                .iter()
                .map(|a| a.device_class.as_str())
                .filter(|c| !c.is_empty())
                .collect()
        } else {
            vec![self.device_class.as_str()]
        }
    }
}

fn toleration(t: &Value) -> String {
    let key = str_at(t, "/key");
    let value = str_at(t, "/value");
    let effect = str_at(t, "/effect");
    let mut out = match (key, value) {
        ("", _) => "*".to_string(),
        (key, "") => key.to_string(),
        (key, value) => format!("{key}={value}"),
    };
    if !effect.is_empty() {
        out.push(':');
        out.push_str(effect);
    }
    out
}

/// The CEL expressions of `selectors: [{cel: {expression}}]`.
pub fn cel_selectors(selectors: &[Value]) -> Vec<String> {
    selectors
        .iter()
        .filter_map(|s| s.pointer("/cel/expression").and_then(Value::as_str))
        .map(|e| e.trim().to_string())
        .collect()
}

/// The requests of a claim spec's `devices` (a ResourceClaim's `spec.devices`, a template's
/// `spec.spec.devices`).
pub fn requests(devices: &Value) -> Vec<Request> {
    array_at(devices, "/requests")
        .iter()
        .map(Request::parse)
        .collect()
}

/// The distinct device classes of some requests.
pub fn device_classes(requests: &[Request]) -> Vec<String> {
    let mut classes: Vec<String> = Vec::new();
    for class in requests.iter().flat_map(Request::classes) {
        if !classes.iter().any(|c| c == class) {
            classes.push(class.to_string());
        }
    }
    classes
}

/// One allocated device.
#[derive(Clone, Debug, PartialEq)]
pub struct AllocatedDevice {
    pub request: String,
    pub driver: String,
    pub pool: String,
    pub device: String,
    pub admin_access: bool,
}

/// What the scheduler allocated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Allocation {
    pub devices: Vec<AllocatedDevice>,
    /// The node, when the node selector names exactly one (the usual case for node-local
    /// devices).
    pub node: Option<String>,
    /// The node selector otherwise (`zone in (a,b)`), empty for devices reachable anywhere.
    pub node_selector: Vec<String>,
    pub time: Option<String>,
}

/// Who a claim is reserved for.
#[derive(Clone, Debug, PartialEq)]
pub struct Consumer {
    pub api_group: String,
    pub resource: String,
    pub name: String,
}

/// A condition (`status.devices[].conditions`, Gateway and VPA status).
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub kind: String,
    pub status: String,
    pub reason: String,
    pub message: String,
}

impl Condition {
    pub fn parse(c: &Value) -> Self {
        Self {
            kind: str_at(c, "/type").to_string(),
            status: str_at(c, "/status").to_string(),
            reason: str_at(c, "/reason").to_string(),
            message: str_at(c, "/message").to_string(),
        }
    }

    pub fn is_true(&self) -> bool {
        self.status == "True"
    }

    pub fn is_false(&self) -> bool {
        self.status == "False"
    }
}

/// The conditions at a JSON pointer.
pub fn conditions(object: &Value, pointer: &str) -> Vec<Condition> {
    array_at(object, pointer)
        .iter()
        .map(Condition::parse)
        .collect()
}

/// A driver's report about an allocated device (`status.devices[]`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceStatus {
    pub driver: String,
    pub pool: String,
    pub device: String,
    pub conditions: Vec<Condition>,
    /// `data` as `key=value` (attributes in the device attribute format, or plain JSON).
    pub data: Vec<String>,
    /// Network data: interface name, IPs, hardware address.
    pub network: Vec<String>,
}

/// A ResourceClaim.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Claim {
    pub requests: Vec<Request>,
    pub allocation: Option<Allocation>,
    pub reserved_for: Vec<Consumer>,
    pub devices: Vec<DeviceStatus>,
    pub deleting: bool,
}

impl Claim {
    pub fn parse(claim: &Value) -> Self {
        let allocation = claim.pointer("/status/allocation").map(|a| {
            let (node, node_selector) = node_selector(a.get("nodeSelector"));
            Allocation {
                devices: array_at(a, "/devices/results")
                    .iter()
                    .map(|r| AllocatedDevice {
                        request: str_at(r, "/request").to_string(),
                        driver: str_at(r, "/driver").to_string(),
                        pool: str_at(r, "/pool").to_string(),
                        device: str_at(r, "/device").to_string(),
                        admin_access: r.get("adminAccess").and_then(Value::as_bool) == Some(true),
                    })
                    .collect(),
                node,
                node_selector,
                time: a
                    .get("allocationTimestamp")
                    .and_then(Value::as_str)
                    .map(String::from),
            }
        });
        Self {
            requests: requests(claim.pointer("/spec/devices").unwrap_or(&Value::Null)),
            allocation,
            reserved_for: array_at(claim, "/status/reservedFor")
                .iter()
                .map(|r| Consumer {
                    api_group: str_at(r, "/apiGroup").to_string(),
                    resource: str_at(r, "/resource").to_string(),
                    name: str_at(r, "/name").to_string(),
                })
                .collect(),
            devices: array_at(claim, "/status/devices")
                .iter()
                .map(|d| DeviceStatus {
                    driver: str_at(d, "/driver").to_string(),
                    pool: str_at(d, "/pool").to_string(),
                    device: str_at(d, "/device").to_string(),
                    conditions: conditions(d, "/conditions"),
                    data: data_pairs(d.get("data")),
                    network: network_data(d.get("networkData")),
                })
                .collect(),
            deleting: claim.pointer("/metadata/deletionTimestamp").is_some(),
        }
    }

    /// `Pending`, `Allocated`, `Allocated, reserved by 2`, `Deleting` (like kubectl's
    /// `pending`, `allocated`, `allocated,reserved`, `deleted`).
    pub fn state(&self) -> String {
        if self.deleting {
            return "Deleting".into();
        }
        match (&self.allocation, self.reserved_for.len()) {
            (None, _) => "Pending".into(),
            (Some(_), 0) => "Allocated".into(),
            (Some(_), n) => format!("Allocated, reserved by {n}"),
        }
    }

    pub fn device_classes(&self) -> Vec<String> {
        device_classes(&self.requests)
    }

    /// The allocated devices.
    pub fn allocated(&self) -> &[AllocatedDevice] {
        self.allocation
            .as_ref()
            .map(|a| a.devices.as_slice())
            .unwrap_or_default()
    }

    /// The driver's report on `device`, if any.
    pub fn device_status(&self, device: &AllocatedDevice) -> Option<&DeviceStatus> {
        self.devices.iter().find(|d| {
            d.driver == device.driver && d.pool == device.pool && d.device == device.device
        })
    }
}

/// The tone of a claim state.
pub fn claim_tone(state: &str) -> Tone {
    crate::status::status_tone(state)
}

/// `(node, selector)`: the single node a selector names (`metadata.name In [n]`), else the
/// selector's terms as text.
fn node_selector(selector: Option<&Value>) -> (Option<String>, Vec<String>) {
    let Some(selector) = selector else {
        return (None, Vec::new());
    };
    let terms = array_at(selector, "/nodeSelectorTerms");
    if let [term] = terms
        && array_at(term, "/matchExpressions").is_empty()
        && let [field] = array_at(term, "/matchFields")
        && str_at(field, "/key") == "metadata.name"
        && str_at(field, "/operator") == "In"
        && let [Value::String(node)] = array_at(field, "/values")
    {
        return (Some(node.clone()), Vec::new());
    }
    let text = terms
        .iter()
        .map(|term| {
            array_at(term, "/matchExpressions")
                .iter()
                .chain(array_at(term, "/matchFields"))
                .map(|e| {
                    let values: Vec<&str> = array_at(e, "/values")
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    format!(
                        "{} {} ({})",
                        str_at(e, "/key"),
                        str_at(e, "/operator").to_lowercase(),
                        values.join(",")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|t| !t.is_empty())
        .collect();
    (None, text)
}

/// `key=value` of a map of device attributes (`{string: …}`, `{int: …}`…) or plain values.
fn data_pairs(data: Option<&Value>) -> Vec<String> {
    let Some(map) = data.and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut pairs: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("{k}={}", attribute_value(v)))
        .collect();
    pairs.sort();
    pairs
}

fn network_data(data: Option<&Value>) -> Vec<String> {
    let Some(data) = data else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let name = str_at(data, "/interfaceName");
    if !name.is_empty() {
        out.push(format!("interface {name}"));
    }
    for ip in array_at(data, "/ips").iter().filter_map(Value::as_str) {
        out.push(ip.to_string());
    }
    let mac = str_at(data, "/hardwareAddress");
    if !mac.is_empty() {
        out.push(mac.to_string());
    }
    out
}

/// A device attribute's value: `{int: 4}` → `4`, `{version: "1.0.0"}` → `1.0.0`.
pub fn attribute_value(value: &Value) -> String {
    let inner = ["string", "int", "bool", "version"]
        .iter()
        .find_map(|k| value.get(*k))
        .unwrap_or(value);
    match inner {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A capacity: `{value: "80Gi"}` (`v1beta1` and later) or `"80Gi"` (older alphas).
pub fn capacity_value(value: &Value) -> String {
    match value.get("value").unwrap_or(value) {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A device taint (`key=value:Effect`).
#[derive(Clone, Debug, PartialEq)]
pub struct Taint {
    pub key: String,
    pub value: String,
    pub effect: String,
}

impl Taint {
    pub fn label(&self) -> String {
        match self.value.as_str() {
            "" => format!("{}:{}", self.key, self.effect),
            value => format!("{}={value}:{}", self.key, self.effect),
        }
    }
}

/// One device of a ResourceSlice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub name: String,
    /// `name=value`, sorted.
    pub attributes: Vec<(String, String)>,
    /// `name`, quantity, sorted.
    pub capacity: Vec<(String, String)>,
    pub taints: Vec<Taint>,
    /// A per-device node (with `perDeviceNodeSelection`).
    pub node: Option<String>,
}

/// A ResourceSlice.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Slice {
    pub driver: String,
    pub pool: String,
    pub generation: i64,
    pub slice_count: i64,
    pub node: Option<String>,
    pub all_nodes: bool,
    pub node_selector: Vec<String>,
    pub per_device_nodes: bool,
    pub devices: Vec<Device>,
}

impl Slice {
    pub fn parse(slice: &Value) -> Self {
        let spec = slice.get("spec").unwrap_or(&Value::Null);
        let node = Some(str_at(spec, "/nodeName"))
            .filter(|n| !n.is_empty())
            .map(String::from);
        Self {
            driver: str_at(spec, "/driver").to_string(),
            pool: str_at(spec, "/pool/name").to_string(),
            generation: int_at(spec, "/pool/generation"),
            slice_count: int_at(spec, "/pool/resourceSliceCount"),
            node,
            all_nodes: spec.get("allNodes").and_then(Value::as_bool) == Some(true),
            node_selector: node_selector(spec.get("nodeSelector")).1,
            per_device_nodes: spec.get("perDeviceNodeSelection").and_then(Value::as_bool)
                == Some(true),
            devices: array_at(spec, "/devices").iter().map(device).collect(),
        }
    }

    /// Where the devices are: a node, `all nodes`, a selector or `per device`.
    pub fn node_label(&self) -> String {
        if let Some(node) = &self.node {
            node.clone()
        } else if self.all_nodes {
            "all nodes".into()
        } else if self.per_device_nodes {
            "per device".into()
        } else if !self.node_selector.is_empty() {
            self.node_selector.join(" or ")
        } else {
            String::new()
        }
    }

    pub fn has_device(&self, name: &str) -> bool {
        self.devices.iter().any(|d| d.name == name)
    }
}

fn device(device: &Value) -> Device {
    // `v1beta1`: attributes, capacity and taints under `basic`.
    let body = device.get("basic").unwrap_or(device);
    let mut attributes: Vec<(String, String)> = body
        .get("attributes")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), attribute_value(v)))
                .collect()
        })
        .unwrap_or_default();
    attributes.sort();
    let mut capacity: Vec<(String, String)> = body
        .get("capacity")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), capacity_value(v)))
                .collect()
        })
        .unwrap_or_default();
    capacity.sort();
    Device {
        name: str_at(device, "/name").to_string(),
        attributes,
        capacity,
        taints: array_at(body, "/taints")
            .iter()
            .map(|t| Taint {
                key: str_at(t, "/key").to_string(),
                value: str_at(t, "/value").to_string(),
                effect: str_at(t, "/effect").to_string(),
            })
            .collect(),
        node: Some(str_at(body, "/nodeName"))
            .filter(|n| !n.is_empty())
            .map(String::from),
    }
}

/// The claims (`namespace/name`) holding each allocated device of the cluster, built once from
/// its ResourceClaims and looked up per slice or device. A device can be held by several claims
/// (admin access, or devices shared through consumable capacity).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceHolders {
    /// `(driver, pool)` → device → claims.
    by_pool: HashMap<(String, String), HashMap<String, Vec<String>>>,
}

impl DeviceHolders {
    pub fn index<'a>(claims: impl IntoIterator<Item = &'a Value>) -> Self {
        let mut by_pool: HashMap<(String, String), HashMap<String, Vec<String>>> = HashMap::new();
        for claim in claims {
            let parsed = Claim::parse(claim);
            if parsed.allocated().is_empty() {
                continue;
            }
            let name = match crate::format::namespace(claim) {
                Some(ns) => format!("{ns}/{}", crate::format::name(claim)),
                None => crate::format::name(claim).to_string(),
            };
            for device in parsed.allocated() {
                let holders = by_pool
                    .entry((device.driver.clone(), device.pool.clone()))
                    .or_default()
                    .entry(device.device.clone())
                    .or_default();
                if !holders.contains(&name) {
                    holders.push(name.clone());
                }
            }
        }
        for devices in by_pool.values_mut() {
            for holders in devices.values_mut() {
                holders.sort();
            }
        }
        Self { by_pool }
    }

    /// The claims holding a device (sorted; empty when it's free).
    pub fn holders(&self, driver: &str, pool: &str, device: &str) -> &[String] {
        self.by_pool
            .get(&(driver.to_string(), pool.to_string()))
            .and_then(|devices| devices.get(device))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// The allocated devices of `slice` with the claims holding each, in the slice's device order:
/// one entry per device, however many claims share it.
pub fn allocations(slice: &Slice, holders: &DeviceHolders) -> Vec<(String, Vec<String>)> {
    let Some(devices) = holders
        .by_pool
        .get(&(slice.driver.clone(), slice.pool.clone()))
    else {
        return Vec::new();
    };
    slice
        .devices
        .iter()
        .filter_map(|device| {
            devices
                .get(&device.name)
                .map(|claims| (device.name.clone(), claims.clone()))
        })
        .collect()
}

/// A DeviceClass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Class {
    pub selectors: Vec<String>,
    /// `(driver, parameters as JSON)` of each opaque config.
    pub config: Vec<(String, String)>,
    pub extended_resource_name: Option<String>,
}

impl Class {
    pub fn parse(class: &Value) -> Self {
        let spec = class.get("spec").unwrap_or(&Value::Null);
        Self {
            selectors: cel_selectors(array_at(spec, "/selectors")),
            config: array_at(spec, "/config")
                .iter()
                .filter_map(|c| c.get("opaque"))
                .map(|o| {
                    (
                        str_at(o, "/driver").to_string(),
                        o.get("parameters")
                            .map(|p| serde_json::to_string_pretty(p).unwrap_or_default())
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            extended_resource_name: Some(str_at(spec, "/extendedResourceName"))
                .filter(|n| !n.is_empty())
                .map(String::from),
        }
    }

    /// The drivers its selectors pin (`device.driver == "gpu.example.com"`).
    pub fn drivers(&self) -> Vec<String> {
        let mut drivers = Vec::new();
        for expression in &self.selectors {
            for driver in drivers_in(expression) {
                if !drivers.contains(&driver) {
                    drivers.push(driver);
                }
            }
        }
        drivers
    }
}

/// The string literals compared with `device.driver` in a CEL expression, on either side of
/// `==` (`device.driver == "a"`, `'a' == device.driver`).
pub fn drivers_in(expression: &str) -> Vec<String> {
    const FIELD: &str = "device.driver";
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = expression[from..].find(FIELD).map(|at| from + at) {
        from = at + FIELD.len();
        // `device.driver == "a"`
        let after = expression[from..].trim_start();
        if let Some(tail) = after.strip_prefix("==")
            && let Some(literal) = leading_literal(tail.trim_start())
        {
            out.push(literal.to_string());
            continue;
        }
        // `"a" == device.driver`
        let before = expression[..at].trim_end();
        if let Some(head) = before.strip_suffix("==")
            && let Some(literal) = trailing_literal(head.trim_end())
        {
            out.push(literal.to_string());
        }
    }
    out
}

/// The quoted string at the start of `text`.
fn leading_literal(text: &str) -> Option<&str> {
    let quote = text.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = text[1..].find(quote)?;
    Some(&text[1..1 + end])
}

/// The quoted string at the end of `text`.
fn trailing_literal(text: &str) -> Option<&str> {
    let quote = text.chars().last().filter(|c| *c == '"' || *c == '\'')?;
    let body = &text[..text.len() - 1];
    let start = body.rfind(quote)?;
    Some(&body[start + 1..])
}

/// One claim a pod uses (`spec.resourceClaims[]`).
#[derive(Clone, Debug, PartialEq)]
pub struct PodClaim {
    /// The name in the pod.
    pub name: String,
    /// The ResourceClaim (given, or generated from the template).
    pub claim: Option<String>,
    pub template: Option<String>,
    /// The containers that use it.
    pub containers: Vec<String>,
    /// What the kubelet reports about the devices: `(health, message)`.
    pub health: Vec<(String, String)>,
}

/// The claims of a pod with the kubelet's device health (`allocatedResourcesStatus`, entries
/// named `claim:<name>` or `claim:<name>/<request>`).
pub fn pod_claims(pod: &Value) -> Vec<PodClaim> {
    let statuses = array_at(pod, "/status/resourceClaimStatuses");
    let containers: Vec<&Value> = array_at(pod, "/spec/initContainers")
        .iter()
        .chain(array_at(pod, "/spec/containers"))
        .collect();
    let container_statuses: Vec<&Value> = array_at(pod, "/status/initContainerStatuses")
        .iter()
        .chain(array_at(pod, "/status/containerStatuses"))
        .collect();
    array_at(pod, "/spec/resourceClaims")
        .iter()
        .map(|c| {
            let name = str_at(c, "/name").to_string();
            let claim = match str_at(c, "/resourceClaimName") {
                "" => statuses
                    .iter()
                    .find(|s| str_at(s, "/name") == name)
                    .map(|s| str_at(s, "/resourceClaimName"))
                    .filter(|n| !n.is_empty())
                    .map(String::from),
                given => Some(given.to_string()),
            };
            let template = Some(str_at(c, "/resourceClaimTemplateName"))
                .filter(|t| !t.is_empty())
                .map(String::from);
            let users = containers
                .iter()
                .filter(|ctr| {
                    array_at(ctr, "/resources/claims")
                        .iter()
                        .any(|rc| str_at(rc, "/name") == name)
                })
                .map(|ctr| str_at(ctr, "/name").to_string())
                .collect();
            let mut health = Vec::new();
            for status in &container_statuses {
                for entry in array_at(status, "/allocatedResourcesStatus") {
                    let entry_name = str_at(entry, "/name");
                    let Some(rest) = entry_name.strip_prefix("claim:") else {
                        continue;
                    };
                    if rest.split('/').next() != Some(name.as_str()) {
                        continue;
                    }
                    for resource in array_at(entry, "/resources") {
                        let value = match str_at(resource, "/health") {
                            "" => "Unknown",
                            h => h,
                        };
                        health.push((value.to_string(), str_at(resource, "/message").to_string()));
                    }
                }
            }
            PodClaim {
                name,
                claim,
                template,
                containers: users,
                health,
            }
        })
        .collect()
}

/// How healthy a claim's devices are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    pub healthy: usize,
    pub unhealthy: usize,
    pub unknown: usize,
}

impl Health {
    /// `2 healthy`, `1 unhealthy`, `unknown`; `None` when nothing reported.
    pub fn label(&self) -> Option<(String, Tone)> {
        if self.unhealthy > 0 {
            Some((format!("{} unhealthy", self.unhealthy), Tone::Bad))
        } else if self.healthy > 0 {
            Some((format!("{} healthy", self.healthy), Tone::Good))
        } else if self.unknown > 0 {
            Some(("unknown".into(), Tone::Muted))
        } else {
            None
        }
    }
}

/// The claims of the pods that use any, by `(namespace, pod)`: built once from the pods and
/// looked up per claim by [`claim_health`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PodClaims {
    by_pod: HashMap<(String, String), Vec<PodClaim>>,
}

impl PodClaims {
    pub fn index<'a>(pods: impl IntoIterator<Item = &'a Value>) -> Self {
        let by_pod = pods
            .into_iter()
            .filter(|pod| !array_at(pod, "/spec/resourceClaims").is_empty())
            .map(|pod| {
                (
                    (
                        crate::format::namespace(pod)
                            .unwrap_or_default()
                            .to_string(),
                        crate::format::name(pod).to_string(),
                    ),
                    pod_claims(pod),
                )
            })
            .collect();
        Self { by_pod }
    }

    /// The claims of the pod `namespace/name` (empty when it uses none or isn't known).
    pub fn of(&self, namespace: &str, pod: &str) -> &[PodClaim] {
        self.by_pod
            .get(&(namespace.to_string(), pod.to_string()))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}

/// A claim's device health: the driver's `Ready` conditions in the claim's status, else what
/// the kubelet reports on the pods of the claim's namespace it's reserved for.
pub fn claim_health(claim: &Value, pods: &PodClaims) -> Health {
    let parsed = Claim::parse(claim);
    let mut health = Health::default();
    let allocated = parsed.allocated();
    if allocated.is_empty() {
        return health;
    }
    let mut reported = false;
    for device in allocated {
        let Some(status) = parsed.device_status(device) else {
            continue;
        };
        if let Some(ready) = status.conditions.iter().find(|c| c.kind == "Ready") {
            reported = true;
            if ready.is_true() {
                health.healthy += 1;
            } else if ready.is_false() {
                health.unhealthy += 1;
            } else {
                health.unknown += 1;
            }
        }
    }
    if reported {
        return health;
    }
    let name = crate::format::name(claim);
    // `reservedFor` names consumers in the claim's namespace.
    let namespace = crate::format::namespace(claim).unwrap_or_default();
    let mut kubelet = Health::default();
    for consumer in parsed
        .reserved_for
        .iter()
        .filter(|c| c.api_group.is_empty() && c.resource == "pods")
    {
        for claim in pods.of(namespace, &consumer.name) {
            if claim.claim.as_deref() != Some(name) {
                continue;
            }
            for (value, _) in &claim.health {
                match value.as_str() {
                    "Healthy" => kubelet.healthy += 1,
                    "Unhealthy" => kubelet.unhealthy += 1,
                    _ => kubelet.unknown += 1,
                }
            }
        }
    }
    if kubelet.unhealthy > 0 {
        return kubelet;
    }
    if kubelet.healthy > 0 {
        // The kubelet reports per container entry, not per device: count the devices.
        return Health {
            healthy: allocated.len(),
            ..Health::default()
        };
    }
    kubelet
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v1_claim() -> Value {
        json!({"apiVersion": "resource.k8s.io/v1", "kind": "ResourceClaim",
            "metadata": {"name": "shared-gpu", "namespace": "kubyl-views"},
            "spec": {"devices": {"requests": [{"name": "gpus", "exactly": {
                "deviceClassName": "gpu.example.com", "allocationMode": "ExactCount", "count": 2}}]}},
            "status": {"allocation": {"allocationTimestamp": "2026-10-07T10:36:24Z",
                "devices": {"results": [
                    {"device": "gpu-0", "driver": "gpu.example.com", "pool": "worker", "request": "gpus"},
                    {"device": "gpu-1", "driver": "gpu.example.com", "pool": "worker", "request": "gpus"}]},
                "nodeSelector": {"nodeSelectorTerms": [{"matchFields": [
                    {"key": "metadata.name", "operator": "In", "values": ["worker"]}]}]}},
                "reservedFor": [{"name": "gpu-shared", "resource": "pods", "uid": "u"}]}})
    }

    #[test]
    fn claims_read_the_same_in_every_version() {
        let claim = Claim::parse(&v1_claim());
        assert_eq!(claim.state(), "Allocated, reserved by 1");
        assert_eq!(claim_tone(&claim.state()), Tone::Good);
        assert_eq!(claim.device_classes(), ["gpu.example.com"]);
        assert_eq!(claim.requests[0].amount(), "exactly 2");
        let allocation = claim.allocation.as_ref().unwrap();
        assert_eq!(allocation.node.as_deref(), Some("worker"));
        assert_eq!(allocation.devices.len(), 2);
        assert_eq!(claim.reserved_for[0].name, "gpu-shared");

        // v1beta1: the request's fields on the request itself, no allocation yet.
        let beta1 = json!({"apiVersion": "resource.k8s.io/v1beta1", "metadata": {"name": "c"},
            "spec": {"devices": {"requests": [{"name": "nic", "deviceClassName": "net.example.com",
                "allocationMode": "All", "selectors": [{"cel": {"expression": "device.attributes['net.example.com'].speed >= 100"}}],
                "adminAccess": true}]}}});
        let claim = Claim::parse(&beta1);
        assert_eq!(claim.state(), "Pending");
        assert_eq!(claim_tone("Pending"), Tone::Warning);
        let request = &claim.requests[0];
        assert_eq!(request.device_class, "net.example.com");
        assert_eq!(request.amount(), "all");
        assert!(request.admin_access);
        assert_eq!(
            request.selectors,
            ["device.attributes['net.example.com'].speed >= 100"]
        );

        // v1beta2: firstAvailable alternatives, allocated but not reserved.
        let beta2 = json!({"metadata": {"name": "c"},
            "spec": {"devices": {"requests": [{"name": "gpu", "firstAvailable": [
                {"name": "big", "deviceClassName": "a100.example.com"},
                {"name": "small", "deviceClassName": "t4.example.com", "count": 2}]}]}},
            "status": {"allocation": {"devices": {"results": [
                {"request": "gpu/small", "driver": "d", "pool": "p", "device": "x"}]}}}});
        let claim = Claim::parse(&beta2);
        assert_eq!(claim.state(), "Allocated");
        assert_eq!(claim.requests[0].amount(), "first of 2");
        assert_eq!(
            claim.device_classes(),
            ["a100.example.com", "t4.example.com"]
        );
        assert_eq!(claim.requests[0].alternatives[1].count, 2);
        let mut deleting = v1_claim();
        deleting["metadata"]["deletionTimestamp"] = json!("2026-10-07T10:40:00Z");
        assert_eq!(Claim::parse(&deleting).state(), "Deleting");
    }

    #[test]
    fn slices_in_v1_and_v1beta1() {
        let v1 = json!({"spec": {"driver": "gpu.example.com", "nodeName": "worker",
            "pool": {"name": "worker", "generation": 1, "resourceSliceCount": 1},
            "devices": [{"name": "gpu-0",
                "attributes": {"index": {"int": 0}, "model": {"string": "LATEST-GPU-MODEL"}, "driverVersion": {"version": "1.0.0"}},
                "capacity": {"memory": {"value": "80Gi"}},
                "taints": [{"key": "maintenance", "value": "planned", "effect": "NoSchedule"}]},
                {"name": "gpu-1"}]}});
        let slice = Slice::parse(&v1);
        assert_eq!(slice.node_label(), "worker");
        assert_eq!(slice.devices.len(), 2);
        let gpu = &slice.devices[0];
        assert_eq!(
            gpu.attributes,
            [
                ("driverVersion".to_string(), "1.0.0".to_string()),
                ("index".into(), "0".into()),
                ("model".into(), "LATEST-GPU-MODEL".into())
            ]
        );
        assert_eq!(gpu.capacity, [("memory".to_string(), "80Gi".to_string())]);
        assert_eq!(gpu.taints[0].label(), "maintenance=planned:NoSchedule");

        let beta1 = json!({"spec": {"driver": "net.example.com", "allNodes": true,
            "pool": {"name": "fabric"},
            "devices": [{"name": "nic-0", "basic": {"attributes": {"speed": {"int": 100}},
                "capacity": {"bandwidth": {"value": "10G"}}}}]}});
        let slice = Slice::parse(&beta1);
        assert_eq!(slice.node_label(), "all nodes");
        assert_eq!(slice.devices[0].attributes[0].1, "100");
        assert_eq!(slice.devices[0].capacity[0].1, "10G");

        // Which claim holds which device.
        let slice = Slice::parse(&json!({"spec": {"driver": "gpu.example.com",
            "pool": {"name": "worker"}, "devices": [{"name": "gpu-1"}, {"name": "gpu-0"}, {"name": "gpu-2"}]}}));
        let other = json!({"metadata": {"name": "other", "namespace": "ns"},
            "status": {"allocation": {"devices": {"results": [
                {"driver": "gpu.example.com", "pool": "elsewhere", "device": "gpu-2"}]}}}});
        // A second claim with admin access to gpu-0: still one allocated device, two holders.
        let admin = json!({"metadata": {"name": "monitor", "namespace": "ops"},
            "status": {"allocation": {"devices": {"results": [
                {"driver": "gpu.example.com", "pool": "worker", "device": "gpu-0", "adminAccess": true}]}}}});
        let claims = [v1_claim(), other, admin];
        let holders = DeviceHolders::index(claims.iter());
        assert_eq!(
            allocations(&slice, &holders),
            [
                (
                    "gpu-1".to_string(),
                    vec!["kubyl-views/shared-gpu".to_string()]
                ),
                (
                    "gpu-0".into(),
                    vec!["kubyl-views/shared-gpu".into(), "ops/monitor".into()]
                )
            ]
        );
        assert_eq!(
            holders.holders("gpu.example.com", "elsewhere", "gpu-2"),
            ["ns/other"]
        );
        assert!(
            holders
                .holders("gpu.example.com", "worker", "gpu-2")
                .is_empty()
        );
    }

    #[test]
    fn device_class_drivers_come_from_selectors() {
        let class = Class::parse(&json!({"spec": {
            "selectors": [{"cel": {"expression": "device.driver == 'gpu.example.com'"}},
                          {"cel": {"expression": "device.driver == \"other.example.com\" && device.attributes['x'].y"}}],
            "config": [{"opaque": {"driver": "gpu.example.com", "parameters": {"sharing": "TimeSlicing"}}}],
            "extendedResourceName": "example.com/gpu"}}));
        assert_eq!(class.drivers(), ["gpu.example.com", "other.example.com"]);
        assert_eq!(class.config[0].0, "gpu.example.com");
        assert!(class.config[0].1.contains("TimeSlicing"));
        assert_eq!(
            class.extended_resource_name.as_deref(),
            Some("example.com/gpu")
        );
        assert!(drivers_in("device.driver.startsWith('x')").is_empty());
        assert_eq!(
            drivers_in("'gpu.example.com' == device.driver || device.driver == \"b\""),
            ["gpu.example.com", "b"]
        );
        assert_eq!(drivers_in("\"a\"==device.driver"), ["a"]);
    }

    #[test]
    fn pods_name_their_claims_and_health() {
        let pod = json!({"metadata": {"name": "gpu-shared"},
            "spec": {"resourceClaims": [{"name": "gpus", "resourceClaimName": "shared-gpu"},
                                        {"name": "gpu", "resourceClaimTemplateName": "single-gpu"}],
                "containers": [{"name": "ctr", "resources": {"claims": [{"name": "gpus"}, {"name": "gpu"}]}}]},
            "status": {"resourceClaimStatuses": [{"name": "gpu", "resourceClaimName": "gpu-shared-gpu-x7"}],
                "containerStatuses": [{"name": "ctr", "allocatedResourcesStatus": [
                    {"name": "claim:gpus", "resources": [{"health": "Healthy", "message": "ok", "resourceID": "r"}]},
                    {"name": "claim:gpu/gpu", "resources": [{"health": "Unhealthy", "resourceID": "r2"}]}]}]}});
        let claims = pod_claims(&pod);
        assert_eq!(claims[0].claim.as_deref(), Some("shared-gpu"));
        assert_eq!(claims[0].containers, ["ctr"]);
        assert_eq!(
            claims[0].health,
            [("Healthy".to_string(), "ok".to_string())]
        );
        assert_eq!(claims[1].claim.as_deref(), Some("gpu-shared-gpu-x7"));
        assert_eq!(claims[1].template.as_deref(), Some("single-gpu"));
        assert_eq!(claims[1].health[0].0, "Unhealthy");

        let mut pod = pod;
        pod["metadata"]["namespace"] = json!("kubyl-views");
        let health = claim_health(&v1_claim(), &PodClaims::index([&pod]));
        assert_eq!(health.label(), Some(("2 healthy".into(), Tone::Good)));
        // A pod of the same name in another namespace doesn't count.
        let mut elsewhere = pod.clone();
        elsewhere["metadata"]["namespace"] = json!("other");
        assert!(
            claim_health(&v1_claim(), &PodClaims::index([&elsewhere]))
                .label()
                .is_none()
        );
        let pods = PodClaims::index([&pod]);
        // A driver's Ready condition wins.
        let mut claim = v1_claim();
        claim["status"]["devices"] = json!([{"driver": "gpu.example.com", "pool": "worker", "device": "gpu-0",
            "conditions": [{"type": "Ready", "status": "False", "reason": "Overheated"}],
            "data": {"model": {"string": "X"}}}]);
        let health = claim_health(&claim, &pods);
        assert_eq!(health.label(), Some(("1 unhealthy".into(), Tone::Bad)));
        let parsed = Claim::parse(&claim);
        assert_eq!(parsed.devices[0].data, ["model=X"]);
        assert!(claim_health(&json!({}), &pods).label().is_none());
    }
}
