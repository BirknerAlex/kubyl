//! OpenShift Routes (`route.openshift.io/v1`): a small model the columns, details, web views,
//! port-forwards, palette and masking share.
//!
//! - [`Route::parse`]: host, path, wildcard policy, backends (`spec.to` first, then
//!   `spec.alternateBackends`), target port, TLS and the per-router status of `status.ingress`.
//! - [`url`]: what a browser opens (none for wildcard hosts), [`services_label`]: the backends
//!   with weights like `oc get routes` (`shop-web(80%),shop-canary(20%)`).
//! - [`resolve_target_port`]: the Service port a Route's target port selects, like OpenShift's
//!   router does (what a Service port-forward or web view needs); [`resolve_target_port_with`]
//!   also reads the endpoints, for numeric target ports behind named Service target ports.
//! - [`has_inline_key`] / [`mask_inline_key`]: `spec.tls.key` holds a private key. It's masked
//!   like Secret data wherever Kubyl shows or copies an object, unless the user reveals it.

use std::fmt;

use serde_json::Value;

use crate::format::{array_at, str_at};

pub const GROUP: &str = "route.openshift.io";
pub const KIND: &str = "Route";
/// The plural resource name.
pub const RESOURCE: &str = "routes";
/// Shown instead of the inline TLS key (the same mask as Secret values everywhere else).
pub const MASK: &str = "••••••••";
/// A backend's weight when it has none (the API server's default, 0–256).
pub const DEFAULT_WEIGHT: u32 = 100;
/// kubectl's copy of the last applied object: on a Route with an inline key it holds the key.
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// Whether `group`/`kind` is an OpenShift Route (other groups have kinds named `Route`, e.g.
/// Knative Serving).
pub fn is_route(group: &str, kind: &str) -> bool {
    group == GROUP && kind == KIND
}

/// Whether a JSON object is an OpenShift Route. Objects without `kind`/`apiVersion` (some
/// watch caches drop them) count when they have a `spec.to`.
pub fn is_route_object(object: &Value) -> bool {
    let kind = object.get("kind").and_then(Value::as_str);
    let api_version = object.get("apiVersion").and_then(Value::as_str);
    match (kind, api_version) {
        (Some(kind), Some(api)) => kind == KIND && api.starts_with("route.openshift.io/"),
        (Some(kind), None) => kind == KIND,
        (None, _) => object.pointer("/spec/to").is_some(),
    }
}

/// `spec.port.targetPort`: a port name or number on the pods behind the Service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetPort {
    Number(i64),
    Name(String),
}

impl fmt::Display for TargetPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetPort::Number(n) => write!(f, "{n}"),
            TargetPort::Name(name) => f.write_str(name),
        }
    }
}

/// `spec.to` or one of `spec.alternateBackends`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backend {
    /// `Service` (the only kind routers support today).
    pub kind: String,
    pub name: String,
    /// 0–256; [`DEFAULT_WEIGHT`] when unset.
    pub weight: u32,
}

impl Backend {
    pub fn is_service(&self) -> bool {
        self.kind == "Service"
    }
}

/// `spec.tls`. Only whether the certificates and the key are set is kept, never their text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tls {
    /// `edge`, `passthrough` or `reencrypt`.
    pub termination: String,
    /// `None`, `Allow` or `Redirect` (what happens to plain HTTP), when set.
    pub insecure_policy: Option<String>,
    pub certificate: bool,
    pub key: bool,
    pub ca_certificate: bool,
    pub destination_ca_certificate: bool,
}

impl Tls {
    /// Like `oc get routes`' TERMINATION column: `edge/Redirect`, `passthrough`.
    pub fn label(&self) -> String {
        match self.insecure_policy.as_deref() {
            Some(policy) if !policy.is_empty() && policy != "None" => {
                format!("{}/{policy}", self.termination)
            }
            _ => self.termination.clone(),
        }
    }

    /// `edge · insecure Redirect`.
    pub fn describe(&self) -> String {
        match self.insecure_policy.as_deref() {
            Some(policy) if !policy.is_empty() => {
                format!("{} · insecure {policy}", self.termination)
            }
            _ => self.termination.clone(),
        }
    }

    /// The router talks TLS to the backend (the Service port serves HTTPS).
    pub fn backend_tls(&self) -> bool {
        matches!(self.termination.as_str(), "passthrough" | "reencrypt")
    }
}

/// The `Admitted` condition of one router.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Admission {
    /// `True`, `False` or `Unknown`.
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition: Option<String>,
}

/// One entry of `status.ingress`: what a router made of the Route.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouterStatus {
    pub router: String,
    pub host: Option<String>,
    pub canonical_hostname: Option<String>,
    pub wildcard_policy: Option<String>,
    pub admitted: Option<Admission>,
}

impl RouterStatus {
    pub fn is_admitted(&self) -> bool {
        self.admitted.as_ref().is_some_and(|a| a.status == "True")
    }

    /// The router refused it (`Admitted=False`: `HostAlreadyClaimed`, `ExtendedValidationFailed`…).
    pub fn is_rejected(&self) -> bool {
        self.admitted.as_ref().is_some_and(|a| a.status == "False")
    }
}

/// A parsed Route.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub namespace: Option<String>,
    /// `spec.host` (the API server fills it in when the author left it out).
    pub host: Option<String>,
    pub path: Option<String>,
    /// `None` or `Subdomain`.
    pub wildcard_policy: String,
    /// `spec.to`, then `spec.alternateBackends`.
    pub backends: Vec<Backend>,
    /// `spec.port.targetPort`; `None`: every port of the Service (`<all>` in `oc`).
    pub target_port: Option<TargetPort>,
    pub tls: Option<Tls>,
    /// `status.ingress`, one entry per router.
    pub routers: Vec<RouterStatus>,
}

fn non_empty(value: &Value, pointer: &str) -> Option<String> {
    Some(str_at(value, pointer))
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn backend(value: &Value) -> Option<Backend> {
    let name = non_empty(value, "/name")?;
    Some(Backend {
        kind: non_empty(value, "/kind").unwrap_or_else(|| "Service".into()),
        name,
        weight: value
            .get("weight")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_WEIGHT, |w| w.min(u64::from(u32::MAX)) as u32),
    })
}

impl Route {
    pub fn parse(object: &Value) -> Self {
        let spec = &object["spec"];
        let backends = std::iter::once(&spec["to"])
            .chain(array_at(spec, "/alternateBackends"))
            .filter_map(backend)
            .collect();
        let target_port = match spec.pointer("/port/targetPort") {
            Some(Value::Number(n)) => n.as_i64().map(TargetPort::Number),
            Some(Value::String(s)) if !s.is_empty() => Some(match s.parse::<i64>() {
                Ok(n) => TargetPort::Number(n),
                Err(_) => TargetPort::Name(s.clone()),
            }),
            _ => None,
        };
        let tls = spec.get("tls").filter(|t| t.is_object()).map(|tls| {
            let set = |key: &str| {
                tls.get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            };
            Tls {
                termination: non_empty(tls, "/termination").unwrap_or_else(|| "edge".into()),
                insecure_policy: non_empty(tls, "/insecureEdgeTerminationPolicy"),
                certificate: set("certificate"),
                key: set("key"),
                ca_certificate: set("caCertificate"),
                destination_ca_certificate: set("destinationCACertificate"),
            }
        });
        let routers = array_at(object, "/status/ingress")
            .iter()
            .map(|ingress| RouterStatus {
                router: non_empty(ingress, "/routerName").unwrap_or_else(|| "default".into()),
                host: non_empty(ingress, "/host"),
                canonical_hostname: non_empty(ingress, "/routerCanonicalHostname"),
                wildcard_policy: non_empty(ingress, "/wildcardPolicy"),
                admitted: array_at(ingress, "/conditions")
                    .iter()
                    .find(|c| str_at(c, "/type") == "Admitted")
                    .map(|c| Admission {
                        status: str_at(c, "/status").to_string(),
                        reason: non_empty(c, "/reason"),
                        message: non_empty(c, "/message"),
                        last_transition: non_empty(c, "/lastTransitionTime"),
                    }),
            })
            .collect();
        Self {
            name: str_at(object, "/metadata/name").to_string(),
            namespace: non_empty(object, "/metadata/namespace"),
            host: non_empty(spec, "/host"),
            path: non_empty(spec, "/path"),
            wildcard_policy: non_empty(spec, "/wildcardPolicy").unwrap_or_else(|| "None".into()),
            backends,
            target_port,
            tls,
            routers,
        }
    }

    /// The host a client uses: `spec.host`, else what a router admitted (or reported).
    pub fn host(&self) -> Option<&str> {
        self.host
            .as_deref()
            .or_else(|| self.admitted_host())
            .or_else(|| self.routers.iter().find_map(|r| r.host.as_deref()))
    }

    /// The host of the first router that admitted the Route. `spec.host` alone is whatever the
    /// author wrote; only an admitted host is known to be served.
    pub fn admitted_host(&self) -> Option<&str> {
        self.routers
            .iter()
            .filter(|r| r.is_admitted())
            .find_map(|r| r.host.as_deref())
    }

    /// `Subdomain` policy (the Route serves every host of its domain) or a `*.` host.
    pub fn is_wildcard(&self) -> bool {
        self.wildcard_policy == "Subdomain" || self.host().is_some_and(|h| h.starts_with("*."))
    }

    /// The host as users see it: `*.apps.example.com` for Subdomain wildcards.
    pub fn display_host(&self) -> Option<String> {
        let host = self.host()?;
        if self.wildcard_policy == "Subdomain" && !host.starts_with("*.") {
            return Some(match host.split_once('.') {
                Some((_, domain)) => format!("*.{domain}"),
                None => format!("*.{host}"),
            });
        }
        Some(host.to_string())
    }

    /// `https` when the router terminates TLS (edge, passthrough, reencrypt), else `http`.
    pub fn scheme(&self) -> &'static str {
        if self.tls.is_some() { "https" } else { "http" }
    }

    /// The Service backends, `spec.to` first.
    pub fn services(&self) -> impl Iterator<Item = &Backend> {
        self.backends.iter().filter(|b| b.is_service())
    }

    /// Some router admitted it.
    pub fn is_admitted(&self) -> bool {
        self.routers.iter().any(RouterStatus::is_admitted)
    }

    /// Some router refused it.
    pub fn is_rejected(&self) -> bool {
        self.routers.iter().any(RouterStatus::is_rejected)
    }

    /// `<all>` without a target port (every Service port), else the port name or number.
    pub fn target_port_label(&self) -> String {
        self.target_port
            .as_ref()
            .map_or_else(|| "<all>".to_string(), ToString::to_string)
    }
}

/// What a browser opens: `https://` for edge, passthrough and reencrypt, `http://` otherwise,
/// plus the host and path. `None` for wildcard Routes and Routes without a host.
pub fn url(route: &Route) -> Option<String> {
    if route.is_wildcard() {
        return None;
    }
    let host = route.host()?;
    let path = route.path.as_deref().unwrap_or_default();
    Some(format!("{}://{host}{path}", route.scheme()))
}

/// Each backend with its share of the traffic in percent, like `oc get routes`: `None` for a
/// single backend (it gets everything its weight allows).
pub fn weights(route: &Route) -> Vec<(String, Option<u32>)> {
    let total: u64 = route.backends.iter().map(|b| u64::from(b.weight)).sum();
    route
        .backends
        .iter()
        .map(|b| {
            let percent = match (route.backends.len(), total) {
                (1, t) if t != 0 => None,
                (_, 0) => Some(0),
                _ => Some((u64::from(b.weight) * 100 / total) as u32),
            };
            (b.name.clone(), percent)
        })
        .collect()
}

/// `shop-web(80%),shop-canary(20%)`, or just `shop-web`.
pub fn services_label(route: &Route) -> String {
    weights(route)
        .into_iter()
        .map(|(name, percent)| match percent {
            Some(p) => format!("{name}({p}%)"),
            None => name,
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The Service port a Route's target port selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServicePortMatch {
    /// `spec.ports[].port`: what a Service port-forward or web view asks for.
    pub port: u16,
    pub name: Option<String>,
    /// The Service port's `targetPort` (`8080`, `http`); the port itself when unset.
    pub target: String,
    pub protocol: String,
}

impl ServicePortMatch {
    /// `80/TCP → 8080`, or `80/TCP` when the target is the same port.
    pub fn label(&self) -> String {
        if self.target == self.port.to_string() {
            format!("{}/{}", self.port, self.protocol)
        } else {
            format!("{}/{} → {}", self.port, self.protocol, self.target)
        }
    }
}

/// Resolves a Route's target port against its Service, like OpenShift's router: the target port
/// names the Service's *target* port. A name matches a Service port's `name` (the endpoints'
/// port name), a number matches a Service port's `targetPort` (a port without one targets its
/// own number). Without a target port the Service's first port serves.
pub fn resolve_target_port(
    target: Option<&TargetPort>,
    service: &Value,
) -> Result<ServicePortMatch, String> {
    let service_name = str_at(service, "/metadata/name");
    let ports = array_at(service, "/spec/ports");
    let matched = |port: &Value| -> Option<ServicePortMatch> {
        let number = u16::try_from(port.get("port")?.as_i64()?).ok()?;
        let target = match port.get("targetPort") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => number.to_string(),
        };
        Some(ServicePortMatch {
            port: number,
            name: non_empty(port, "/name"),
            target,
            protocol: non_empty(port, "/protocol").unwrap_or_else(|| "TCP".into()),
        })
    };
    let found = match target {
        None => ports.first().and_then(matched),
        Some(TargetPort::Name(name)) => ports
            .iter()
            .find(|p| str_at(p, "/name") == name)
            .and_then(matched),
        Some(TargetPort::Number(number)) => ports
            .iter()
            .filter_map(matched)
            .find(|m| m.target == number.to_string()),
    };
    found.ok_or_else(|| match target {
        None => format!("Service {service_name} has no ports"),
        Some(TargetPort::Name(name)) => format!("port {name} not found on Service {service_name}"),
        Some(TargetPort::Number(n)) => {
            format!("target port {n} not found on Service {service_name}")
        }
    })
}

/// The ports a Service's endpoints serve, as `(Service port name, number)`: from its
/// EndpointSlices (`ports[]`, matched by the `kubernetes.io/service-name` label) or its
/// Endpoints object (`subsets[].ports[]`) among `objects`.
pub fn endpoint_ports<'a>(
    objects: impl IntoIterator<Item = &'a Value>,
    service: &str,
) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    for object in objects {
        let ports: Vec<&Value> = if str_at(object, "/metadata/labels/kubernetes.io~1service-name")
            == service
        {
            array_at(object, "/ports").iter().collect()
        } else if object.get("subsets").is_some() && str_at(object, "/metadata/name") == service {
            array_at(object, "/subsets")
                .iter()
                .flat_map(|s| array_at(s, "/ports"))
                .collect()
        } else {
            continue;
        };
        for port in ports {
            if let Some(number) = port
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
            {
                let entry = (str_at(port, "/name").to_string(), number);
                if !out.contains(&entry) {
                    out.push(entry);
                }
            }
        }
    }
    out
}

/// [`resolve_target_port`] the way the router sees it, with the Service's [`endpoint_ports`]:
/// a numeric target port also matches a Service port whose `targetPort` names a container
/// port, when the endpoints serve that name on that number (`targetPort: http` → 8080).
pub fn resolve_target_port_with(
    target: Option<&TargetPort>,
    service: &Value,
    endpoints: &[(String, u16)],
) -> Result<ServicePortMatch, String> {
    let err = match resolve_target_port(target, service) {
        Ok(matched) => return Ok(matched),
        Err(err) => err,
    };
    let Some(TargetPort::Number(number)) = target else {
        return Err(err);
    };
    endpoints
        .iter()
        .filter(|(_, port)| i64::from(*port) == *number)
        .find_map(|(name, _)| {
            let named = TargetPort::Name(name.clone());
            resolve_target_port(Some(&named), service).ok()
        })
        .map(|matched| ServicePortMatch {
            target: number.to_string(),
            ..matched
        })
        .ok_or(err)
}

/// The inline private key (`spec.tls.key`) of a Route, for an explicit reveal or copy only.
pub fn inline_key(object: &Value) -> Option<&str> {
    if !is_route_object(object) {
        return None;
    }
    object
        .pointer("/spec/tls/key")
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty())
}

/// Whether `object` is a Route that carries an inline private key.
pub fn has_inline_key(object: &Value) -> bool {
    inline_key(object).is_some()
}

/// Replaces a Route's inline key with [`MASK`], and kubectl's last-applied annotation (which
/// holds the key too). Returns whether anything was masked.
pub fn mask_inline_key(object: &mut Value) -> bool {
    if !has_inline_key(object) {
        return false;
    }
    if let Some(key) = object.pointer_mut("/spec/tls/key") {
        *key = Value::String(MASK.into());
    }
    if let Some(annotation) = object
        .pointer_mut("/metadata/annotations")
        .and_then(Value::as_object_mut)
        .and_then(|a| a.get_mut(LAST_APPLIED))
    {
        *annotation = Value::String(MASK.into());
    }
    true
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEv\n-----END PRIVATE KEY-----\n";

    fn route(spec: Value, status: Value) -> Value {
        json!({"apiVersion": "route.openshift.io/v1", "kind": "Route",
            "metadata": {"name": "shop", "namespace": "shop"}, "spec": spec, "status": status})
    }

    fn admitted(router: &str, host: &str, status: &str, reason: Option<&str>) -> Value {
        let mut condition = json!({"type": "Admitted", "status": status,
            "lastTransitionTime": "2026-09-26T10:00:00Z"});
        if let Some(reason) = reason {
            condition["reason"] = json!(reason);
            condition["message"] = json!("route shop-old already exposes the host");
        }
        json!({"routerName": router, "host": host, "wildcardPolicy": "None",
            "routerCanonicalHostname": format!("router-{router}.apps.example.com"),
            "conditions": [condition]})
    }

    fn service(ports: Value) -> Value {
        json!({"metadata": {"name": "shop-web", "namespace": "shop"}, "spec": {"ports": ports}})
    }

    #[test]
    fn parses_backends_tls_and_router_status() {
        let object = route(
            json!({"host": "shop.apps.example.com", "path": "/store",
                "to": {"kind": "Service", "name": "shop-web", "weight": 80},
                "alternateBackends": [{"kind": "Service", "name": "shop-canary", "weight": 20}],
                "port": {"targetPort": "http"},
                "tls": {"termination": "edge", "insecureEdgeTerminationPolicy": "Redirect",
                        "certificate": "CERT", "key": KEY}}),
            json!({"ingress": [
                admitted("default", "shop.apps.example.com", "True", None),
                admitted("sharded", "shop.apps.example.com", "False", Some("HostAlreadyClaimed")),
            ]}),
        );
        let route = Route::parse(&object);
        assert_eq!(route.name, "shop");
        assert_eq!(route.namespace.as_deref(), Some("shop"));
        assert_eq!(route.path.as_deref(), Some("/store"));
        assert_eq!(route.wildcard_policy, "None");
        assert_eq!(
            route.backends,
            [
                Backend {
                    kind: "Service".into(),
                    name: "shop-web".into(),
                    weight: 80
                },
                Backend {
                    kind: "Service".into(),
                    name: "shop-canary".into(),
                    weight: 20
                }
            ]
        );
        assert_eq!(route.target_port, Some(TargetPort::Name("http".into())));
        let tls = route.tls.as_ref().unwrap();
        assert_eq!(tls.label(), "edge/Redirect");
        assert_eq!(tls.describe(), "edge · insecure Redirect");
        assert!(tls.certificate && tls.key && !tls.ca_certificate);
        assert!(!tls.backend_tls());
        assert_eq!(route.routers.len(), 2);
        assert!(route.routers[0].is_admitted());
        assert!(route.routers[1].is_rejected());
        let rejected = route.routers[1].admitted.as_ref().unwrap();
        assert_eq!(rejected.reason.as_deref(), Some("HostAlreadyClaimed"));
        assert_eq!(
            route.routers[0].canonical_hostname.as_deref(),
            Some("router-default.apps.example.com")
        );
        assert!(route.is_admitted() && route.is_rejected());
        assert_eq!(route.admitted_host(), Some("shop.apps.example.com"));

        // Defaults: kind Service, weight 100, `<all>` ports, no TLS, no status.
        let plain = Route::parse(&route_without_status(json!({"to": {"name": "web"}})));
        assert_eq!(plain.backends[0].kind, "Service");
        assert_eq!(plain.backends[0].weight, DEFAULT_WEIGHT);
        assert_eq!(plain.target_port_label(), "<all>");
        assert!(plain.tls.is_none() && plain.routers.is_empty());
        assert!(!plain.is_admitted() && !plain.is_rejected());
        let numeric = Route::parse(&route_without_status(
            json!({"to": {"name": "web"}, "port": {"targetPort": 8080}}),
        ));
        assert_eq!(numeric.target_port, Some(TargetPort::Number(8080)));
        assert_eq!(numeric.target_port_label(), "8080");
    }

    fn route_without_status(spec: Value) -> Value {
        json!({"apiVersion": "route.openshift.io/v1", "kind": "Route",
            "metadata": {"name": "web", "namespace": "shop"}, "spec": spec})
    }

    #[test]
    fn urls_follow_termination_path_and_wildcards() {
        let url_of = |spec: Value| url(&Route::parse(&route_without_status(spec)));
        assert_eq!(
            url_of(json!({"host": "a.example.com", "to": {"name": "web"}})).as_deref(),
            Some("http://a.example.com")
        );
        for termination in ["edge", "passthrough", "reencrypt"] {
            assert_eq!(
                url_of(
                    json!({"host": "a.example.com", "path": "/api", "to": {"name": "web"},
                    "tls": {"termination": termination}})
                )
                .as_deref(),
                Some("https://a.example.com/api"),
                "{termination}"
            );
        }
        // Wildcards and missing hosts have no link.
        let wildcard = json!({"host": "www.apps.example.com", "wildcardPolicy": "Subdomain",
            "to": {"name": "web"}});
        assert_eq!(url_of(wildcard.clone()), None);
        let parsed = Route::parse(&route_without_status(wildcard));
        assert!(parsed.is_wildcard());
        assert_eq!(parsed.display_host().as_deref(), Some("*.apps.example.com"));
        assert_eq!(
            url_of(json!({"host": "*.apps.example.com", "to": {"name": "web"}})),
            None
        );
        assert_eq!(url_of(json!({"to": {"name": "web"}})), None);
        // A generated host the router reports.
        let generated = route(
            json!({"to": {"name": "web"}}),
            json!({"ingress": [admitted("default", "web-shop.apps.example.com", "True", None)]}),
        );
        assert_eq!(
            url(&Route::parse(&generated)).as_deref(),
            Some("http://web-shop.apps.example.com")
        );
    }

    #[test]
    fn weights_print_like_oc() {
        let label = |spec: Value| services_label(&Route::parse(&route_without_status(spec)));
        assert_eq!(label(json!({"to": {"name": "web"}})), "web");
        assert_eq!(
            label(json!({"to": {"name": "web", "weight": 0}})),
            "web(0%)",
            "a single backend with weight 0 gets nothing"
        );
        assert_eq!(
            label(json!({"to": {"name": "shop-web", "weight": 80},
                "alternateBackends": [{"name": "shop-canary", "weight": 20}]})),
            "shop-web(80%),shop-canary(20%)"
        );
        assert_eq!(
            label(
                json!({"to": {"name": "a"}, "alternateBackends": [{"name": "b"}, {"name": "c"}]})
            ),
            "a(33%),b(33%),c(33%)"
        );
        assert_eq!(
            label(json!({"to": {"name": "a", "weight": 0},
                "alternateBackends": [{"name": "b", "weight": 0}]})),
            "a(0%),b(0%)"
        );
    }

    #[test]
    fn target_ports_resolve_like_the_router() {
        let svc = service(json!([
            {"name": "http", "port": 80, "targetPort": 8080},
            {"name": "https", "port": 443, "targetPort": "tls"},
            {"name": "metrics", "port": 9090}
        ]));
        // A name matches the Service port's name.
        let named = resolve_target_port(Some(&TargetPort::Name("http".into())), &svc).unwrap();
        assert_eq!((named.port, named.name.as_deref()), (80, Some("http")));
        assert_eq!(named.label(), "80/TCP → 8080");
        // A number matches the Service port's targetPort…
        let numeric = resolve_target_port(Some(&TargetPort::Number(8080)), &svc).unwrap();
        assert_eq!(numeric.port, 80);
        // …or the port itself when it has no targetPort.
        let own = resolve_target_port(Some(&TargetPort::Number(9090)), &svc).unwrap();
        assert_eq!((own.port, own.label().as_str()), (9090, "9090/TCP"));
        // The Service port number isn't a target port.
        assert_eq!(
            resolve_target_port(Some(&TargetPort::Number(80)), &svc).unwrap_err(),
            "target port 80 not found on Service shop-web"
        );
        assert_eq!(
            resolve_target_port(Some(&TargetPort::Name("grpc".into())), &svc).unwrap_err(),
            "port grpc not found on Service shop-web"
        );
        // No spec.port: the first port.
        assert_eq!(resolve_target_port(None, &svc).unwrap().port, 80);
        assert_eq!(
            resolve_target_port(None, &service(json!([]))).unwrap_err(),
            "Service shop-web has no ports"
        );
    }

    #[test]
    fn numeric_target_ports_resolve_through_the_endpoints() {
        // `oc expose`-style Services target a container port by name; the Route names the
        // number the pods listen on.
        let svc = json!({"metadata": {"name": "shop-tls"}, "spec": {"ports": [
            {"name": "https", "port": 443, "targetPort": "https"}]}});
        let slices = [
            json!({"metadata": {"labels": {"kubernetes.io/service-name": "shop-tls"}},
                "ports": [{"name": "https", "port": 8443, "protocol": "TCP"}]}),
            json!({"metadata": {"labels": {"kubernetes.io/service-name": "other"}},
                "ports": [{"name": "https", "port": 9443}]}),
        ];
        let endpoints = endpoint_ports(&slices, "shop-tls");
        assert_eq!(endpoints, [("https".to_string(), 8443)]);
        let target = TargetPort::Number(8443);
        assert_eq!(
            resolve_target_port(Some(&target), &svc).unwrap_err(),
            "target port 8443 not found on Service shop-tls"
        );
        let matched = resolve_target_port_with(Some(&target), &svc, &endpoints).unwrap();
        assert_eq!(matched.port, 443);
        assert_eq!(matched.label(), "443/TCP → 8443");
        assert_eq!(
            resolve_target_port_with(Some(&TargetPort::Number(9443)), &svc, &endpoints)
                .unwrap_err(),
            "target port 9443 not found on Service shop-tls"
        );
        // Core Endpoints work too.
        let legacy = [json!({"metadata": {"name": "shop-tls"},
            "subsets": [{"ports": [{"name": "https", "port": 8443}]}]})];
        assert_eq!(endpoint_ports(&legacy, "shop-tls"), endpoints);
    }

    #[test]
    fn several_backends_resolve_against_their_own_service() {
        let route = Route::parse(&route_without_status(json!({
            "to": {"name": "shop-web", "weight": 80},
            "alternateBackends": [{"name": "shop-canary", "weight": 20}],
            "port": {"targetPort": "web"}
        })));
        let services = [
            json!({"metadata": {"name": "shop-web"},
                "spec": {"ports": [{"name": "web", "port": 80, "targetPort": 8080}]}}),
            json!({"metadata": {"name": "shop-canary"},
                "spec": {"ports": [{"name": "admin", "port": 9000},
                                   {"name": "web", "port": 8081, "targetPort": "http"}]}}),
        ];
        let resolved: Vec<(String, u16)> = route
            .services()
            .map(|backend| {
                let service = services
                    .iter()
                    .find(|s| s["metadata"]["name"] == backend.name.as_str())
                    .unwrap();
                let matched = resolve_target_port(route.target_port.as_ref(), service).unwrap();
                (backend.name.clone(), matched.port)
            })
            .collect();
        assert_eq!(
            resolved,
            [
                ("shop-web".to_string(), 80),
                ("shop-canary".to_string(), 8081)
            ]
        );
    }

    #[test]
    fn inline_keys_are_masked_with_the_last_applied_copy() {
        let mut object = route_without_status(json!({"to": {"name": "web"},
            "tls": {"termination": "reencrypt", "key": KEY, "certificate": "CERT"}}));
        object["metadata"]["annotations"] = json!({
            LAST_APPLIED: format!("{{\"spec\":{{\"tls\":{{\"key\":{KEY:?}}}}}}}"),
            "keep": "me"
        });
        assert!(has_inline_key(&object));
        assert_eq!(inline_key(&object), Some(KEY));
        assert!(mask_inline_key(&mut object));
        assert_eq!(object["spec"]["tls"]["key"], MASK);
        assert_eq!(object["spec"]["tls"]["certificate"], "CERT");
        assert_eq!(object["metadata"]["annotations"][LAST_APPLIED], MASK);
        assert_eq!(object["metadata"]["annotations"]["keep"], "me");
        assert!(!object.to_string().contains("MIIEv"));

        // Without a key nothing changes, and other kinds are left alone.
        let mut plain = route_without_status(json!({"to": {"name": "web"},
            "tls": {"termination": "edge"}}));
        assert!(!has_inline_key(&plain));
        assert!(!mask_inline_key(&mut plain));
        let mut knative = json!({"apiVersion": "serving.knative.dev/v1", "kind": "Route",
            "spec": {"tls": {"key": "x"}}});
        assert!(!mask_inline_key(&mut knative));
        assert!(is_route(GROUP, KIND) && !is_route("serving.knative.dev", KIND));
    }
}
