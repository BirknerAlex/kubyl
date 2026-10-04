//! What a web view points at (one port of a Service or Pod), which ports look like web UIs,
//! and the built-in presets for common apps.

use std::fmt;

use kubyl_core::{ClusterId, Gvr, ResourceRef};
use kubyl_portforward::resolve::http_kind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The kind of object a web view forwards to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Service,
    Pod,
}

impl TargetKind {
    pub fn resource(self) -> &'static str {
        match self {
            TargetKind::Service => "services",
            TargetKind::Pod => "pods",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            TargetKind::Service => "svc",
            TargetKind::Pod => "pod",
        }
    }

    pub fn from_resource(resource: &str) -> Option<Self> {
        match resource {
            "services" => Some(TargetKind::Service),
            "pods" => Some(TargetKind::Pod),
            _ => None,
        }
    }
}

/// One port of a Service or Pod in one cluster: what a web view (and its forward) serves.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WebTarget {
    pub cluster: ClusterId,
    pub namespace: String,
    pub kind: TargetKind,
    pub name: String,
    /// The Service port, or the container port of a Pod.
    pub port: u16,
}

impl WebTarget {
    pub fn new(target: &ResourceRef, port: u16) -> Option<Self> {
        Some(Self {
            cluster: target.cluster.clone(),
            namespace: target.namespace.clone()?,
            kind: TargetKind::from_resource(&target.gvr.resource)?,
            name: target.name.clone()?,
            port,
        })
    }

    /// The Service or Pod.
    pub fn object(&self) -> ResourceRef {
        ResourceRef::object(
            self.cluster.clone(),
            Gvr::new("", "v1", self.kind.resource()),
            Some(self.namespace.clone()),
            self.name.clone(),
        )
    }

    /// Same object (any port).
    pub fn same_object(&self, other: &ResourceRef) -> bool {
        self.cluster == other.cluster
            && other.gvr.group.is_empty()
            && other.gvr.resource == self.kind.resource()
            && other.namespace.as_deref() == Some(self.namespace.as_str())
            && other.name.as_deref() == Some(self.name.as_str())
    }
}

impl fmt::Display for WebTarget {
    /// `svc/grafana:80`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}:{}", self.kind.short(), self.name, self.port)
    }
}

/// `http` or `https`: how the page talks to the service (the forward itself is plain TCP).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    #[default]
    Http,
    Https,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// A TCP port of an object, with what makes it look like a web UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebPort {
    pub port: u16,
    pub name: Option<String>,
    pub app_protocol: Option<String>,
    /// Service ports: the target port (`3000`, `http`).
    pub target_port: Option<String>,
    /// Looks like HTTP(S).
    pub web: bool,
    pub scheme: Scheme,
    /// Why it looks like HTTP (`appProtocol http`, `port name web`, `port 9090`).
    pub reason: Option<String>,
}

impl WebPort {
    fn new(
        port: u16,
        name: Option<String>,
        app_protocol: Option<String>,
        target: Option<String>,
    ) -> Self {
        let (web, https) = http_kind(port, name.as_deref(), app_protocol.as_deref());
        let reason = if !web {
            None
        } else if let Some(app) = &app_protocol {
            Some(format!("appProtocol {app}"))
        } else if let Some(name) = &name {
            Some(format!("port name {name}"))
        } else {
            Some(format!("port {port}"))
        };
        Self {
            port,
            name,
            app_protocol,
            target_port: target,
            web,
            scheme: if https { Scheme::Https } else { Scheme::Http },
            reason,
        }
    }

    /// `http-web · 80 → 3000/TCP`.
    pub fn label(&self) -> String {
        let mut label = String::new();
        if let Some(name) = &self.name {
            label.push_str(name);
            label.push_str(" · ");
        }
        label.push_str(&self.port.to_string());
        if let Some(target) = &self.target_port
            && target != &self.port.to_string()
        {
            label.push_str(" → ");
            label.push_str(target);
        }
        label.push_str("/TCP");
        label
    }
}

fn str_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// The TCP ports of a Service (`spec.ports`) or Pod (container ports), in order.
pub fn ports_of(kind: TargetKind, object: &Value) -> Vec<WebPort> {
    let tcp = |p: &Value| str_at(p, "/protocol").unwrap_or("TCP") == "TCP";
    let port_of = |p: &Value, key: &str| {
        p.get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok())
    };
    let name_of = |p: &Value| str_at(p, "/name").map(String::from);
    match kind {
        TargetKind::Service => object
            .pointer("/spec/ports")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|p| tcp(p))
            .filter_map(|p| {
                let target = match p.get("targetPort") {
                    Some(Value::Number(n)) => Some(n.to_string()),
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => None,
                };
                Some(WebPort::new(
                    port_of(p, "port")?,
                    name_of(p),
                    str_at(p, "/appProtocol").map(String::from),
                    target,
                ))
            })
            .collect(),
        TargetKind::Pod => {
            let mut ports: Vec<WebPort> = Vec::new();
            for container in object
                .pointer("/spec/containers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for p in container
                    .get("ports")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|p| tcp(p))
                {
                    let Some(number) = port_of(p, "containerPort") else {
                        continue;
                    };
                    if ports.iter().any(|existing| existing.port == number) {
                        continue;
                    }
                    ports.push(WebPort::new(number, name_of(p), None, None));
                }
            }
            ports
        }
    }
}

/// A backend of an Ingress: which Service port serves which host and path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngressBackend {
    pub host: Option<String>,
    pub path: String,
    pub service: String,
    /// The port number, or its name (resolved against the Service).
    pub port: BackendPort,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendPort {
    Number(u16),
    Name(String),
}

/// The Service backends of an Ingress (`networking.k8s.io/v1`), rules first, then the default.
pub fn ingress_backends(object: &Value) -> Vec<IngressBackend> {
    let backend = |b: &Value, host: Option<String>, path: String| {
        let service = b.pointer("/service")?;
        let name = str_at(service, "/name")?.to_string();
        let port = match service.pointer("/port/number").and_then(Value::as_u64) {
            Some(n) => BackendPort::Number(u16::try_from(n).ok()?),
            None => BackendPort::Name(str_at(service, "/port/name")?.to_string()),
        };
        Some(IngressBackend {
            host,
            path,
            service: name,
            port,
        })
    };
    let mut backends = Vec::new();
    for rule in object
        .pointer("/spec/rules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let host = str_at(rule, "/host").map(String::from);
        for path in rule
            .pointer("/http/paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let prefix = str_at(path, "/path").unwrap_or("/").to_string();
            if let Some(b) = path
                .get("backend")
                .and_then(|b| backend(b, host.clone(), prefix))
                && !backends.contains(&b)
            {
                backends.push(b);
            }
        }
    }
    if let Some(b) = object
        .pointer("/spec/defaultBackend")
        .and_then(|b| backend(b, None, "/".into()))
    {
        backends.push(b);
    }
    backends
}

/// A built-in preset for a well-known app: where to start and how to talk to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// Matched against the object's name and its `app.kubernetes.io/name` / `app` labels.
    names: &'static [&'static str],
    /// Only these ports (empty: any web port).
    ports: &'static [u16],
    /// Names that look similar but aren't the app (exporters, operators).
    not: &'static [&'static str],
    pub start_path: &'static str,
    pub scheme: Option<Scheme>,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "grafana",
        name: "Grafana",
        names: &["grafana"],
        ports: &[],
        not: &["operator", "agent", "alloy"],
        start_path: "/",
        scheme: None,
    },
    Preset {
        id: "alertmanager",
        name: "Alertmanager",
        names: &["alertmanager"],
        ports: &[9093],
        not: &["operator"],
        start_path: "/#/alerts",
        scheme: None,
    },
    Preset {
        id: "prometheus",
        name: "Prometheus",
        names: &["prometheus"],
        ports: &[9090],
        not: &[
            "operator",
            "node-exporter",
            "adapter",
            "pushgateway",
            "blackbox",
        ],
        start_path: "/graph",
        scheme: None,
    },
    Preset {
        id: "argocd",
        name: "Argo CD",
        names: &["argocd-server"],
        ports: &[],
        not: &["metrics"],
        start_path: "/",
        scheme: None,
    },
    Preset {
        id: "rabbitmq",
        name: "RabbitMQ",
        names: &["rabbitmq"],
        ports: &[15672, 15671],
        not: &["operator"],
        start_path: "/",
        scheme: None,
    },
    Preset {
        id: "kibana",
        name: "Kibana",
        names: &["kibana"],
        ports: &[5601],
        not: &[],
        start_path: "/app/home",
        scheme: None,
    },
    Preset {
        id: "kafka-ui",
        name: "Kafka UI",
        names: &["kafka-ui", "kafdrop", "akhq"],
        ports: &[],
        not: &[],
        start_path: "/",
        scheme: None,
    },
    Preset {
        id: "jaeger",
        name: "Jaeger",
        names: &["jaeger"],
        ports: &[16686],
        not: &["operator", "collector", "agent"],
        start_path: "/search",
        scheme: None,
    },
];

/// The preset for `name` (object name or app label) on `port`, if one fits.
pub fn preset_for(names: &[&str], port: u16) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| {
        (preset.ports.is_empty() || preset.ports.contains(&port))
            && names.iter().any(|name| {
                let name = name.to_ascii_lowercase();
                preset.names.iter().any(|n| name.contains(n))
                    && !preset.not.iter().any(|n| name.contains(n))
            })
    })
}

/// The names a preset is matched against: the object's name and its app labels.
pub fn preset_names(object: &Value) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = str_at(object, "/metadata/name") {
        names.push(name.to_string());
    }
    for label in ["app.kubernetes.io/name", "app"] {
        if let Some(value) = object
            .pointer("/metadata/labels")
            .and_then(|l| l.get(label))
            .and_then(Value::as_str)
        {
            names.push(value.to_string());
        }
    }
    names
}

/// Query parameters that may carry credentials (tokens in Jupyter-style links, OAuth codes):
/// never remembered.
const SECRET_PARAMS: &[&str] = &[
    "token",
    "access_token",
    "id_token",
    "refresh_token",
    "code",
    "state",
    "auth",
    "authorization",
    "key",
    "apikey",
    "api_key",
    "password",
    "passwd",
    "secret",
    "session",
    "sessionid",
    "jwt",
    "sig",
    "signature",
    "x-amz-signature",
];

/// The path and query of `url` for remembering it: no origin (the port changes), no fragment
/// (OAuth implicit flows put tokens there) and no parameters that look like credentials.
pub fn remembered_path(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let mut path = url.path().to_string();
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            // `session_id`, `X-Amz-Signature`, `apiKey`: any word of the name counts.
            let mut words = name.split(|c: char| !c.is_ascii_alphanumeric());
            !SECRET_PARAMS
                .iter()
                .any(|secret| name == *secret || name.ends_with(secret))
                && !words.any(|word| SECRET_PARAMS.contains(&word))
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if !kept.is_empty() {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(kept)
            .finish();
        path.push('?');
        path.push_str(&query);
    }
    Some(path)
}

/// Joins an origin (`http://127.0.0.1:52871`) and a path the user typed or Kubyl remembered.
pub fn join(origin: &str, path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return format!("{origin}/");
    }
    if path.starts_with('/') {
        format!("{origin}{path}")
    } else {
        format!("{origin}/{path}")
    }
}

/// Whether `url` is on `origin` (scheme, host and port).
pub fn same_origin(url: &str, origin: &str) -> bool {
    let (Ok(url), Ok(origin)) = (url::Url::parse(url), url::Url::parse(origin)) else {
        return false;
    };
    url.origin() == origin.origin()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn service_ports_keep_names_targets_and_detection() {
        let service = json!({"spec": {"ports": [
            {"name": "http-web", "port": 80, "targetPort": 3000, "appProtocol": "http"},
            {"name": "grpc", "port": 3001, "targetPort": "grpc"},
            {"name": "dns", "port": 53, "protocol": "UDP"},
            {"port": 9090}
        ]}});
        let ports = ports_of(TargetKind::Service, &service);
        assert_eq!(ports.len(), 3);
        assert!(ports[0].web);
        assert_eq!(ports[0].label(), "http-web · 80 → 3000/TCP");
        assert_eq!(ports[0].reason.as_deref(), Some("appProtocol http"));
        assert!(!ports[1].web);
        assert_eq!(ports[1].label(), "grpc · 3001 → grpc/TCP");
        assert!(ports[2].web);
        assert_eq!(ports[2].reason.as_deref(), Some("port 9090"));
    }

    #[test]
    fn https_ports_default_to_https() {
        let service =
            json!({"spec": {"ports": [{"name": "https", "port": 443, "targetPort": 8080}]}});
        assert_eq!(
            ports_of(TargetKind::Service, &service)[0].scheme,
            Scheme::Https
        );
    }

    #[test]
    fn pod_ports_are_deduplicated_across_containers() {
        let pod = json!({"spec": {"containers": [
            {"ports": [{"name": "http", "containerPort": 8080}]},
            {"ports": [{"containerPort": 8080}, {"name": "metrics", "containerPort": 9100}]}
        ]}});
        let ports = ports_of(TargetKind::Pod, &pod);
        assert_eq!(
            ports.iter().map(|p| p.port).collect::<Vec<_>>(),
            [8080, 9100]
        );
        assert_eq!(ports[0].label(), "http · 8080/TCP");
    }

    #[test]
    fn ingress_backends_come_from_rules_and_default() {
        let ingress = json!({"spec": {
            "defaultBackend": {"service": {"name": "fallback", "port": {"number": 80}}},
            "rules": [{"host": "grafana.example.com", "http": {"paths": [
                {"path": "/", "backend": {"service": {"name": "grafana", "port": {"name": "http"}}}},
                {"path": "/api", "backend": {"service": {"name": "api", "port": {"number": 8080}}}}
            ]}}]
        }});
        let backends = ingress_backends(&ingress);
        assert_eq!(backends.len(), 3);
        assert_eq!(backends[0].service, "grafana");
        assert_eq!(backends[0].port, BackendPort::Name("http".into()));
        assert_eq!(backends[0].host.as_deref(), Some("grafana.example.com"));
        assert_eq!(backends[1].path, "/api");
        assert_eq!(backends[2].service, "fallback");
        assert_eq!(backends[2].host, None);
    }

    #[test]
    fn presets_match_names_and_ports() {
        let names = ["kube-prometheus-stack-grafana"];
        assert_eq!(preset_for(&names, 80).unwrap().id, "grafana");
        assert_eq!(
            preset_for(&["prometheus-k8s"], 9090).unwrap().start_path,
            "/graph"
        );
        assert!(preset_for(&["prometheus-k8s"], 8080).is_none());
        assert!(preset_for(&["kube-prometheus-stack-operator"], 9090).is_none());
        assert!(preset_for(&["prometheus-node-exporter"], 9090).is_none());
        assert_eq!(
            preset_for(&["alertmanager-main"], 9093).unwrap().start_path,
            "/#/alerts"
        );
        assert!(preset_for(&["ledger"], 80).is_none());
    }

    #[test]
    fn remembered_paths_drop_origin_fragment_and_credentials() {
        assert_eq!(
            remembered_path(
                "http://127.0.0.1:52871/d/k8s-pods/pods?orgId=1&var-namespace=payments"
            )
            .as_deref(),
            Some("/d/k8s-pods/pods?orgId=1&var-namespace=payments")
        );
        assert_eq!(
            remembered_path("http://127.0.0.1:1/lab?token=abc123&x=1#access_token=zzz").as_deref(),
            Some("/lab?x=1")
        );
        assert_eq!(
            remembered_path("http://127.0.0.1:1/cb?code=1&state=2&session_id=3").as_deref(),
            Some("/cb")
        );
        assert_eq!(remembered_path("about:blank"), None);
    }

    #[test]
    fn joins_and_compares_origins() {
        let origin = "http://127.0.0.1:52871";
        assert_eq!(join(origin, "/graph"), "http://127.0.0.1:52871/graph");
        assert_eq!(join(origin, "graph"), "http://127.0.0.1:52871/graph");
        assert_eq!(join(origin, ""), "http://127.0.0.1:52871/");
        assert_eq!(join(origin, "#/alerts"), "http://127.0.0.1:52871/#/alerts");
        assert!(same_origin("http://127.0.0.1:52871/x?y", origin));
        assert!(!same_origin("http://127.0.0.1:52872/x", origin));
        assert!(!same_origin("https://grafana.example.com/login", origin));
    }

    #[test]
    fn targets_label_like_the_mockup() {
        let target = WebTarget {
            cluster: ClusterId::new("c"),
            namespace: "monitoring".into(),
            kind: TargetKind::Service,
            name: "grafana".into(),
            port: 80,
        };
        assert_eq!(target.to_string(), "svc/grafana:80");
        assert!(target.same_object(&target.object()));
    }
}
