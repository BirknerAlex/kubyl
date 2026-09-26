//! Finding a cluster's Alertmanagers.
//!
//! In this order:
//! 1. Settings (`alerts.clusters.<cluster>.alertmanagers`): with entries there, nothing is
//!    discovered.
//! 2. prometheus-operator objects: the `spec.alerting.alertmanagers` of `Prometheus` objects,
//!    and `Alertmanager` objects (the Service selecting `alertmanager: <name>`, else
//!    `alertmanager-operated`; the prefix is `spec.routePrefix`, else the path of
//!    `spec.externalUrl`).
//! 3. Prometheus' own list (`/api/v1/alertmanagers`: pod URLs, mapped to Services through
//!    EndpointSlices).
//! 4. Service names and labels (kube-prometheus, OpenShift, kube-prometheus-stack, the
//!    community chart, VictoriaMetrics, GMP, `app.kubernetes.io/name=alertmanager`).
//!
//! Services that select the same pods count as one (the ClusterIP Service wins over the
//! headless `-operated`). Every candidate is probed with `GET <prefix>/api/v2/status`.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::settings::AlertmanagerConfig;

/// OpenShift's platform monitoring and user workload monitoring: only cluster admins can
/// create `openshift-*` namespaces, so their Services may receive the user's token.
pub const OPENSHIFT_MONITORING: &str = "openshift-monitoring";
pub const OPENSHIFT_USER_WORKLOAD: &str = "openshift-user-workload-monitoring";

/// Namespaces searched when listing Services cluster-wide is forbidden.
pub const WELL_KNOWN_NAMESPACES: &[&str] = &[
    "monitoring",
    "openshift-monitoring",
    "openshift-user-workload-monitoring",
    "prometheus",
    "observability",
    "kube-prometheus-stack",
    "victoria-metrics",
    "vm",
    "gmp-system",
    "cattle-monitoring-system",
    "kube-system",
    "default",
];

/// Where an Alertmanager API lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AmTarget {
    /// A Service, reached through the API server's service proxy (and, behind an auth proxy,
    /// its Route or a temporary forward).
    Service {
        namespace: String,
        service: String,
        /// Port number or name.
        port: String,
        /// `http` or `https`.
        scheme: String,
        /// API prefix (`""`, `/am`).
        path: String,
        /// Named in settings: may receive the user's token behind an auth proxy.
        from_settings: bool,
        /// `namespace/name` of a service account for tokens (settings).
        service_account: Option<(String, String)>,
        /// `X-Scope-OrgID`.
        tenant: Option<String>,
    },
    /// An external URL (a central Alertmanager), with its TLS options.
    Url {
        url: String,
        tenant: Option<String>,
        ca_file: Option<String>,
        client_certificate: Option<String>,
        client_key: Option<String>,
        insecure: bool,
    },
}

impl AmTarget {
    /// `monitoring/alertmanager-operated`, or the URL's host.
    pub fn label(&self) -> String {
        match self {
            AmTarget::Service {
                namespace, service, ..
            } => format!("{namespace}/{service}"),
            AmTarget::Url { url, .. } => url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| url.clone()),
        }
    }

    pub fn service(namespace: &str, service: &str, port: &str, scheme: &str, path: &str) -> Self {
        AmTarget::Service {
            namespace: namespace.into(),
            service: service.into(),
            port: port.into(),
            scheme: scheme.into(),
            path: normalize(path),
            from_settings: false,
            service_account: None,
            tenant: None,
        }
    }
}

/// The token rule (decision 4 of phase 14): the user's kube token, or a service-account token
/// Kubyl minted, only goes to Services in `openshift-monitoring` or
/// `openshift-user-workload-monitoring` (only cluster admins can create `openshift-*`
/// namespaces) or to Services named in settings. Never to a Service found elsewhere, and never
/// to an external URL (those use the keychain header). Transports enforce HTTPS with verified
/// TLS on top.
pub fn token_allowed(target: &AmTarget) -> bool {
    match target {
        AmTarget::Service {
            namespace,
            from_settings,
            ..
        } => {
            *from_settings
                || namespace == OPENSHIFT_MONITORING
                || namespace == OPENSHIFT_USER_WORKLOAD
        }
        AmTarget::Url { .. } => false,
    }
}

fn normalize(path: &str) -> String {
    kubyl_metrics::transport::normalize_prefix(path)
}

/// A candidate and how it was found (for "what was tried").
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub target: AmTarget,
    pub score: i32,
    pub found_by: &'static str,
}

/// Targets named in settings.
/// The URL an external Alertmanager's API lives at: `url` without a trailing slash, plus `path`.
/// `None` for Service entries.
pub fn url_of(config: &AlertmanagerConfig) -> Option<String> {
    let url = config.url.as_deref().filter(|u| !u.is_empty())?;
    Some(format!(
        "{}{}",
        url.trim_end_matches('/'),
        normalize(config.path.as_deref().unwrap_or(""))
    ))
}

pub fn from_settings(configs: &[AlertmanagerConfig]) -> Vec<Candidate> {
    configs
        .iter()
        .filter_map(|c| {
            let tenant = c.tenant.clone().filter(|t| !t.is_empty());
            let target = match (url_of(c), &c.service) {
                (Some(url), _) => AmTarget::Url {
                    url,
                    tenant,
                    ca_file: c.ca_file.clone().filter(|p| !p.is_empty()),
                    client_certificate: c.client_certificate.clone().filter(|p| !p.is_empty()),
                    client_key: c.client_key.clone().filter(|p| !p.is_empty()),
                    insecure: c.insecure_skip_tls_verify,
                },
                (_, Some(service)) if !service.is_empty() => AmTarget::Service {
                    namespace: c.namespace.clone().unwrap_or_else(|| "monitoring".into()),
                    service: service.clone(),
                    port: c.port.clone().unwrap_or_default(),
                    scheme: c.scheme.clone().unwrap_or_else(|| "http".into()),
                    path: normalize(c.path.as_deref().unwrap_or("")),
                    from_settings: true,
                    service_account: c
                        .service_account
                        .as_deref()
                        .and_then(|sa| sa.split_once('/'))
                        .map(|(ns, name)| (ns.to_string(), name.to_string())),
                    tenant,
                },
                _ => return None,
            };
            Some(Candidate {
                target,
                score: 1000,
                found_by: "settings",
            })
        })
        .collect()
}

fn meta<'a>(object: &'a Value, key: &str) -> &'a str {
    object["metadata"][key].as_str().unwrap_or_default()
}

fn selector(service: &Value) -> BTreeMap<String, String> {
    service["spec"]["selector"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn headless(service: &Value) -> bool {
    service["spec"]["clusterIP"].as_str() == Some("None")
}

/// The API port of an Alertmanager Service: named `web`/`http-web`/`http`, or 9093–9095,
/// else the first; HTTPS for OpenShift's (`web` behind kube-rbac-proxy) and `https` names.
fn pick_port(service: &Value) -> Option<(String, &'static str)> {
    let ports = service["spec"]["ports"].as_array()?;
    let name = |p: &Value| p["name"].as_str().unwrap_or_default().to_string();
    let number = |p: &Value| p["port"].as_i64();
    let port = ports
        .iter()
        .find(|p| matches!(name(p).as_str(), "web" | "http-web" | "http"))
        .or_else(|| {
            ports
                .iter()
                .find(|p| matches!(number(p), Some(9093..=9095)))
        })
        .or_else(|| ports.first())?;
    let namespace = meta(service, "namespace");
    let https = name(port).contains("https")
        || number(port) == Some(443)
        || ((namespace == OPENSHIFT_MONITORING || namespace == OPENSHIFT_USER_WORKLOAD)
            && matches!(number(port), Some(9094 | 9095)));
    Some((
        number(port)?.to_string(),
        if https { "https" } else { "http" },
    ))
}

/// How much a Service's name and labels look like an Alertmanager API (0: not at all).
fn name_score(service: &Value) -> i32 {
    let name = meta(service, "name").to_lowercase();
    let namespace = meta(service, "namespace");
    let label = |key: &str| {
        service["metadata"]["labels"][key]
            .as_str()
            .unwrap_or_default()
    };
    if namespace == OPENSHIFT_MONITORING && name == "alertmanager-main" {
        150
    } else if namespace == OPENSHIFT_USER_WORKLOAD && name == "alertmanager-user-workload" {
        140
    } else if name.ends_with("-kube-prometheus-stack-alertmanager")
        || name.ends_with("-kube-prom-alertmanager")
        || name == "kube-prometheus-stack-alertmanager"
    {
        120
    } else if name == "alertmanager-main" {
        115
    } else if name == "alertmanager-operated" {
        100
    } else if name == "alertmanager" {
        85
    } else if name.ends_with("-alertmanager") && !name.ends_with("-headless-alertmanager") {
        90
    } else if name.starts_with("vmalertmanager-") {
        80
    } else if label("app.kubernetes.io/name") == "alertmanager" || label("app") == "alertmanager" {
        70
    } else {
        0
    }
}

/// Candidates from Service names and labels (step 4), deduplicated by selector.
pub fn from_services(services: &[Value]) -> Vec<Candidate> {
    let mut found: Vec<(Candidate, BTreeMap<String, String>, bool)> = Vec::new();
    for service in services {
        let score = name_score(service);
        if score == 0 {
            continue;
        }
        let Some((port, scheme)) = pick_port(service) else {
            continue;
        };
        let candidate = Candidate {
            target: AmTarget::service(
                meta(service, "namespace"),
                meta(service, "name"),
                &port,
                scheme,
                "",
            ),
            score: if meta(service, "namespace") == "monitoring" {
                score + 1
            } else {
                score
            },
            found_by: "Service name",
        };
        found.push((candidate, selector(service), headless(service)));
    }
    dedupe(found)
}

/// Services selecting the same pods are one Alertmanager: the best score wins, ClusterIP over
/// headless.
fn dedupe(mut found: Vec<(Candidate, BTreeMap<String, String>, bool)>) -> Vec<Candidate> {
    found.sort_by(|a, b| {
        a.2.cmp(&b.2)
            .then_with(|| b.0.score.cmp(&a.0.score))
            .then_with(|| a.0.target.label().cmp(&b.0.target.label()))
    });
    let mut kept: Vec<(Candidate, BTreeMap<String, String>, String)> = Vec::new();
    for (candidate, selector, _) in found {
        let namespace = match &candidate.target {
            AmTarget::Service { namespace, .. } => namespace.clone(),
            AmTarget::Url { .. } => String::new(),
        };
        let same = kept.iter_mut().find(|(_, s, ns)| {
            !selector.is_empty()
                && *ns == namespace
                && (*s == selector || subset(s, &selector) || subset(&selector, s))
        });
        match same {
            Some((existing, _, _)) => existing.score = existing.score.max(candidate.score),
            None => kept.push((candidate, selector, namespace)),
        }
    }
    let mut out: Vec<Candidate> = kept.into_iter().map(|(c, _, _)| c).collect();
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.target.label().cmp(&b.target.label()))
    });
    out
}

fn subset(small: &BTreeMap<String, String>, big: &BTreeMap<String, String>) -> bool {
    !small.is_empty() && small.iter().all(|(k, v)| big.get(k) == Some(v))
}

/// Candidates from prometheus-operator objects (step 2): `Prometheus` objects'
/// `spec.alerting.alertmanagers`, and `Alertmanager` objects.
pub fn from_operator(
    prometheuses: &[Value],
    alertmanagers: &[Value],
    services: &[Value],
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let service_port = |namespace: &str, name: &str| {
        services
            .iter()
            .find(|s| meta(s, "namespace") == namespace && meta(s, "name") == name)
            .and_then(pick_port)
    };
    for prometheus in prometheuses {
        let default_ns = meta(prometheus, "namespace");
        for entry in prometheus["spec"]["alerting"]["alertmanagers"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(name) = entry["name"].as_str() else {
                continue;
            };
            let namespace = entry["namespace"].as_str().unwrap_or(default_ns);
            let port = match &entry["port"] {
                Value::String(p) => p.clone(),
                Value::Number(n) => n.to_string(),
                _ => service_port(namespace, name)
                    .map(|p| p.0)
                    .unwrap_or_else(|| "web".into()),
            };
            let scheme = entry["scheme"].as_str().unwrap_or("http");
            out.push(Candidate {
                target: AmTarget::service(
                    namespace,
                    name,
                    &port,
                    scheme,
                    entry["pathPrefix"].as_str().unwrap_or(""),
                ),
                score: 200,
                found_by: "Prometheus object",
            });
        }
    }
    for am in alertmanagers {
        let namespace = meta(am, "namespace");
        let name = meta(am, "name");
        // The Service selecting `alertmanager: <name>`, else the operator's headless one.
        let service = services
            .iter()
            .filter(|s| meta(s, "namespace") == namespace)
            .filter(|s| selector(s).get("alertmanager").map(String::as_str) == Some(name))
            .min_by_key(|s| headless(s))
            .map(|s| meta(s, "name").to_string())
            .unwrap_or_else(|| "alertmanager-operated".into());
        let prefix = am["spec"]["routePrefix"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                am["spec"]["externalUrl"]
                    .as_str()
                    .and_then(|u| url::Url::parse(u).ok())
                    .map(|u| u.path().to_string())
            })
            .unwrap_or_default();
        let (port, scheme) = service_port(namespace, &service).unwrap_or(("9093".into(), "http"));
        out.push(Candidate {
            target: AmTarget::service(namespace, &service, &port, scheme, &prefix),
            score: 190,
            found_by: "Alertmanager object",
        });
    }
    out
}

/// Candidates from Prometheus' `/api/v1/alertmanagers` (step 3): pod URLs mapped to the
/// Service whose EndpointSlice has that address and port.
pub fn from_prometheus(urls: &[String], slices: &[Value], services: &[Value]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for raw in urls {
        let Ok(url) = url::Url::parse(raw) else {
            continue;
        };
        let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
            continue;
        };
        let prefix = url
            .path()
            .trim_end_matches('/')
            .trim_end_matches("/api/v2/alerts")
            .trim_end_matches("/api/v1/alerts")
            .to_string();
        let slice = slices.iter().find(|slice| {
            slice["endpoints"].as_array().is_some_and(|eps| {
                eps.iter().any(|ep| {
                    ep["addresses"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|a| a.as_str() == Some(host)))
                })
            }) && slice["ports"]
                .as_array()
                .is_some_and(|ps| ps.iter().any(|p| p["port"].as_u64() == Some(port as u64)))
        });
        let Some(slice) = slice else {
            continue;
        };
        let namespace = meta(slice, "namespace");
        let Some(service) = slice["metadata"]["labels"]["kubernetes.io/service-name"].as_str()
        else {
            continue;
        };
        // The Service port that targets this container port.
        let service_port = services
            .iter()
            .find(|s| meta(s, "namespace") == namespace && meta(s, "name") == service)
            .and_then(|s| {
                s["spec"]["ports"].as_array()?.iter().find(|p| {
                    p["targetPort"].as_u64() == Some(port as u64)
                        || p["port"].as_u64() == Some(port as u64)
                        || p["targetPort"].as_str().is_some()
                })
            })
            .and_then(|p| p["port"].as_i64())
            .map(|p| p.to_string())
            .unwrap_or_else(|| port.to_string());
        out.push(Candidate {
            target: AmTarget::service(namespace, service, &service_port, url.scheme(), &prefix),
            score: 180,
            found_by: "Prometheus' Alertmanager list",
        });
    }
    out
}

/// All candidates, best first, each target once.
pub fn rank(mut lists: Vec<Vec<Candidate>>) -> Vec<Candidate> {
    let mut all: Vec<Candidate> = lists.drain(..).flatten().collect();
    all.sort_by_key(|c| std::cmp::Reverse(c.score));
    let mut out: Vec<Candidate> = Vec::new();
    for candidate in all {
        let same = out.iter().any(|c| match (&c.target, &candidate.target) {
            (
                AmTarget::Service {
                    namespace: a,
                    service: s,
                    ..
                },
                AmTarget::Service {
                    namespace: b,
                    service: t,
                    ..
                },
            ) => a == b && s == t,
            (a, b) => a == b,
        });
        if !same {
            out.push(candidate);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc(namespace: &str, name: &str, selector: Value, ports: Value, headless: bool) -> Value {
        json!({"metadata": {"name": name, "namespace": namespace, "labels": {}},
               "spec": {"selector": selector, "ports": ports, "clusterIP": if headless { "None" } else { "10.96.0.9" }}})
    }

    fn labels(target: &AmTarget) -> String {
        target.label()
    }

    /// kube-prometheus-stack: the ClusterIP Service and the operator's headless one select the
    /// same pods; the ClusterIP one wins.
    #[test]
    fn kube_prometheus_stack() {
        let services = vec![
            svc(
                "monitoring",
                "kube-prometheus-stack-alertmanager",
                json!({"app.kubernetes.io/name": "alertmanager", "alertmanager": "kube-prometheus-stack-alertmanager"}),
                json!([{"name": "http-web", "port": 9093}, {"name": "reloader-web", "port": 8080}]),
                false,
            ),
            svc(
                "monitoring",
                "alertmanager-operated",
                json!({"app.kubernetes.io/name": "alertmanager"}),
                json!([{"name": "http-web", "port": 9093}, {"name": "tcp-mesh", "port": 9094}]),
                true,
            ),
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                json!({"app": "prometheus"}),
                json!([{"port": 9090}]),
                false,
            ),
        ];
        let found = from_services(&services);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].target,
            AmTarget::service(
                "monitoring",
                "kube-prometheus-stack-alertmanager",
                "9093",
                "http",
                ""
            )
        );
    }

    #[test]
    fn openshift_platform_and_user_workload() {
        let services = vec![
            svc(
                OPENSHIFT_MONITORING,
                "alertmanager-main",
                json!({"app.kubernetes.io/name": "alertmanager", "alertmanager": "main"}),
                json!([{"name": "web", "port": 9094}, {"name": "tenancy", "port": 9092}, {"name": "metrics", "port": 9097}]),
                false,
            ),
            svc(
                OPENSHIFT_MONITORING,
                "alertmanager-operated",
                json!({"app.kubernetes.io/name": "alertmanager"}),
                json!([{"name": "web", "port": 9093}]),
                true,
            ),
            svc(
                OPENSHIFT_USER_WORKLOAD,
                "alertmanager-user-workload",
                json!({"alertmanager": "user-workload"}),
                json!([{"name": "web", "port": 9095}, {"name": "tenancy", "port": 9092}]),
                false,
            ),
        ];
        let found = from_services(&services);
        let names: Vec<String> = found.iter().map(|c| labels(&c.target)).collect();
        assert_eq!(
            names,
            [
                "openshift-monitoring/alertmanager-main",
                "openshift-user-workload-monitoring/alertmanager-user-workload"
            ]
        );
        match &found[0].target {
            AmTarget::Service { port, scheme, .. } => {
                assert_eq!((port.as_str(), scheme.as_str()), ("9094", "https"))
            }
            other => panic!("{other:?}"),
        }
        assert!(found.iter().all(|c| token_allowed(&c.target)));
    }

    #[test]
    fn community_chart_victoriametrics_and_gmp() {
        let services = vec![
            svc(
                "prometheus",
                "prometheus-alertmanager",
                json!({"app.kubernetes.io/name": "alertmanager", "app.kubernetes.io/instance": "prometheus"}),
                json!([{"name": "http", "port": 9093}]),
                false,
            ),
            svc(
                "prometheus",
                "prometheus-alertmanager-headless",
                json!({"app.kubernetes.io/name": "alertmanager", "app.kubernetes.io/instance": "prometheus"}),
                json!([{"name": "http", "port": 9093}]),
                true,
            ),
            svc(
                "vm",
                "vmalertmanager-vm",
                json!({"app.kubernetes.io/name": "vmalertmanager"}),
                json!([{"name": "http", "port": 9093}]),
                false,
            ),
            svc(
                "gmp-system",
                "alertmanager",
                json!({"app": "managed-prometheus-alertmanager"}),
                json!([{"name": "alertmanager", "port": 9093}]),
                false,
            ),
            svc(
                "apps",
                "shop-frontend",
                json!({"app": "shop"}),
                json!([{"port": 80}]),
                false,
            ),
        ];
        let found = from_services(&services);
        let names: Vec<String> = found.iter().map(|c| labels(&c.target)).collect();
        assert_eq!(
            names,
            [
                "prometheus/prometheus-alertmanager",
                "gmp-system/alertmanager",
                "vm/vmalertmanager-vm"
            ]
        );
        // A look-alike outside the trusted namespaces never gets a token.
        let look_alike =
            AmTarget::service("monitoring-evil", "alertmanager-main", "9094", "https", "");
        assert!(!token_allowed(&look_alike));
        assert!(!token_allowed(&found[0].target));
    }

    #[test]
    fn operator_objects_and_route_prefixes() {
        let services = vec![svc(
            "monitoring",
            "am-prefixed",
            json!({"alertmanager": "prefixed"}),
            json!([{"name": "web", "port": 9093}]),
            false,
        )];
        let alertmanagers = vec![
            json!({"metadata": {"name": "prefixed", "namespace": "monitoring"}, "spec": {"routePrefix": "/am"}}),
            json!({"metadata": {"name": "ext", "namespace": "team"}, "spec": {"externalUrl": "https://am.example.com/alertmanager/"}}),
        ];
        let prometheuses = vec![
            json!({"metadata": {"name": "k8s", "namespace": "monitoring"},
            "spec": {"alerting": {"alertmanagers": [{"name": "alertmanager-main", "port": "web", "pathPrefix": "/"}]}}}),
        ];
        let found = from_operator(&prometheuses, &alertmanagers, &services);
        assert_eq!(
            found[0].target,
            AmTarget::service("monitoring", "alertmanager-main", "web", "http", "")
        );
        assert_eq!(
            found[1].target,
            AmTarget::service("monitoring", "am-prefixed", "9093", "http", "/am")
        );
        assert_eq!(
            found[2].target,
            AmTarget::service(
                "team",
                "alertmanager-operated",
                "9093",
                "http",
                "/alertmanager"
            )
        );
    }

    #[test]
    fn prometheus_pod_urls_map_to_services() {
        let slices = vec![
            json!({"metadata": {"namespace": "monitoring", "labels": {"kubernetes.io/service-name": "kube-prometheus-stack-alertmanager"}},
            "endpoints": [{"addresses": ["10.244.1.7"]}], "ports": [{"name": "http-web", "port": 9093}]}),
        ];
        let services = vec![svc(
            "monitoring",
            "kube-prometheus-stack-alertmanager",
            json!({}),
            json!([{"name": "http-web", "port": 9093, "targetPort": 9093}]),
            false,
        )];
        let found = from_prometheus(
            &[
                "http://10.244.1.7:9093/api/v2/alerts".into(),
                "http://10.9.9.9:9093/api/v2/alerts".into(),
            ],
            &slices,
            &services,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].target,
            AmTarget::service(
                "monitoring",
                "kube-prometheus-stack-alertmanager",
                "9093",
                "http",
                ""
            )
        );
    }

    #[test]
    fn settings_come_first_and_ranking_deduplicates() {
        let configs = vec![
            AlertmanagerConfig {
                namespace: Some("monitoring".into()),
                service: Some("am-proxy".into()),
                port: Some("9443".into()),
                scheme: Some("https".into()),
                service_account: Some("monitoring/am-reader".into()),
                ..Default::default()
            },
            AlertmanagerConfig {
                url: Some("https://am.example.com".into()),
                path: Some("alertmanager".into()),
                tenant: Some("team-a".into()),
                ..Default::default()
            },
        ];
        let settings = from_settings(&configs);
        assert!(token_allowed(&settings[0].target), "named in settings");
        match &settings[1].target {
            AmTarget::Url { url, tenant, .. } => {
                assert_eq!(url, "https://am.example.com/alertmanager");
                assert_eq!(tenant.as_deref(), Some("team-a"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !token_allowed(&settings[1].target),
            "URLs use the keychain header, never a kube token"
        );
        let a = Candidate {
            target: AmTarget::service("m", "am", "9093", "http", ""),
            score: 100,
            found_by: "x",
        };
        let b = Candidate {
            target: AmTarget::service("m", "am", "web", "http", ""),
            score: 200,
            found_by: "y",
        };
        let ranked = rank(vec![vec![a], vec![b]]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].found_by, "y");
    }
}
