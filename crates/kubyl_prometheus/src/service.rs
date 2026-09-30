//! Which Prometheus-compatible servers each cluster has: Prometheus, Thanos Query, OpenShift's
//! thanos-querier, VictoriaMetrics. The Prometheus sidebar row shows while a cluster has at
//! least one, and the view lets the user pick between several.
//!
//! Only discovery lives here. Data (targets, rules, query results) is read by the open view
//! itself, so nothing is polled for a view nobody has open.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, SharedString, Subscription, Task};
use kubyl_core::{ClusterId, spawn_kube};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_metrics::MetricsService;
use kubyl_metrics::discover::{self, Candidate};
use kubyl_metrics::prometheus::{PromClient, Target};
use kubyl_metrics::settings::MetricsSettings;
use kubyl_settings::Settings;
use serde_json::Value;

/// How long a cluster without any Prometheus waits before it is searched again.
const REDISCOVER: Duration = Duration::from_secs(300);
/// How long discovery waits for the metrics service's own detection (it knows about overrides,
/// external URLs and OpenShift Routes).
const METRICS_WAIT: Duration = Duration::from_secs(20);
const PROBE_TIMEOUT: Duration = Duration::from_secs(6);
/// Services probed per cluster at most.
const MAX_PROBES: usize = 8;
/// Listing Services and probing them together may take this long.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);

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

    fn of(target: &Target) -> Self {
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

/// A server the view can talk to.
#[derive(Clone, Debug)]
pub struct Instance {
    /// Stable within a cluster: `monitoring/prometheus-operated`.
    pub id: String,
    pub kind: Kind,
    pub client: PromClient,
}

impl Instance {
    pub(crate) fn new(client: PromClient) -> Self {
        let target = client.target();
        Self {
            id: key(target),
            kind: Kind::of(target),
            client,
        }
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
fn key(target: &Target) -> String {
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

struct ClusterState {
    phase: Phase,
    instances: Vec<Instance>,
    generation: u64,
    since: Instant,
    discovered_at: Option<Instant>,
    discovering: bool,
}

impl ClusterState {
    fn new(generation: u64) -> Self {
        Self {
            phase: Phase::Unknown,
            instances: Vec::new(),
            generation,
            since: Instant::now(),
            discovered_at: None,
            discovering: false,
        }
    }
}

pub struct PrometheusService {
    clusters: HashMap<ClusterId, ClusterState>,
    generation: u64,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

struct GlobalService(Entity<PrometheusService>);

impl Global for GlobalService {}

impl PrometheusService {
    /// Creates the service and makes it global. `tick` starts the discovery loop (off in tests).
    pub fn install(tick: bool, cx: &mut App) -> Entity<Self> {
        let service = cx.new(|cx| Self::new(tick, cx));
        cx.set_global(GlobalService(service.clone()));
        service
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalService>().map(|g| g.0.clone())
    }

    fn new(tick: bool, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe(&manager, |this, _, event, cx| {
                this.connection_event(event, cx)
            }));
        }
        let task = if tick {
            cx.spawn(async move |this, cx| {
                loop {
                    if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                }
            })
        } else {
            Task::ready(())
        };
        Self {
            clusters: HashMap::new(),
            generation: 0,
            _tick: task,
            _subscriptions: subscriptions,
        }
    }

    // ----- Reading -----

    pub fn phase(&self, cluster: &ClusterId) -> Phase {
        self.clusters
            .get(cluster)
            .map_or(Phase::Unknown, |c| c.phase.clone())
    }

    pub fn instances(&self, cluster: &ClusterId) -> &[Instance] {
        self.clusters
            .get(cluster)
            .map_or(&[], |c| c.instances.as_slice())
    }

    /// Whether the cluster has a Prometheus (`None` while still looking).
    pub fn has_instances(&self, cluster: &ClusterId) -> Option<bool> {
        match self.clusters.get(cluster)?.phase {
            Phase::Ready => Some(true),
            Phase::NoSource(_) | Phase::Disabled => Some(false),
            Phase::Unknown | Phase::Discovering => None,
        }
    }

    // ----- Control -----

    /// Forgets what was found and looks again.
    pub fn redetect(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        self.reset(cluster);
        self.tick(cx);
        crate::changed(cx);
        cx.notify();
    }

    fn reset(&mut self, cluster: &ClusterId) {
        self.generation += 1;
        self.clusters
            .insert(cluster.clone(), ClusterState::new(self.generation));
    }

    fn connection_event(&mut self, event: &ConnectionEvent, cx: &mut Context<Self>) {
        match event {
            ConnectionEvent::StateChanged(id) => {
                let connected = ConnectionManager::try_global(cx)
                    .and_then(|m| m.read(cx).client(id))
                    .is_some();
                // A reconnect may reach another cluster (kubeconfig edited): start over.
                if !connected && self.clusters.remove(id).is_some() {
                    self.generation += 1;
                    crate::changed(cx);
                    cx.notify();
                }
            }
            ConnectionEvent::DiscoveryChanged(id) => {
                // Prometheus may have been installed.
                if matches!(self.phase(id), Phase::NoSource(_)) {
                    self.reset(id);
                    cx.notify();
                }
            }
            ConnectionEvent::Rekeyed { from, to } if self.clusters.remove(from).is_some() => {
                self.reset(to);
                crate::changed(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let connected: Vec<(ClusterId, kube::Client, Vec<String>)> = {
            let manager = manager.read(cx);
            manager
                .entries()
                .iter()
                .filter_map(|c| {
                    Some((
                        c.id.clone(),
                        manager.client(&c.id)?,
                        manager.settings_keys(&c.id),
                    ))
                })
                .collect()
        };
        let gone: Vec<ClusterId> = self
            .clusters
            .keys()
            .filter(|id| !connected.iter().any(|(c, _, _)| &c == id))
            .cloned()
            .collect();
        if !gone.is_empty() {
            for id in gone {
                self.clusters.remove(&id);
            }
            self.generation += 1;
            crate::changed(cx);
            cx.notify();
        }
        for (cluster, client, keys) in connected {
            let disabled = Settings::get::<MetricsSettings>(cx)
                .prometheus_for_keys(&keys)
                .is_some_and(|p| p.disabled);
            let generation = self.generation;
            let state = self
                .clusters
                .entry(cluster.clone())
                .or_insert_with(|| ClusterState::new(generation));
            if disabled {
                if state.phase != Phase::Disabled {
                    state.phase = Phase::Disabled;
                    crate::changed(cx);
                    cx.notify();
                }
                continue;
            }
            if state.phase == Phase::Disabled {
                state.phase = Phase::Unknown;
            }
            let rediscover = match state.phase {
                Phase::Unknown | Phase::Discovering => true,
                Phase::NoSource(_) => state
                    .discovered_at
                    .is_some_and(|t| t.elapsed() >= REDISCOVER),
                Phase::Ready | Phase::Disabled => false,
            };
            if rediscover && !state.discovering {
                self.discover(&cluster, client, cx);
            }
        }
    }

    fn discover(&mut self, cluster: &ClusterId, client: kube::Client, cx: &mut Context<Self>) {
        // The metrics service knows settings overrides, external URLs and OpenShift Routes:
        // wait a little for its detection, then take its client as the first instance.
        let (metrics_client, metrics_pending) = match MetricsService::global(cx) {
            Some(metrics) => {
                let metrics = metrics.read(cx);
                let source = metrics.source(cluster);
                (
                    metrics.prometheus(cluster),
                    matches!(
                        source,
                        kubyl_metrics::Source::Unknown | kubyl_metrics::Source::Detecting
                    ),
                )
            }
            None => (None, false),
        };
        let Some(state) = self.clusters.get_mut(cluster) else {
            return;
        };
        if metrics_pending && state.since.elapsed() < METRICS_WAIT {
            state.phase = Phase::Discovering;
            return;
        }
        state.discovering = true;
        state.phase = Phase::Discovering;
        let generation = state.generation;
        let task = spawn_kube(cx, async move {
            // A stalled API server must not leave the cluster "discovering" forever.
            tokio::time::timeout(DISCOVERY_TIMEOUT, find(client, metrics_client))
                .await
                .unwrap_or_default()
        });
        let cluster = cluster.clone();
        cx.spawn(async move |this, cx| {
            let instances = task.await;
            this.update(cx, |this, cx| {
                let Some(state) = this.clusters.get_mut(&cluster) else {
                    return;
                };
                if state.generation != generation {
                    return;
                }
                state.discovering = false;
                state.discovered_at = Some(Instant::now());
                state.phase = if instances.is_empty() {
                    Phase::NoSource(
                        "No Prometheus, Thanos Query or VictoriaMetrics answered in this cluster."
                            .into(),
                    )
                } else {
                    Phase::Ready
                };
                state.instances = instances;
                crate::changed(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// Lists Services, ranks and de-duplicates them, and probes each. The metrics service's client
/// comes first.
async fn find(client: kube::Client, metrics_client: Option<PromClient>) -> Vec<Instance> {
    let services = discover::list_services(&client).await;
    let targets = probe_targets(&services, metrics_client.as_ref().map(PromClient::target));
    let probes = targets.into_iter().take(MAX_PROBES).map(|target| {
        let prom = PromClient::new(client.clone(), target);
        async move { prom.probe(PROBE_TIMEOUT).await.ok().map(|()| prom) }
    });
    let mut found: Vec<PromClient> = metrics_client.into_iter().collect();
    for prom in futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
    {
        if !found.iter().any(|f| key(f.target()) == key(prom.target())) {
            found.push(prom);
        }
    }
    found.into_iter().map(Instance::new).collect()
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
