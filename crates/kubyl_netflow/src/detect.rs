//! Which flow backends a cluster has (README "Flow providers"), in a fixed order: Hubble Relay,
//! Calico Whisker, NetObserv (the CNI's own API first, the CNI-independent one last). Runs on
//! Tokio with the cluster's client. Reads Services, ConfigMaps, DaemonSets and the
//! FlowCollector; never Secrets. What it checked is kept for the empty state, with the CNI it
//! recognized so the hint can name what to install.

use std::path::PathBuf;

use k8s_openapi::api::apps::v1::DaemonSet;
use k8s_openapi::api::core::v1::{ConfigMap, Service};
use kube::api::{Api, ListParams};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use serde_json::Value;

use crate::provider::{BackendKind, ProviderError};
use crate::settings::{ClusterNetflowSettings, HubbleSettings};

/// What discovery says the cluster serves, gathered on the UI thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Served {
    /// `flows.netobserv.io` `flowcollectors`, its preferred version.
    pub flow_collectors: Option<String>,
    pub cilium: bool,
    pub calico: bool,
    pub ovn: bool,
    pub antrea: bool,
}

impl Served {
    pub fn from_discovery(discovery: &kubyl_kube::discovery::Discovery) -> Self {
        let mut served = Served::default();
        for resource in discovery.preferred() {
            let group = resource.gvr.group.as_str();
            match (group, resource.gvr.resource.as_str()) {
                ("flows.netobserv.io", "flowcollectors") => {
                    served.flow_collectors = Some(resource.gvr.version.clone());
                }
                ("cilium.io", _) => served.cilium = true,
                ("crd.projectcalico.org" | "projectcalico.org", _) => served.calico = true,
                ("k8s.ovn.org", _) => served.ovn = true,
                (g, _) if g.ends_with("antrea.io") => served.antrea = true,
                _ => {}
            }
        }
        served
    }
}

/// A Service reached through the API server's service proxy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceTarget {
    pub namespace: String,
    pub service: String,
    pub port: String,
    pub scheme: String,
    pub path: String,
}

impl ServiceTarget {
    pub fn label(&self) -> String {
        format!("{}/{}", self.namespace, self.service)
    }
}

/// How Hubble Relay talks.
#[derive(Clone, PartialEq)]
pub enum RelayTls {
    Plain,
    /// Server TLS: the CA (PEM) if one was found, where it came from or should come from, and
    /// the name to verify.
    Server {
        ca: Option<Vec<u8>>,
        ca_source: String,
        server_name: String,
    },
    /// Relay wants client certificates: not supported.
    Mutual,
}

impl std::fmt::Debug for RelayTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayTls::Plain => f.write_str("Plain"),
            RelayTls::Server { ca, ca_source, .. } => write!(
                f,
                "Server {{ ca: {}, ca_source: {ca_source} }}",
                ca.is_some()
            ),
            RelayTls::Mutual => f.write_str("Mutual"),
        }
    }
}

/// Where NetObserv keeps flows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LokiTarget {
    Service(ServiceTarget),
    /// A URL the user set in settings; the desktop requests it directly.
    Url(String),
    /// A URL only the FlowCollector names (outside the cluster, an IP or localhost). The cluster
    /// doesn't get to pick what the desktop connects to, so it is shown, not requested.
    External(String),
    /// A LokiStack gateway: needs the user's token, which the service proxy doesn't forward.
    LokiStack {
        namespace: String,
        name: String,
    },
    /// Loki is turned off in the FlowCollector.
    Disabled,
}

/// Where NetObserv's metrics are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromTarget {
    Service(ServiceTarget),
    /// Phase 07's Prometheus (`MetricsService::prometheus`).
    Cluster,
}

/// A backend found.
#[derive(Clone, Debug, PartialEq)]
pub enum Candidate {
    Hubble {
        namespace: String,
        service: String,
        port: u16,
        tls: RelayTls,
    },
    Whisker(ServiceTarget),
    NetObserv {
        loki: LokiTarget,
        prometheus: PromTarget,
    },
}

impl Candidate {
    pub fn kind(&self) -> BackendKind {
        match self {
            Candidate::Hubble { .. } => BackendKind::Hubble,
            Candidate::Whisker(_) => BackendKind::Whisker,
            Candidate::NetObserv { .. } => BackendKind::NetObserv,
        }
    }

    /// `kube-system/hubble-relay`, `calico-system/whisker`, `FlowCollector cluster`.
    pub fn label(&self) -> String {
        match self {
            Candidate::Hubble {
                namespace, service, ..
            } => format!("{namespace}/{service}"),
            Candidate::Whisker(target) => target.label(),
            Candidate::NetObserv { .. } => "FlowCollector cluster".into(),
        }
    }
}

/// One thing detection looked at, for the empty state.
#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub backend: BackendKind,
    pub found: bool,
    /// What was found or not (`no hubble-relay Service in kube-system or cilium`).
    pub text: String,
    /// A read that was denied.
    pub forbidden: Option<ProviderError>,
}

/// The cluster's CNI, as far as Kubyl can tell (for the empty state's hint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cni {
    Cilium,
    Calico,
    OvnKubernetes,
    Flannel,
    Kindnet,
    AwsVpc,
    Azure,
    Antrea,
    Weave,
    Canal,
}

impl Cni {
    pub fn label(&self) -> &'static str {
        match self {
            Cni::Cilium => "Cilium",
            Cni::Calico => "Calico",
            Cni::OvnKubernetes => "OVN-Kubernetes",
            Cni::Flannel => "flannel",
            Cni::Kindnet => "kindnet",
            Cni::AwsVpc => "the Amazon VPC CNI",
            Cni::Azure => "Azure CNI",
            Cni::Antrea => "Antrea",
            Cni::Weave => "Weave Net",
            Cni::Canal => "Canal",
        }
    }
}

/// What detection found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detection {
    /// In priority order.
    pub candidates: Vec<Candidate>,
    pub checks: Vec<Check>,
    pub cni: Option<Cni>,
}

impl Detection {
    pub fn find(&self, kind: BackendKind) -> Option<&Candidate> {
        self.candidates.iter().find(|c| c.kind() == kind)
    }
}

/// Detection's inputs.
#[derive(Clone)]
pub struct Inputs {
    pub client: kube::Client,
    pub served: Served,
    /// `/version`'s `gitVersion` (`+k3s` means flannel).
    pub git_version: String,
    pub settings: ClusterNetflowSettings,
}

/// Looks for every backend.
pub async fn detect(inputs: Inputs) -> Detection {
    let mut detection = Detection::default();
    let (hubble, whisker, netobserv, cni) = futures::join!(
        hubble(&inputs),
        whisker(&inputs),
        netobserv(&inputs),
        cni(&inputs)
    );
    for (candidate, check) in [hubble, whisker, netobserv] {
        if let Some(candidate) = candidate {
            detection.candidates.push(candidate);
        }
        detection.checks.push(check);
    }
    detection.cni = cni;
    detection
}

fn check(backend: BackendKind, found: bool, text: impl Into<String>) -> Check {
    Check {
        backend,
        found,
        text: text.into(),
        forbidden: None,
    }
}

fn denied(backend: BackendKind, err: ProviderError) -> Check {
    Check {
        backend,
        found: false,
        text: err
            .permission()
            .map(|p| format!("403: needs {p}"))
            .unwrap_or_else(|| err.to_string()),
        forbidden: Some(err),
    }
}

async fn get_service(
    client: &kube::Client,
    namespace: &str,
    name: &str,
) -> Result<Option<Service>, ProviderError> {
    let api: Api<Service> = Api::namespaced(client.clone(), namespace);
    api.get_opt(name)
        .await
        .map_err(|e| ProviderError::from_kube(&e, "get", "services", Some(namespace)))
}

async fn get_config_map(
    client: &kube::Client,
    namespace: &str,
    name: &str,
) -> Result<Option<ConfigMap>, ProviderError> {
    let api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
    api.get_opt(name)
        .await
        .map_err(|e| ProviderError::from_kube(&e, "get", "configmaps", Some(namespace)))
}

/// The Service port to use: a named one, else a preferred number, else the first.
fn service_port(service: &Service, name: &str, preferred: &[i32]) -> Option<u16> {
    let ports = service.spec.as_ref()?.ports.as_ref()?;
    ports
        .iter()
        .find(|p| p.name.as_deref() == Some(name))
        .or_else(|| ports.iter().find(|p| preferred.contains(&p.port)))
        .or_else(|| ports.first())
        .and_then(|p| u16::try_from(p.port).ok())
}

async fn hubble(inputs: &Inputs) -> (Option<Candidate>, Check) {
    let kind = BackendKind::Hubble;
    let settings = inputs.settings.hubble.clone().unwrap_or_default();
    let client = &inputs.client;
    let named = settings.namespace.is_some() || settings.service.is_some();
    let places: Vec<(String, String)> = if named {
        vec![(
            settings
                .namespace
                .clone()
                .unwrap_or_else(|| "kube-system".into()),
            settings
                .service
                .clone()
                .unwrap_or_else(|| "hubble-relay".into()),
        )]
    } else {
        vec![
            ("kube-system".into(), "hubble-relay".into()),
            ("cilium".into(), "hubble-relay".into()),
        ]
    };
    let mut found: Option<Service> = None;
    let mut forbidden = None;
    for (namespace, name) in &places {
        match get_service(client, namespace, name).await {
            Ok(Some(service)) => {
                found = Some(service);
                break;
            }
            Ok(None) => {}
            Err(err @ ProviderError::Forbidden { .. }) => forbidden = forbidden.or(Some(err)),
            Err(err) => return (None, check(kind, false, err.to_string())),
        }
    }
    if found.is_none() && !named {
        // Anywhere, by label.
        let api: Api<Service> = Api::all(client.clone());
        match api
            .list(&ListParams::default().labels("k8s-app=hubble-relay"))
            .await
        {
            Ok(list) => found = list.items.into_iter().next(),
            Err(err) => match ProviderError::from_kube(&err, "list", "services", None) {
                // Listing every namespace is often denied: the known places decide.
                ProviderError::Forbidden { .. } => {}
                other if forbidden.is_none() => {
                    return (None, check(kind, false, other.to_string()));
                }
                _ => {}
            },
        }
    }
    let Some(service) = found else {
        if let Some(err) = forbidden {
            return (None, denied(kind, err));
        }
        let text = if named {
            format!(
                "no {}/{} Service (named in settings)",
                places[0].0, places[0].1
            )
        } else {
            "no hubble-relay Service in kube-system or cilium, none labelled k8s-app=hubble-relay"
                .into()
        };
        return (None, check(kind, false, text));
    };
    let namespace = service.metadata.namespace.clone().unwrap_or_default();
    let name = service.metadata.name.clone().unwrap_or_default();
    let port = settings
        .port
        .or_else(|| service_port(&service, "grpc", &[80, 443]))
        .unwrap_or(80);
    let tls = relay_tls(client, &namespace, port, &settings).await;
    let text = match &tls {
        RelayTls::Plain => format!("{namespace}/{name} port {port}, plain gRPC"),
        RelayTls::Server {
            ca: Some(_),
            ca_source,
            ..
        } => {
            format!("{namespace}/{name} port {port}, TLS (CA from {ca_source})")
        }
        RelayTls::Server {
            ca: None,
            ca_source,
            ..
        } => {
            format!("{namespace}/{name} port {port}, TLS without a CA ({ca_source})")
        }
        RelayTls::Mutual => format!("{namespace}/{name} port {port}, wants client certificates"),
    };
    (
        Some(Candidate::Hubble {
            namespace,
            service: name,
            port,
            tls,
        }),
        check(kind, true, text),
    )
}

/// Relay's TLS setup from settings, else its `hubble-relay-config` ConfigMap, else the port.
async fn relay_tls(
    client: &kube::Client,
    namespace: &str,
    port: u16,
    settings: &HubbleSettings,
) -> RelayTls {
    let (tls, mutual) = match settings.tls {
        Some(tls) => (tls, false),
        None => match get_config_map(client, namespace, "hubble-relay-config").await {
            Ok(Some(map)) => {
                let config = map
                    .data
                    .as_ref()
                    .and_then(|d| d.get("config.yaml"))
                    .cloned()
                    .unwrap_or_default();
                parse_relay_config(&config)
            }
            _ => (port == 443, false),
        },
    };
    if mutual {
        return RelayTls::Mutual;
    }
    if !tls {
        return RelayTls::Plain;
    }
    let server_name = settings
        .server_name
        .clone()
        .unwrap_or_else(|| crate::backends::hubble::RELAY_SERVER_NAME.into());
    if let Some(file) = &settings.ca_file {
        return match read_file(file.clone()).await {
            Ok(ca) => RelayTls::Server {
                ca: Some(ca),
                ca_source: format!("the file {}", file.display()),
                server_name,
            },
            Err(err) => RelayTls::Server {
                ca: None,
                ca_source: format!("the file {} can't be read: {err}", file.display()),
                server_name,
            },
        };
    }
    let (map_namespace, map_name) = match settings.ca_config_map.as_deref() {
        Some(named) => match named.split_once('/') {
            Some((ns, name)) => (ns.to_string(), name.to_string()),
            None => (namespace.to_string(), named.to_string()),
        },
        None => (namespace.to_string(), "cilium-root-ca.crt".to_string()),
    };
    let key = settings.ca_key.clone().unwrap_or_else(|| "ca.crt".into());
    let source = format!("the ConfigMap {map_namespace}/{map_name}");
    match get_config_map(client, &map_namespace, &map_name).await {
        Ok(Some(map)) => match map.data.as_ref().and_then(|d| d.get(&key)) {
            Some(pem) if pem.contains("BEGIN CERTIFICATE") => RelayTls::Server {
                ca: Some(pem.clone().into_bytes()),
                ca_source: source,
                server_name,
            },
            _ => RelayTls::Server {
                ca: None,
                ca_source: format!("{source} has no {key}"),
                server_name,
            },
        },
        Ok(None) => RelayTls::Server {
            ca: None,
            ca_source: format!("no {source}"),
            server_name,
        },
        Err(err) => RelayTls::Server {
            ca: None,
            ca_source: format!(
                "{source}: {}",
                err.permission()
                    .map(|p| format!("403, needs {p}"))
                    .unwrap_or_else(|| err.to_string())
            ),
            server_name,
        },
    }
}

async fn read_file(path: PathBuf) -> Result<Vec<u8>, String> {
    tokio::fs::read(&path).await.map_err(|e| e.to_string())
}

/// `(server TLS, client certificates required)` from Relay's `config.yaml`.
pub fn parse_relay_config(config: &str) -> (bool, bool) {
    let value = |key: &str| {
        config.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
        })
    };
    let disabled = value("disable-server-tls").is_some_and(|v| v == "true");
    let has_cert = value("tls-relay-server-cert-file").is_some_and(|v| !v.is_empty());
    let client_ca = value("tls-relay-client-ca-files").is_some_and(|v| !v.is_empty());
    let tls = !disabled && has_cert;
    (tls, tls && client_ca)
}

async fn whisker(inputs: &Inputs) -> (Option<Candidate>, Check) {
    let kind = BackendKind::Whisker;
    let settings = inputs.settings.whisker.clone().unwrap_or_default();
    let namespace = settings.namespace.unwrap_or_else(|| "calico-system".into());
    let name = settings.service.unwrap_or_else(|| "whisker".into());
    match get_service(&inputs.client, &namespace, &name).await {
        Ok(Some(service)) => {
            let port = settings
                .port
                .or_else(|| service_port(&service, "whisker", &[8081]))
                .unwrap_or(8081);
            (
                Some(Candidate::Whisker(ServiceTarget {
                    namespace: namespace.clone(),
                    service: name.clone(),
                    port: port.to_string(),
                    scheme: "http".into(),
                    path: String::new(),
                })),
                check(kind, true, format!("{namespace}/{name} port {port}")),
            )
        }
        Ok(None) => (
            None,
            check(kind, false, format!("no {namespace}/{name} Service")),
        ),
        Err(err @ ProviderError::Forbidden { .. }) => (None, denied(kind, err)),
        Err(err) => (None, check(kind, false, err.to_string())),
    }
}

/// `http://loki.netobserv.svc.cluster.local.:3100/` → a Service target; so are cluster-only
/// short names, `loki` (in `namespace`, the FlowCollector's) and `loki.netobserv`. Other hosts
/// stay URLs.
pub fn in_cluster_url(url: &str, namespace: &str) -> Option<ServiceTarget> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let rest = host
        .strip_suffix(".svc.cluster.local")
        .or_else(|| host.strip_suffix(".svc"))
        .unwrap_or(host);
    let (service, namespace) = rest.split_once('.').unwrap_or((rest, namespace));
    if namespace.contains('.') || service.is_empty() || namespace.is_empty() {
        return None;
    }
    let scheme = parsed.scheme().to_string();
    let port = parsed
        .port()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    Some(ServiceTarget {
        namespace: namespace.into(),
        service: service.into(),
        port: port.to_string(),
        scheme,
        path: parsed.path().trim_end_matches('/').to_string(),
    })
}

async fn netobserv(inputs: &Inputs) -> (Option<Candidate>, Check) {
    let kind = BackendKind::NetObserv;
    let Some(version) = inputs.served.flow_collectors.clone() else {
        return (
            None,
            check(
                kind,
                false,
                "no FlowCollector (flows.netobserv.io isn't served)",
            ),
        );
    };
    let resource = ApiResource::from_gvk_with_plural(
        &GroupVersionKind::gvk("flows.netobserv.io", &version, "FlowCollector"),
        "flowcollectors",
    );
    let api: Api<DynamicObject> = Api::all_with(inputs.client.clone(), &resource);
    let collector = match api.get_opt("cluster").await {
        Ok(Some(collector)) => collector,
        Ok(None) => return (None, check(kind, false, "no FlowCollector named cluster")),
        Err(err) => {
            let err =
                ProviderError::from_kube(&err, "get", "flowcollectors.flows.netobserv.io", None);
            return match err {
                ProviderError::Forbidden { .. } => (None, denied(kind, err)),
                err => (None, check(kind, false, err.to_string())),
            };
        }
    };
    let spec = collector.data.get("spec").cloned().unwrap_or(Value::Null);
    let loki = match &inputs.settings.loki {
        Some(settings) => match (&settings.url, &settings.service) {
            (Some(url), _) => LokiTarget::Url(url.clone()),
            (None, Some(service)) => LokiTarget::Service(ServiceTarget {
                namespace: settings
                    .namespace
                    .clone()
                    .unwrap_or_else(|| "netobserv".into()),
                service: service.clone(),
                port: settings.port.clone().unwrap_or_else(|| "3100".into()),
                scheme: settings.scheme.clone().unwrap_or_else(|| "http".into()),
                path: settings.path.clone().unwrap_or_default(),
            }),
            (None, None) => loki_of(&spec),
        },
        None => loki_of(&spec),
    };
    let prometheus = spec
        .pointer("/prometheus/querier")
        .filter(|q| q.get("mode").and_then(Value::as_str) == Some("Manual"))
        .and_then(|q| q.pointer("/manual/url"))
        .and_then(Value::as_str)
        .and_then(|url| {
            let namespace = spec
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or("netobserv");
            in_cluster_url(url, namespace)
        })
        .map_or(PromTarget::Cluster, PromTarget::Service);
    let text = match &loki {
        LokiTarget::Service(target) => format!("FlowCollector cluster, Loki {}", target.label()),
        LokiTarget::Url(url) | LokiTarget::External(url) => {
            format!("FlowCollector cluster, Loki {url}")
        }
        LokiTarget::LokiStack { namespace, name } => {
            format!("FlowCollector cluster, LokiStack {namespace}/{name} (metrics only)")
        }
        LokiTarget::Disabled => "FlowCollector cluster, Loki off (metrics only)".into(),
    };
    (
        Some(Candidate::NetObserv { loki, prometheus }),
        check(kind, true, text),
    )
}

/// Where the FlowCollector's Loki is.
pub fn loki_of(spec: &Value) -> LokiTarget {
    let loki = spec.get("loki").cloned().unwrap_or(Value::Null);
    // Where NetObserv runs, for short Service names.
    let namespace = spec
        .get("namespace")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .unwrap_or("netobserv")
        .to_string();
    if loki.get("enable").and_then(Value::as_bool) == Some(false) {
        return LokiTarget::Disabled;
    }
    let url = |pointer: &str| {
        loki.pointer(pointer)
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
            .map(|u| {
                in_cluster_url(u, &namespace)
                    .map_or_else(|| LokiTarget::External(u.to_string()), LokiTarget::Service)
            })
    };
    match loki
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("Monolithic")
    {
        "LokiStack" => LokiTarget::LokiStack {
            namespace: loki
                .pointer("/lokiStack/namespace")
                .and_then(Value::as_str)
                .unwrap_or("netobserv")
                .into(),
            name: loki
                .pointer("/lokiStack/name")
                .and_then(Value::as_str)
                .unwrap_or("loki")
                .into(),
        },
        "Microservices" => url("/microservices/querierUrl").unwrap_or(LokiTarget::Disabled),
        "Manual" => url("/manual/querierUrl").unwrap_or(LokiTarget::Disabled),
        _ => url("/monolithic/url").unwrap_or_else(|| {
            LokiTarget::Service(ServiceTarget {
                namespace: namespace.clone(),
                service: "loki".into(),
                port: "3100".into(),
                scheme: "http".into(),
                path: String::new(),
            })
        }),
    }
}

async fn cni(inputs: &Inputs) -> Option<Cni> {
    let served = &inputs.served;
    if served.cilium {
        return Some(Cni::Cilium);
    }
    if served.calico {
        return Some(Cni::Calico);
    }
    if served.ovn {
        return Some(Cni::OvnKubernetes);
    }
    if served.antrea {
        return Some(Cni::Antrea);
    }
    if inputs.git_version.contains("+k3s") {
        return Some(Cni::Flannel);
    }
    for namespace in ["kube-system", "kube-flannel"] {
        let api: Api<DaemonSet> = Api::namespaced(inputs.client.clone(), namespace);
        let Ok(list) = api.list_metadata(&ListParams::default()).await else {
            continue;
        };
        for set in list.items {
            let name = set.metadata.name.unwrap_or_default();
            let cni = match name.as_str() {
                n if n.starts_with("kindnet") => Cni::Kindnet,
                n if n.contains("flannel") => Cni::Flannel,
                "aws-node" => Cni::AwsVpc,
                n if n.starts_with("azure-cn") => Cni::Azure,
                "weave-net" => Cni::Weave,
                "canal" => Cni::Canal,
                _ => continue,
            };
            return Some(cni);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_config() {
        let plain = "cluster-name: default\nlisten-address: :4245\ntls-hubble-client-cert-file: /x\n\ndisable-server-tls: true\n";
        assert_eq!(parse_relay_config(plain), (false, false));
        let tls = "listen-address: :4245\ntls-relay-server-cert-file: /var/lib/hubble-relay/tls/server.crt\ntls-relay-server-key-file: /k\n";
        assert_eq!(parse_relay_config(tls), (true, false));
        let mutual =
            format!("{tls}tls-relay-client-ca-files: /var/lib/hubble-relay/tls/client-ca.crt\n");
        assert_eq!(parse_relay_config(&mutual), (true, true));
    }

    #[test]
    fn in_cluster_urls_become_service_targets() {
        let target = in_cluster_url(
            "http://netobserv-loki.netobserv.svc.cluster.local.:3100/",
            "other",
        )
        .unwrap();
        assert_eq!(target.label(), "netobserv/netobserv-loki");
        assert_eq!(
            (
                target.port.as_str(),
                target.scheme.as_str(),
                target.path.as_str()
            ),
            ("3100", "http", "")
        );
        let prom = in_cluster_url(
            "https://thanos.openshift-monitoring.svc:9091/api",
            "netobserv",
        )
        .unwrap();
        assert_eq!(
            (
                prom.namespace.as_str(),
                prom.port.as_str(),
                prom.path.as_str()
            ),
            ("openshift-monitoring", "9091", "/api")
        );
        assert!(in_cluster_url("https://loki.example.com/", "netobserv").is_none());
        assert!(in_cluster_url("http://localhost:3100/", "netobserv").is_none());
        assert!(in_cluster_url("http://10.96.0.5:3100/", "netobserv").is_none());
        // Cluster-only short names.
        let short = in_cluster_url("http://loki:3100/", "flows").unwrap();
        assert_eq!(short.label(), "flows/loki");
        let dotted = in_cluster_url("http://loki.netobserv:3100/", "flows").unwrap();
        assert_eq!(dotted.label(), "netobserv/loki");
    }

    #[test]
    fn flow_collector_loki() {
        let monolithic = serde_json::json!({ "loki": { "mode": "Monolithic", "monolithic": { "url": "http://netobserv-loki.netobserv.svc.cluster.local.:3100/" } } });
        assert!(
            matches!(loki_of(&monolithic), LokiTarget::Service(t) if t.service == "netobserv-loki")
        );
        let stack = serde_json::json!({ "loki": { "mode": "LokiStack", "lokiStack": { "name": "loki", "namespace": "netobserv" } } });
        assert_eq!(
            loki_of(&stack),
            LokiTarget::LokiStack {
                namespace: "netobserv".into(),
                name: "loki".into()
            }
        );
        let off = serde_json::json!({ "loki": { "enable": false } });
        assert_eq!(loki_of(&off), LokiTarget::Disabled);
        let external = serde_json::json!({ "loki": { "mode": "Manual", "manual": { "querierUrl": "https://loki.example.com" } } });
        assert_eq!(
            loki_of(&external),
            LokiTarget::External("https://loki.example.com".into())
        );
    }
}
