//! Which Prometheus-compatible servers a cluster has (Prometheus, Thanos Query, OpenShift's
//! thanos-querier, VictoriaMetrics), how they're reached and whether they want a password.
//! `kubyl_prometheus::service::PrometheusService` keeps them per cluster and signs in.

use std::collections::BTreeMap;
use std::time::Duration;

use kubyl_base::SharedString;
use kubyl_metrics_core::discover::{self, Candidate};
use kubyl_metrics_core::prometheus::{PromClient, PromError, Target};
use serde_json::Value;

/// How long a cluster without any Prometheus waits before it is searched again.
pub const REDISCOVER: Duration = Duration::from_secs(300);
/// How long discovery waits for the metrics service's own detection (it knows about overrides,
/// external URLs and OpenShift Routes).
pub const METRICS_WAIT: Duration = Duration::from_secs(20);
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// Services probed per cluster at most.
pub const MAX_PROBES: usize = 8;
/// Listing Services and probing them together may take this long.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Prometheus,
    Thanos,
    VictoriaMetrics,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Prometheus => "Prometheus",
            Kind::Thanos => "Thanos Query",
            Kind::VictoriaMetrics => "VictoriaMetrics",
        }
    }

    pub fn of(target: &Target) -> Self {
        let (name, is_service) = match target {
            Target::Service { service, .. } | Target::Route { service, .. } => {
                (service.to_lowercase(), true)
            }
            Target::Url { url, .. } => (url.to_lowercase(), false),
        };
        // Thanos' other components (ruler, store) have no query UI either, but only a querier
        // answers the query API.
        if name.contains("thanos") && (name.contains("query") || name.contains("querier")) {
            Kind::Thanos
        } else if name.contains("victoria") || (is_service && name.starts_with("vm")) {
            Kind::VictoriaMetrics
        } else {
            Kind::Prometheus
        }
    }
}

/// Whether a server can be read, or wants a username and password first.
#[derive(Clone, Debug, PartialEq)]
pub enum Access {
    /// Answers without credentials.
    Open,
    /// HTTP basic auth and no working credentials. `saved`: the keychain has some (the
    /// forward was stopped, or the server didn't answer), so reconnecting may work. `problem`
    /// says why the last attempt failed.
    Locked {
        saved: bool,
        problem: Option<SharedString>,
    },
    /// Signing in.
    Unlocking,
    /// Signed in with credentials from the keychain, through a loopback forward.
    SignedIn,
}

/// A server the view can talk to.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Stable within a cluster: `monitoring/prometheus-operated`.
    pub id: String,
    pub kind: Kind,
    pub client: PromClient,
    pub access: Access,
    /// Which sign-in the client belongs to: a refused read only locks the server again when
    /// it was made with the current credentials.
    pub session: u64,
}

impl Instance {
    pub fn new(client: PromClient) -> Self {
        let target = client.target();
        Self {
            id: key(target),
            kind: Kind::of(target),
            client,
            access: Access::Open,
            session: 0,
        }
    }

    /// A server that asks for a username and password.
    pub fn locked(client: PromClient) -> Self {
        Self {
            access: Access::Locked {
                saved: false,
                problem: None,
            },
            ..Self::new(client)
        }
    }

    /// Whether the tabs can read it.
    pub fn is_readable(&self) -> bool {
        matches!(self.access, Access::Open | Access::SignedIn)
    }

    /// `monitoring/prometheus-k8s`.
    pub fn label(&self) -> String {
        self.client.target().label()
    }

    /// The Service behind it (for the web UI), when it is one.
    pub fn service(&self) -> Option<(&str, &str, &str, &str)> {
        match self.client.target() {
            Target::Service {
                namespace,
                service,
                port,
                path,
                ..
            } => Some((namespace, service, port, path)),
            _ => None,
        }
    }
}

/// One server, however it is reached (service proxy or Route).
pub fn key(target: &Target) -> String {
    match target {
        Target::Service {
            namespace, service, ..
        }
        | Target::Route {
            namespace, service, ..
        } => format!("{namespace}/{service}"),
        Target::Url { url, .. } => url.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Unknown,
    Discovering,
    Ready,
    /// Nothing answered. The text says why, for the empty state.
    NoSource(SharedString),
    Disabled,
}

/// Lists Services, ranks and de-duplicates them, and probes each. The metrics service's client
/// comes first, servers that want a password last.
pub async fn find(client: kube::Client, metrics_client: Option<PromClient>) -> Vec<Instance> {
    let services = discover::list_services(&client).await;
    let targets = probe_targets(&services, metrics_client.as_ref().map(PromClient::target));
    let probes = targets.into_iter().take(MAX_PROBES).map(|target| {
        let prom = PromClient::new(client.clone(), target);
        async move {
            match prom.probe(PROBE_TIMEOUT).await {
                Ok(()) => Some(Instance::new(prom)),
                // An auth proxy (OpenShift's prometheus-k8s) says 401 too, without a Basic
                // challenge: no password would help there.
                Err(PromError::Http(401, _)) if prom.asks_for_basic_auth().await => {
                    Some(Instance::locked(prom))
                }
                Err(_) => None,
            }
        }
    });
    let mut found: Vec<Instance> = metrics_client.into_iter().map(Instance::new).collect();
    for instance in futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
    {
        if !found.iter().any(|f| f.id == instance.id) {
            found.push(instance);
        }
    }
    // The view shows the first one.
    found.sort_by_key(|i| !i.is_readable());
    found
}

fn selector(service: &Value) -> BTreeMap<String, String> {
    service["spec"]["selector"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// The candidates worth probing, best first. Several Services can front the same pods
/// (`prometheus-operated` and the chart's `…-prometheus`): when one's selector is the same as
/// or a subset of a better ranked Service's in the same namespace, it isn't another instance.
pub fn probe_targets(services: &[Value], preferred: Option<&Target>) -> Vec<Target> {
    let mut ranked: Vec<(Candidate, String, BTreeMap<String, String>)> = services
        .iter()
        .filter_map(|service| {
            let candidate = discover::candidates(std::slice::from_ref(service))
                .into_iter()
                .next()?;
            let namespace = service["metadata"]["namespace"]
                .as_str()
                .unwrap_or("default")
                .to_string();
            Some((candidate, namespace, selector(service)))
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.score
            .cmp(&a.0.score)
            .then_with(|| a.0.target.label().cmp(&b.0.target.label()))
    });
    // The server the metrics service found comes first whatever its rank; Services over its pods
    // (either way round: `prometheus-operated` covers a chart's own Service) aren't another one.
    let preferred_pods = preferred.and_then(|target| {
        let (Target::Service {
            namespace, service, ..
        }
        | Target::Route {
            namespace, service, ..
        }) = target
        else {
            return None;
        };
        let found = services.iter().find(|s| {
            s["metadata"]["namespace"].as_str() == Some(namespace)
                && s["metadata"]["name"].as_str() == Some(service)
        })?;
        Some((namespace.clone(), selector(found)))
    });
    let mut kept: Vec<(Candidate, String, BTreeMap<String, String>)> = Vec::new();
    for (candidate, namespace, selector) in ranked {
        let same_as_preferred = !selector.is_empty()
            && preferred_pods.as_ref().is_some_and(|(ns, other)| {
                *ns == namespace
                    && !other.is_empty()
                    && (selector.iter().all(|(k, v)| other.get(k) == Some(v))
                        || other.iter().all(|(k, v)| selector.get(k) == Some(v)))
            });
        let same_pods = same_as_preferred
            || (!selector.is_empty()
                && kept.iter().any(|(_, ns, other)| {
                    *ns == namespace && selector.iter().all(|(k, v)| other.get(k) == Some(v))
                }));
        if !same_pods {
            kept.push((candidate, namespace, selector));
        }
    }
    kept.into_iter().map(|(c, _, _)| c.target).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn svc(namespace: &str, name: &str, selector: Value) -> Value {
        json!({"metadata": {"name": name, "namespace": namespace},
               "spec": {"ports": [{"name": "web", "port": 9090}], "selector": selector}})
    }

    fn labels(targets: &[Target]) -> Vec<String> {
        targets.iter().map(Target::label).collect()
    }

    #[test]
    fn services_in_front_of_the_same_pods_are_one_instance() {
        let services = vec![
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                json!({"app.kubernetes.io/name": "prometheus", "operator.prometheus.io/name": "kps"}),
            ),
            svc(
                "monitoring",
                "prometheus-operated",
                json!({"app.kubernetes.io/name": "prometheus"}),
            ),
        ];
        assert_eq!(
            labels(&probe_targets(&services, None)),
            ["monitoring/kube-prometheus-stack-prometheus"]
        );
    }

    #[test]
    fn two_prometheuses_stay_two() {
        let services = vec![
            svc(
                "monitoring",
                "prometheus-a",
                json!({"app.kubernetes.io/name": "prometheus", "operator.prometheus.io/name": "a"}),
            ),
            svc(
                "monitoring",
                "prometheus-b",
                json!({"app.kubernetes.io/name": "prometheus", "operator.prometheus.io/name": "b"}),
            ),
            svc(
                "other",
                "prometheus-server",
                json!({"app.kubernetes.io/name": "prometheus"}),
            ),
        ];
        assert_eq!(probe_targets(&services, None).len(), 3);
    }

    #[test]
    fn thanos_next_to_prometheus_is_another_instance() {
        let services = vec![
            svc(
                "monitoring",
                "prometheus-operated",
                json!({"app": "prometheus"}),
            ),
            svc("monitoring", "thanos-query", json!({"app": "thanos-query"})),
        ];
        let targets = probe_targets(&services, None);
        assert_eq!(targets.len(), 2);
        assert_eq!(Kind::of(&targets[1]), Kind::Thanos);
    }

    #[test]
    fn a_service_over_the_preferred_servers_pods_is_not_another_one() {
        let services = vec![
            svc(
                "monitoring",
                "prometheus-operated",
                json!({"app": "prometheus"}),
            ),
            svc(
                "monitoring",
                "kube-prometheus-stack-prometheus",
                json!({"app": "prometheus", "operator.prometheus.io/name": "kps"}),
            ),
            svc("monitoring", "thanos-query", json!({"app": "thanos-query"})),
        ];
        let preferred = Target::service("monitoring", "prometheus-operated", "9090");
        assert_eq!(
            labels(&probe_targets(&services, Some(&preferred))),
            ["monitoring/thanos-query"]
        );
    }

    #[test]
    fn kinds_follow_the_name() {
        let kind = |name: &str| Kind::of(&Target::service("ns", name, "9090"));
        assert_eq!(kind("thanos-querier"), Kind::Thanos);
        assert_eq!(kind("thanos-query-frontend"), Kind::Thanos);
        assert_eq!(kind("thanos-ruler"), Kind::Prometheus);
        assert_eq!(kind("vmselect-cluster"), Kind::VictoriaMetrics);
        assert_eq!(kind("prometheus-k8s"), Kind::Prometheus);
    }
}
