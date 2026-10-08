//! The Gateway API (`gateway.networking.k8s.io`, standard and experimental channels):
//! GatewayClasses, Gateways, their listeners, and the routes (HTTP, GRPC, TCP, TLS, UDP) with
//! their parents and backends. ReferenceGrants decide which cross-namespace backends count.
//!
//! Fields are the same in `v1`, `v1beta1` and the `v1alpha2`/`v1alpha3` route versions.

use serde_json::Value;

use crate::dra::{Condition, conditions};
use crate::format::{array_at, int_at, str_at};
use kubyl_base::types::Tone;

/// The API group of the standard and experimental kinds.
pub const GROUP: &str = "gateway.networking.k8s.io";
/// The group of the `X`-prefixed experimental kinds (XListenerSet…).
pub const EXPERIMENTAL_GROUP: &str = "gateway.networking.x-k8s.io";

/// The route kinds and their plurals.
pub const ROUTE_KINDS: &[(&str, &str)] = &[
    ("HTTPRoute", "httproutes"),
    ("GRPCRoute", "grpcroutes"),
    ("TLSRoute", "tlsroutes"),
    ("TCPRoute", "tcproutes"),
    ("UDPRoute", "udproutes"),
];

/// Whether `(group, kind)` is a Gateway API route.
pub fn is_route(group: &str, kind: &str) -> bool {
    group == GROUP && ROUTE_KINDS.iter().any(|(k, _)| *k == kind)
}

/// The condition `kind` among `conditions`.
pub fn condition<'a>(conditions: &'a [Condition], kind: &str) -> Option<&'a Condition> {
    conditions.iter().find(|c| c.kind == kind)
}

/// One listener with what the controller reports about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listener {
    pub name: String,
    pub protocol: String,
    pub port: i64,
    pub hostname: String,
    /// `Same`, `All`, `Selector team=a`.
    pub allowed_namespaces: String,
    /// `Terminate · Secret shop-tls`.
    pub tls: Option<String>,
    pub attached_routes: Option<i64>,
    pub conditions: Vec<Condition>,
}

impl Listener {
    /// `Programmed`, `Not programmed: <reason>`, `Pending` and its tone.
    pub fn state(&self) -> (String, Tone) {
        programmed_state(&self.conditions)
    }
}

fn programmed_state(conditions: &[Condition]) -> (String, Tone) {
    if let Some(accepted) = condition(conditions, "Accepted")
        && accepted.is_false()
    {
        return (with_reason("Not accepted", &accepted.reason), Tone::Bad);
    }
    match condition(conditions, "Programmed") {
        Some(c) if c.is_true() => ("Programmed".into(), Tone::Good),
        Some(c) if c.is_false() => (with_reason("Not programmed", &c.reason), Tone::Bad),
        _ => ("Pending".into(), Tone::Warning),
    }
}

fn with_reason(label: &str, reason: &str) -> String {
    if reason.is_empty() || reason == "Pending" {
        label.to_string()
    } else {
        format!("{label}: {reason}")
    }
}

/// A Gateway.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gateway {
    pub class: String,
    /// The addresses the controller reports, else the requested ones.
    pub addresses: Vec<String>,
    pub listeners: Vec<Listener>,
    pub conditions: Vec<Condition>,
}

impl Gateway {
    pub fn parse(gateway: &Value) -> Self {
        let address = |a: &Value| str_at(a, "/value").to_string();
        let mut addresses: Vec<String> = array_at(gateway, "/status/addresses")
            .iter()
            .map(address)
            .collect();
        if addresses.is_empty() {
            addresses = array_at(gateway, "/spec/addresses")
                .iter()
                .map(address)
                .collect();
        }
        let statuses = array_at(gateway, "/status/listeners");
        let listeners = array_at(gateway, "/spec/listeners")
            .iter()
            .map(|l| {
                let name = str_at(l, "/name").to_string();
                let status = statuses.iter().find(|s| str_at(s, "/name") == name);
                let allowed = match str_at(l, "/allowedRoutes/namespaces/from") {
                    "" => "Same".to_string(),
                    "Selector" => format!(
                        "Selector {}",
                        crate::admission::selector_text(
                            l.pointer("/allowedRoutes/namespaces/selector")
                                .unwrap_or(&Value::Null)
                        )
                    ),
                    from => from.to_string(),
                };
                let tls = l.get("tls").map(|tls| {
                    let mode = match str_at(tls, "/mode") {
                        "" => "Terminate",
                        m => m,
                    };
                    let refs: Vec<String> = array_at(tls, "/certificateRefs")
                        .iter()
                        .map(|r| {
                            let kind = match str_at(r, "/kind") {
                                "" => "Secret",
                                k => k,
                            };
                            format!("{kind} {}", str_at(r, "/name"))
                        })
                        .collect();
                    if refs.is_empty() {
                        mode.to_string()
                    } else {
                        format!("{mode} · {}", refs.join(", "))
                    }
                });
                Listener {
                    protocol: str_at(l, "/protocol").to_string(),
                    port: int_at(l, "/port"),
                    hostname: str_at(l, "/hostname").to_string(),
                    allowed_namespaces: allowed,
                    tls,
                    attached_routes: status.and_then(|s| s.get("attachedRoutes")?.as_i64()),
                    conditions: status
                        .map(|s| conditions(s, "/conditions"))
                        .unwrap_or_default(),
                    name,
                }
            })
            .collect();
        Self {
            class: str_at(gateway, "/spec/gatewayClassName").to_string(),
            addresses,
            listeners,
            conditions: conditions(gateway, "/status/conditions"),
        }
    }

    /// `Programmed`, `Not accepted: <reason>`, `Pending`.
    pub fn state(&self) -> (String, Tone) {
        programmed_state(&self.conditions)
    }

    /// The routes attached to all listeners (as the controller counts them).
    pub fn attached_routes(&self) -> Option<i64> {
        let counts: Vec<i64> = self
            .listeners
            .iter()
            .filter_map(|l| l.attached_routes)
            .collect();
        (!counts.is_empty()).then(|| counts.iter().sum())
    }
}

/// A GatewayClass: `(controller, accepted state)`.
pub fn gateway_class(class: &Value) -> (String, (String, Tone)) {
    let conditions = conditions(class, "/status/conditions");
    let state = match condition(&conditions, "Accepted") {
        Some(c) if c.is_true() => ("Accepted".to_string(), Tone::Good),
        Some(c) if c.is_false() => (with_reason("Not accepted", &c.reason), Tone::Bad),
        _ => ("Pending".to_string(), Tone::Warning),
    };
    (str_at(class, "/spec/controllerName").to_string(), state)
}

/// A route's parent (usually a Gateway listener).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParentRef {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub section: String,
    pub port: Option<i64>,
}

impl ParentRef {
    /// Defaults filled in: group `gateway.networking.k8s.io`, kind `Gateway`, the route's
    /// namespace.
    fn parse(parent: &Value, route_namespace: &str) -> Self {
        Self {
            group: match parent.get("group").and_then(Value::as_str) {
                None => GROUP.to_string(),
                Some(g) => g.to_string(),
            },
            kind: match str_at(parent, "/kind") {
                "" => "Gateway".into(),
                k => k.to_string(),
            },
            namespace: match str_at(parent, "/namespace") {
                "" => route_namespace.to_string(),
                ns => ns.to_string(),
            },
            name: str_at(parent, "/name").to_string(),
            section: str_at(parent, "/sectionName").to_string(),
            port: parent.get("port").and_then(Value::as_i64),
        }
    }

    pub fn is_gateway(&self) -> bool {
        self.group == GROUP && self.kind == "Gateway"
    }

    /// `web-gateway`, `web-gateway/https`, `ingress/gw:443` (namespace shown when it differs).
    pub fn label(&self, route_namespace: &str) -> String {
        let mut out = if self.namespace == route_namespace {
            self.name.clone()
        } else {
            format!("{}/{}", self.namespace, self.name)
        };
        if !self.is_gateway() {
            out = format!("{} {out}", self.kind);
        }
        if !self.section.is_empty() {
            out.push('/');
            out.push_str(&self.section);
        }
        if let Some(port) = self.port {
            out.push_str(&format!(":{port}"));
        }
        out
    }

    /// The same parent, ignoring the section and port.
    fn same_object(&self, other: &ParentRef) -> bool {
        self.group == other.group
            && self.kind == other.kind
            && self.namespace == other.namespace
            && self.name == other.name
    }
}

/// A route backend.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Backend {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub port: Option<i64>,
    pub weight: i64,
}

impl Backend {
    pub fn is_service(&self) -> bool {
        self.group.is_empty() && self.kind == "Service"
    }

    /// `web:80`, `other/web:80 (90)`, `Backend x` for non-Services.
    pub fn label(&self, route_namespace: &str) -> String {
        let mut out = if self.namespace == route_namespace {
            self.name.clone()
        } else {
            format!("{}/{}", self.namespace, self.name)
        };
        if !self.is_service() {
            out = format!("{} {out}", self.kind);
        }
        if let Some(port) = self.port {
            out.push_str(&format!(":{port}"));
        }
        out
    }
}

/// One rule: what it matches and where it sends traffic.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub name: String,
    /// `PathPrefix /api · header x-canary=true`, `GET Exact /healthz`, `shop.Checkout/Pay`.
    pub matches: Vec<String>,
    pub filters: Vec<String>,
    pub backends: Vec<Backend>,
}

/// What a controller reports for one parent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParentStatus {
    pub parent: ParentRef,
    pub controller: String,
    pub conditions: Vec<Condition>,
}

impl ParentStatus {
    /// `Accepted`, `Not accepted: NotAllowedByListeners`, `Pending`, and `ResolvedRefs`
    /// problems.
    pub fn state(&self) -> (String, Tone) {
        match condition(&self.conditions, "Accepted") {
            Some(c) if c.is_true() => match condition(&self.conditions, "ResolvedRefs") {
                Some(r) if r.is_false() => (with_reason("Refs not resolved", &r.reason), Tone::Bad),
                _ => ("Accepted".into(), Tone::Good),
            },
            Some(c) if c.is_false() => (with_reason("Not accepted", &c.reason), Tone::Bad),
            _ => ("Pending".into(), Tone::Warning),
        }
    }
}

/// An HTTPRoute, GRPCRoute, TCPRoute, TLSRoute or UDPRoute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    pub kind: String,
    pub namespace: String,
    pub parents: Vec<ParentRef>,
    pub hostnames: Vec<String>,
    pub rules: Vec<Rule>,
    pub statuses: Vec<ParentStatus>,
}

impl Route {
    pub fn parse(route: &Value) -> Self {
        let namespace = crate::format::namespace(route)
            .unwrap_or_default()
            .to_string();
        let kind = str_at(route, "/kind").to_string();
        Self {
            parents: array_at(route, "/spec/parentRefs")
                .iter()
                .map(|p| ParentRef::parse(p, &namespace))
                .collect(),
            hostnames: array_at(route, "/spec/hostnames")
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
            rules: array_at(route, "/spec/rules")
                .iter()
                .map(|r| Rule {
                    name: str_at(r, "/name").to_string(),
                    matches: array_at(r, "/matches").iter().map(route_match).collect(),
                    filters: array_at(r, "/filters")
                        .iter()
                        .map(|f| str_at(f, "/type").to_string())
                        .collect(),
                    backends: array_at(r, "/backendRefs")
                        .iter()
                        .map(|b| Backend {
                            group: str_at(b, "/group").to_string(),
                            kind: match str_at(b, "/kind") {
                                "" => "Service".into(),
                                k => k.to_string(),
                            },
                            namespace: match str_at(b, "/namespace") {
                                "" => namespace.clone(),
                                ns => ns.to_string(),
                            },
                            name: str_at(b, "/name").to_string(),
                            port: b.get("port").and_then(Value::as_i64),
                            weight: b.get("weight").and_then(Value::as_i64).unwrap_or(1),
                        })
                        .collect(),
                })
                .collect(),
            statuses: array_at(route, "/status/parents")
                .iter()
                .map(|s| ParentStatus {
                    parent: ParentRef::parse(
                        s.get("parentRef").unwrap_or(&Value::Null),
                        &namespace,
                    ),
                    controller: str_at(s, "/controllerName").to_string(),
                    conditions: conditions(s, "/conditions"),
                })
                .collect(),
            kind,
            namespace,
        }
    }

    /// The status of one parent ref: the one for the same ref; for a ref to a whole Gateway (no
    /// section or port), else any status for that Gateway. A ref to one listener never takes
    /// another listener's status.
    pub fn status_of(&self, parent: &ParentRef) -> Option<&ParentStatus> {
        self.statuses
            .iter()
            .find(|s| &s.parent == parent)
            .or_else(|| {
                if !parent.section.is_empty() || parent.port.is_some() {
                    return None;
                }
                self.statuses.iter().find(|s| s.parent.same_object(parent))
            })
    }

    /// `Accepted` when every parent accepted it, `Not accepted` when one refused, else
    /// `Pending` (no controller reported yet).
    pub fn state(&self) -> (String, Tone) {
        if self.parents.is_empty() {
            return ("No parents".into(), Tone::Muted);
        }
        let states: Vec<(String, Tone)> = self
            .parents
            .iter()
            .map(|p| {
                self.status_of(p)
                    .map(ParentStatus::state)
                    .unwrap_or(("Pending".into(), Tone::Warning))
            })
            .collect();
        if let Some(bad) = states.iter().find(|(_, t)| *t == Tone::Bad) {
            return bad.clone();
        }
        if states.iter().all(|(_, t)| *t == Tone::Good) {
            return ("Accepted".into(), Tone::Good);
        }
        if states.iter().any(|(_, t)| *t == Tone::Good) {
            return ("Partly accepted".into(), Tone::Warning);
        }
        ("Pending".into(), Tone::Warning)
    }

    /// The backends of every rule.
    pub fn backends(&self) -> impl Iterator<Item = &Backend> {
        self.rules.iter().flat_map(|r| r.backends.iter())
    }

    /// The parent refs that name the Gateway `namespace/name`.
    pub fn parents_on(&self, namespace: &str, name: &str) -> Vec<&ParentRef> {
        self.parents
            .iter()
            .filter(|p| p.is_gateway() && p.namespace == namespace && p.name == name)
            .collect()
    }

    /// Whether a controller accepted it on the Gateway `namespace/name`.
    pub fn accepted_on(&self, namespace: &str, name: &str) -> bool {
        self.parents_on(namespace, name).iter().any(|p| {
            self.status_of(p)
                .and_then(|s| condition(&s.conditions, "Accepted"))
                .is_some_and(Condition::is_true)
        })
    }

    /// The backends that send traffic to the Service `namespace/name`.
    pub fn backends_to(&self, namespace: &str, name: &str) -> Vec<&Backend> {
        self.backends()
            .filter(|b| b.is_service() && b.namespace == namespace && b.name == name)
            .collect()
    }
}

fn route_match(m: &Value) -> String {
    let mut parts = Vec::new();
    // HTTPRoute
    let method = str_at(m, "/method");
    if !method.is_empty() {
        parts.push(method.to_string());
    }
    if let Some(path) = m.get("path") {
        parts.push(format!(
            "{} {}",
            match str_at(path, "/type") {
                "" => "PathPrefix",
                t => t,
            },
            str_at(path, "/value")
        ));
    }
    // GRPCRoute: `method: {service, method}`.
    if let Some(grpc) = m.get("method").filter(|v| v.is_object()) {
        let service = str_at(grpc, "/service");
        let method = str_at(grpc, "/method");
        parts.push(match (service, method) {
            ("", "") => "any method".into(),
            (s, "") => format!("{s}/*"),
            ("", m) => format!("*/{m}"),
            (s, m) => format!("{s}/{m}"),
        });
    }
    for header in array_at(m, "/headers") {
        parts.push(format!(
            "header {}={}",
            str_at(header, "/name"),
            str_at(header, "/value")
        ));
    }
    for query in array_at(m, "/queryParams") {
        parts.push(format!(
            "query {}={}",
            str_at(query, "/name"),
            str_at(query, "/value")
        ));
    }
    if parts.is_empty() {
        "everything".into()
    } else {
        parts.join(" · ")
    }
}

/// Whether a ReferenceGrant in the target's namespace lets a `kind` in `from_namespace` refer to
/// the Service `name`.
pub fn grant_allows(grant: &Value, kind: &str, from_namespace: &str, name: &str) -> bool {
    let from = array_at(grant, "/spec/from").iter().any(|f| {
        str_at(f, "/group") == GROUP
            && str_at(f, "/kind") == kind
            && str_at(f, "/namespace") == from_namespace
    });
    let to = array_at(grant, "/spec/to").iter().any(|t| {
        str_at(t, "/group").is_empty()
            && str_at(t, "/kind") == "Service"
            && match str_at(t, "/name") {
                "" => true,
                only => only == name,
            }
    });
    from && to
}

/// Whether any grant lets routes from another namespace refer to Services here.
pub fn grants_routes<'a>(grants: impl IntoIterator<Item = &'a Value>) -> bool {
    grants.into_iter().any(|grant| {
        array_at(grant, "/spec/from")
            .iter()
            .any(|f| str_at(f, "/group") == GROUP && is_route(GROUP, str_at(f, "/kind")))
            && array_at(grant, "/spec/to")
                .iter()
                .any(|t| str_at(t, "/group").is_empty() && str_at(t, "/kind") == "Service")
    })
}

/// The `(route kind, plural, namespace)` the grants (of a Service's namespace) let refer to the
/// Service `name`: where the routes that may send it traffic from other namespaces live.
pub fn granted_route_sources<'a>(
    grants: impl IntoIterator<Item = &'a Value>,
    name: &str,
) -> Vec<(&'static str, &'static str, String)> {
    let mut out: Vec<(&'static str, &'static str, String)> = Vec::new();
    for grant in grants {
        let to_service = array_at(grant, "/spec/to").iter().any(|t| {
            str_at(t, "/group").is_empty()
                && str_at(t, "/kind") == "Service"
                && match str_at(t, "/name") {
                    "" => true,
                    only => only == name,
                }
        });
        if !to_service {
            continue;
        }
        for from in array_at(grant, "/spec/from") {
            if str_at(from, "/group") != GROUP {
                continue;
            }
            let Some((kind, plural)) = ROUTE_KINDS
                .iter()
                .find(|(kind, _)| *kind == str_at(from, "/kind"))
            else {
                continue;
            };
            let namespace = str_at(from, "/namespace").to_string();
            if namespace.is_empty() {
                continue;
            }
            let entry = (*kind, *plural, namespace);
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
    }
    out.sort();
    out
}

/// Which routes may attach to a Gateway: the route kinds (`(kind, plural)`) its listeners
/// allow (`allowedRoutes.kinds`, else those of the listener's protocol) and whether any
/// listener takes routes from other namespaces (`allowedRoutes.namespaces.from` `All` or
/// `Selector`; the default is `Same`).
pub fn attachable_routes(gateway: &Value) -> (Vec<(&'static str, &'static str)>, bool) {
    let mut kinds: Vec<(&'static str, &'static str)> = Vec::new();
    let mut add = |kind: &str| {
        if let Some(entry) = ROUTE_KINDS.iter().find(|(k, _)| *k == kind)
            && !kinds.contains(entry)
        {
            kinds.push(*entry);
        }
    };
    let mut other_namespaces = false;
    for listener in array_at(gateway, "/spec/listeners") {
        if matches!(
            str_at(listener, "/allowedRoutes/namespaces/from"),
            "All" | "Selector"
        ) {
            other_namespaces = true;
        }
        let allowed = array_at(listener, "/allowedRoutes/kinds");
        if !allowed.is_empty() {
            for kind in allowed {
                if matches!(
                    kind.get("group").and_then(Value::as_str),
                    None | Some(GROUP)
                ) {
                    add(str_at(kind, "/kind"));
                }
            }
            continue;
        }
        let by_protocol: &[&str] = match str_at(listener, "/protocol") {
            "HTTP" | "HTTPS" => &["HTTPRoute", "GRPCRoute"],
            "TLS" => &["TLSRoute", "TCPRoute"],
            "TCP" => &["TCPRoute"],
            "UDP" => &["UDPRoute"],
            // An implementation's own protocol: any kind may be meant.
            _ => &["HTTPRoute", "GRPCRoute", "TLSRoute", "TCPRoute", "UDPRoute"],
        };
        for kind in by_protocol {
            add(kind);
        }
    }
    kinds.sort_by_key(|entry| ROUTE_KINDS.iter().position(|k| k == entry));
    (kinds, other_namespaces)
}

/// The routes (`(kind, object)`) that send traffic to the Service `namespace/name`: routes in
/// its namespace, and routes elsewhere a ReferenceGrant of the namespace allows.
pub fn routes_to_service<'a>(
    namespace: &str,
    name: &str,
    routes: impl IntoIterator<Item = &'a Value>,
    grants: &[&Value],
) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = routes
        .into_iter()
        .filter(|route| {
            let parsed = Route::parse(route);
            if parsed.backends_to(namespace, name).is_empty() {
                return false;
            }
            parsed.namespace == namespace
                || grants
                    .iter()
                    .any(|g| grant_allows(g, &parsed.kind, &parsed.namespace, name))
        })
        .collect();
    out.sort_by_key(|r| (str_at(r, "/kind"), crate::format::name(r)));
    out
}

/// The routes attached to the Gateway `namespace/name` (they name it as a parent).
pub fn routes_on_gateway<'a>(
    namespace: &str,
    name: &str,
    routes: impl IntoIterator<Item = &'a Value>,
) -> Vec<&'a Value> {
    let mut out: Vec<&Value> = routes
        .into_iter()
        .filter(|route| !Route::parse(route).parents_on(namespace, name).is_empty())
        .collect();
    out.sort_by_key(|r| (str_at(r, "/kind"), crate::format::name(r)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cond(kind: &str, status: &str, reason: &str) -> Value {
        json!({"type": kind, "status": status, "reason": reason})
    }

    fn gateway() -> Value {
        json!({"apiVersion": "gateway.networking.k8s.io/v1", "kind": "Gateway",
            "metadata": {"name": "web-gateway", "namespace": "kubyl-views"},
            "spec": {"gatewayClassName": "kubyl-fake", "listeners": [
                {"name": "http", "protocol": "HTTP", "port": 80, "allowedRoutes": {"namespaces": {"from": "Same"}}},
                {"name": "https", "protocol": "HTTPS", "port": 443, "hostname": "*.shop.example.com",
                 "tls": {"mode": "Terminate", "certificateRefs": [{"kind": "Secret", "name": "shop-tls"}]},
                 "allowedRoutes": {"namespaces": {"from": "Selector", "selector": {"matchLabels": {"shared": "true"}}}}}]},
            "status": {"addresses": [{"type": "IPAddress", "value": "172.18.0.240"}],
                "conditions": [cond("Accepted", "True", "Accepted"), cond("Programmed", "True", "Programmed")],
                "listeners": [
                    {"name": "http", "attachedRoutes": 1, "conditions": [cond("Programmed", "True", "Programmed")]},
                    {"name": "https", "attachedRoutes": 2, "conditions": [cond("Programmed", "False", "Invalid")]}]}})
    }

    fn http_route(version: &str) -> Value {
        json!({"apiVersion": format!("gateway.networking.k8s.io/{version}"), "kind": "HTTPRoute",
            "metadata": {"name": "shop", "namespace": "kubyl-views"},
            "spec": {"parentRefs": [{"name": "web-gateway", "sectionName": "http"},
                                    {"name": "web-gateway", "sectionName": "https"}],
                "hostnames": ["shop.example.com"],
                "rules": [
                    {"matches": [{"path": {"type": "PathPrefix", "value": "/api"},
                                  "headers": [{"name": "x-canary", "value": "true"}]}],
                     "backendRefs": [{"name": "web-canary", "port": 80}]},
                    {"matches": [{"path": {"value": "/"}, "method": "GET"}],
                     "filters": [{"type": "RequestHeaderModifier"}],
                     "backendRefs": [{"name": "web", "port": 80, "weight": 90},
                                     {"name": "web", "namespace": "other", "port": 80, "weight": 10}]}]},
            "status": {"parents": [
                {"parentRef": {"group": "gateway.networking.k8s.io", "kind": "Gateway", "name": "web-gateway", "sectionName": "http"},
                 "controllerName": "kubyl.dev/fake", "conditions": [cond("Accepted", "True", "Accepted")]},
                {"parentRef": {"name": "web-gateway", "sectionName": "https"},
                 "controllerName": "kubyl.dev/fake", "conditions": [cond("Accepted", "False", "NotAllowedByListeners")]}]}})
    }

    #[test]
    fn gateways_and_listeners() {
        let gw = Gateway::parse(&gateway());
        assert_eq!(gw.class, "kubyl-fake");
        assert_eq!(gw.addresses, ["172.18.0.240"]);
        assert_eq!(gw.state(), ("Programmed".into(), Tone::Good));
        assert_eq!(gw.attached_routes(), Some(3));
        assert_eq!(gw.listeners[0].allowed_namespaces, "Same");
        assert_eq!(gw.listeners[1].allowed_namespaces, "Selector shared=true");
        assert_eq!(
            gw.listeners[1].tls.as_deref(),
            Some("Terminate · Secret shop-tls")
        );
        assert_eq!(
            gw.listeners[1].state(),
            ("Not programmed: Invalid".into(), Tone::Bad)
        );
        // No controller yet.
        let pending = Gateway::parse(&json!({"spec": {"gatewayClassName": "x",
            "addresses": [{"value": "10.0.0.1"}], "listeners": [{"name": "a"}]}}));
        assert_eq!(pending.state(), ("Pending".into(), Tone::Warning));
        assert_eq!(pending.addresses, ["10.0.0.1"]);
        assert_eq!(pending.attached_routes(), None);
        let class = json!({"spec": {"controllerName": "kubyl.dev/fake"},
            "status": {"conditions": [cond("Accepted", "False", "InvalidParameters")]}});
        assert_eq!(
            gateway_class(&class),
            (
                "kubyl.dev/fake".into(),
                ("Not accepted: InvalidParameters".into(), Tone::Bad)
            )
        );
    }

    #[test]
    fn routes_in_every_version() {
        for version in ["v1", "v1beta1"] {
            let route = Route::parse(&http_route(version));
            assert_eq!(route.kind, "HTTPRoute");
            assert_eq!(route.parents.len(), 2);
            assert_eq!(route.parents[1].label("kubyl-views"), "web-gateway/https");
            assert_eq!(
                route.status_of(&route.parents[0]).unwrap().state(),
                ("Accepted".into(), Tone::Good)
            );
            assert_eq!(
                route.state(),
                ("Not accepted: NotAllowedByListeners".into(), Tone::Bad)
            );
            assert_eq!(
                route.rules[0].matches,
                ["PathPrefix /api · header x-canary=true"]
            );
            assert_eq!(route.rules[1].matches, ["GET · PathPrefix /"]);
            assert_eq!(route.rules[1].filters, ["RequestHeaderModifier"]);
            assert_eq!(
                route.rules[1].backends[1].label("kubyl-views"),
                "other/web:80"
            );
            assert_eq!(route.backends_to("kubyl-views", "web").len(), 1);
            assert!(route.accepted_on("kubyl-views", "web-gateway"));
            assert!(!route.accepted_on("kubyl-views", "other"));
        }
        let grpc = Route::parse(
            &json!({"kind": "GRPCRoute", "metadata": {"namespace": "ns"},
            "spec": {"parentRefs": [{"name": "gw", "namespace": "infra", "port": 443}],
                "rules": [{"matches": [{"method": {"service": "shop.Checkout", "method": "Pay"}}],
                           "backendRefs": [{"name": "checkout", "port": 9090}]}]}}),
        );
        assert_eq!(grpc.rules[0].matches, ["shop.Checkout/Pay"]);
        assert_eq!(grpc.parents[0].label("ns"), "infra/gw:443");
        assert_eq!(grpc.state(), ("Pending".into(), Tone::Warning));
        // TCPRoute (v1alpha2): backends only.
        let tcp = Route::parse(&json!({"kind": "TCPRoute", "metadata": {"namespace": "ns"},
            "spec": {"parentRefs": [{"name": "gw"}], "rules": [{"backendRefs": [{"name": "db", "port": 5432}]}]}}));
        assert_eq!(tcp.rules[0].matches, Vec::<String>::new());
        assert_eq!(tcp.backends_to("ns", "db")[0].port, Some(5432));
    }

    #[test]
    fn service_routes_need_a_grant_across_namespaces() {
        let local = http_route("v1");
        let mut remote = http_route("v1");
        remote["metadata"]["namespace"] = json!("default");
        remote["metadata"]["name"] = json!("remote");
        remote["spec"]["rules"][0]["backendRefs"][0]["namespace"] = json!("kubyl-views");
        let routes = [local.clone(), remote.clone()];
        let found = routes_to_service("kubyl-views", "web-canary", routes.iter(), &[]);
        assert_eq!(found.len(), 1);
        let grant = json!({"spec": {"from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "default"}],
            "to": [{"group": "", "kind": "Service"}]}});
        assert!(grants_routes([&grant]));
        let found = routes_to_service("kubyl-views", "web-canary", routes.iter(), &[&grant]);
        assert_eq!(found.len(), 2);
        let named = json!({"spec": {"from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "default"}],
            "to": [{"group": "", "kind": "Service", "name": "web"}]}});
        assert!(!grant_allows(&named, "HTTPRoute", "default", "web-canary"));
        assert!(grant_allows(&named, "HTTPRoute", "default", "web"));
        assert_eq!(
            routes_on_gateway("kubyl-views", "web-gateway", routes.iter()).len(),
            1
        );
        assert!(is_route(GROUP, "TLSRoute"));
        assert!(!is_route(GROUP, "Gateway"));
        // Where cross-namespace routes may come from.
        let other_kind = json!({"spec": {"from": [{"group": "gateway.networking.k8s.io", "kind": "GRPCRoute", "namespace": "shop"}],
            "to": [{"group": "", "kind": "Service"}]}});
        assert_eq!(
            granted_route_sources([&grant, &named, &other_kind], "web-canary"),
            [
                ("GRPCRoute", "grpcroutes", "shop".to_string()),
                ("HTTPRoute", "httproutes", "default".to_string())
            ]
        );
        assert_eq!(granted_route_sources([&named], "web").len(), 1);
        // A grant to a Service of another group isn't one to a core Service.
        let other_group = json!({"spec": {"from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "default"}],
            "to": [{"group": "example.com", "kind": "Service"}]}});
        assert!(!grants_routes([&other_group]));
        assert!(granted_route_sources([&other_group], "web").is_empty());
    }

    #[test]
    fn a_listener_ref_takes_only_its_own_status() {
        // Attached to both listeners, but the controller reported only on `http`.
        let mut route = http_route("v1");
        route["status"]["parents"] = json!([
            {"parentRef": {"name": "web-gateway", "sectionName": "http"},
             "conditions": [cond("Accepted", "True", "Accepted")]}]);
        let parsed = Route::parse(&route);
        assert!(parsed.status_of(&parsed.parents[0]).is_some());
        assert!(parsed.status_of(&parsed.parents[1]).is_none());
        assert_eq!(parsed.state(), ("Partly accepted".into(), Tone::Warning));
        // A ref to the whole Gateway takes a status reported for one of its listeners.
        route["spec"]["parentRefs"] = json!([{"name": "web-gateway"}]);
        let parsed = Route::parse(&route);
        assert_eq!(parsed.state(), ("Accepted".into(), Tone::Good));
    }

    #[test]
    fn gateways_say_which_routes_may_attach() {
        let (kinds, other) = attachable_routes(&gateway());
        assert_eq!(
            kinds,
            [("HTTPRoute", "httproutes"), ("GRPCRoute", "grpcroutes")]
        );
        assert!(
            other,
            "the https listener takes routes from selected namespaces"
        );
        let tcp = json!({"spec": {"listeners": [
            {"name": "db", "protocol": "TCP", "port": 5432},
            {"name": "custom", "protocol": "HTTP", "port": 8080,
             "allowedRoutes": {"kinds": [{"kind": "GRPCRoute"}, {"group": "example.com", "kind": "FooRoute"}]}}]}});
        let (kinds, other) = attachable_routes(&tcp);
        assert_eq!(
            kinds,
            [("GRPCRoute", "grpcroutes"), ("TCPRoute", "tcproutes")]
        );
        assert!(!other);
    }
}
