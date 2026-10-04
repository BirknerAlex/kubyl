//! Calico's Whisker (README "Flow transports"): the Whisker backend's HTTP API over a temporary
//! loopback port-forward to the `whisker` Service (Calico 3.30 or newer, tested 3.32.2), as
//! Calico's docs reach it. Not the API server's service proxy: the operator's
//! `calico-system.whisker` policy denies all ingress, so the proxy only gets through when Whisker
//! runs on the API server's own node. `GET /whisker-backend/flows?watch=true` is a server-sent
//! event stream that replays Goldmane's buffer from `startTimeGte` and then follows. Goldmane's
//! own gRPC needs a client certificate from a Secret, so Kubyl doesn't use it.
//!
//! Records are 15-second aggregates per source and destination (pods aggregated to
//! `<replicaset>-*`) with packets, bytes and a policy trace. An explicit Deny names its policy;
//! an isolating NetworkPolicy shows as the tier's end-of-tier Deny, whose trigger names the
//! NetworkPolicy that isolated the pod. The API is the Whisker UI's own and unversioned: the
//! mapping stays in this file.

use std::time::Duration;

use futures::StreamExt as _;
use jiff::Timestamp;
use kubyl_metrics_core::transport::{PromError, Transport};
use serde::Deserialize;
use serde_json::json;

use crate::detect::ServiceTarget;
use crate::filter::{Field, FlowFilter, Op, Side, Term};
use crate::model::{
    Direction, Endpoint, EndpointKind, Flow, Policies, PolicyRef, Protocol, Text, Verdict, Workload,
};
use crate::provider::{
    BATCH_WAIT, BackendKind, BackendStatus, Batcher, Capabilities, FlowProvider, FlowSink,
    HISTORY_GAP, ProviderError, ProviderFuture, StreamEvent, StreamQuery,
};

pub struct Whisker {
    /// Reads the Whisker Deployment for its version.
    pub client: kube::Client,
    pub target: ServiceTarget,
    /// The local end of the temporary forward to the Service.
    pub local_port: u16,
}

/// The HTTP client for the forward. Build it on Tokio, inside a provider future: kube's client
/// spawns a task when it's made.
fn transport(local_port: u16) -> Result<Transport, ProviderError> {
    Transport::loopback(local_port, "/whisker-backend")
        .map_err(|e| ProviderError::Unavailable(format!("Calico Whisker: {e}")))
}

/// A proxy error in Kubyl's words.
pub(crate) fn proxy_error(err: PromError, target: &ServiceTarget, what: &str) -> ProviderError {
    match err {
        PromError::Http(403, _) => {
            ProviderError::forbidden("get", "services/proxy", Some(&target.namespace))
        }
        PromError::Http(404, _) => ProviderError::NotFound(format!("{what} at {}", target.label())),
        PromError::Http(503, _) => {
            ProviderError::Unavailable(format!("{} has no ready endpoints (503)", target.label()))
        }
        other => ProviderError::Unavailable(format!("{what}: {other}")),
    }
}

impl FlowProvider for Whisker {
    fn kind(&self) -> BackendKind {
        BackendKind::Whisker
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            names_allows: true,
            names_denies: true,
            names_isolation: true,
            live: true,
            history: "Goldmane's buffer (about an hour)",
            bytes: true,
            l7: false,
            aggregated: true,
            single_flows: true,
            graph_from_metrics: false,
        }
    }

    fn pushdown(&self, filter: &FlowFilter) -> FlowFilter {
        pushdown(filter)
    }

    fn probe(&self) -> ProviderFuture<Result<BackendStatus, ProviderError>> {
        let target = self.target.clone();
        let client = self.client.clone();
        let local_port = self.local_port;
        Box::pin(async move {
            transport(local_port)?
                .get("/flows", &[("page", "0".into()), ("pageSize", "1".into())])
                .await
                .map_err(|e| proxy_error(e, &target, "Calico Whisker"))?;
            Ok(BackendStatus {
                endpoint: target.label(),
                via: format!("through a temporary port-forward (127.0.0.1:{local_port})"),
                version: whisker_version(&client, &target.namespace).await,
                nodes: None,
                buffered: None,
                notes: Vec::new(),
            })
        })
    }

    fn stream(
        &self,
        query: StreamQuery,
        sink: FlowSink,
    ) -> ProviderFuture<Result<(), ProviderError>> {
        let local_port = self.local_port;
        let target = self.target.clone();
        Box::pin(async move {
            let transport = transport(local_port)?;
            let mut params: Vec<(&str, String)> = vec![("watch", "true".into())];
            if let Some(since) = query.since {
                let seconds = Timestamp::now().duration_since(since).as_secs().max(1);
                params.push(("startTimeGte", format!("-{seconds}")));
            }
            if let Some(filters) = to_filters(&query.filter) {
                params.push(("filters", filters.to_string()));
            }
            let mut lines = transport
                .get_lines("/flows", &params, "text/event-stream")
                .await
                .map_err(|e| proxy_error(e, &target, "Calico Whisker"))?;
            let mut batcher = Batcher::new(sink);
            let mut caught_up = query.since.is_none();
            let started = Timestamp::now();
            let mut last_line = std::time::Instant::now();
            loop {
                let line = match tokio::time::timeout(BATCH_WAIT, lines.next()).await {
                    Err(_) => {
                        if batcher.flush().await.is_err() {
                            return Ok(());
                        }
                        if !caught_up && last_line.elapsed() >= HISTORY_GAP {
                            caught_up = true;
                            if batcher.send(StreamEvent::CaughtUp).await.is_err() {
                                return Ok(());
                            }
                        }
                        continue;
                    }
                    Ok(None) => {
                        let _ = batcher.flush().await;
                        return Err(ProviderError::Unavailable(
                            "Calico Whisker ended the stream".into(),
                        ));
                    }
                    Ok(Some(Err(err))) => {
                        let _ = batcher.flush().await;
                        return Err(proxy_error(err, &target, "Calico Whisker"));
                    }
                    Ok(Some(Ok(line))) => line,
                };
                last_line = std::time::Instant::now();
                if let Some(message) = line.strip_prefix("error:") {
                    let _ = batcher.flush().await;
                    return Err(ProviderError::Unavailable(format!(
                        "Calico Whisker: {}",
                        message.trim()
                    )));
                }
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let Ok(record) = serde_json::from_str::<Record>(data.trim()) else {
                    continue;
                };
                let Some(flow) = map_record(record) else {
                    continue;
                };
                // Records of the last interval arrive as the stream catches up.
                if !caught_up && flow.time >= started {
                    caught_up = true;
                    if batcher.send(StreamEvent::CaughtUp).await.is_err() {
                        return Ok(());
                    }
                }
                if batcher.push(flow).await.is_err() {
                    return Ok(());
                }
            }
        })
    }
}

/// The Whisker backend's image tag (`v3.32.2`), when Kubyl may read the Deployment.
async fn whisker_version(client: &kube::Client, namespace: &str) -> Option<String> {
    let api: kube::Api<k8s_openapi::api::apps::v1::Deployment> =
        kube::Api::namespaced(client.clone(), namespace);
    let deployment = tokio::time::timeout(Duration::from_secs(5), api.get_opt("whisker"))
        .await
        .ok()?
        .ok()??;
    deployment
        .spec?
        .template
        .spec?
        .containers
        .into_iter()
        .find(|c| c.name == "whisker-backend")?
        .image?
        .rsplit(':')
        .next()
        .map(|tag| tag.trim_start_matches('v').to_string())
}

// ----- Pushdown -----

fn pushable(term: &Term) -> bool {
    if term.op != Op::Eq || term.has_glob() || term.side == Side::Either && term.field.sided() {
        return false;
    }
    match term.field {
        Field::Namespace | Field::Pod | Field::Workload => {
            !term.values.iter().any(|v| v.contains('/'))
        }
        Field::Port => term.side == Side::Destination && term.exact_numbers().is_some(),
        Field::Protocol => term
            .values
            .iter()
            .all(|v| matches!(v.to_lowercase().as_str(), "tcp" | "udp" | "icmp" | "sctp")),
        Field::Verdict => term.verdicts().is_some_and(|v| {
            v.iter()
                .all(|v| matches!(v, Verdict::Forwarded | Verdict::Dropped))
        }),
        _ => false,
    }
}

/// One-sided namespaces and names, destination ports, protocols and verdicts (Whisker's filters
/// are ANDed per field, so either-side terms stay in Kubyl).
pub fn pushdown(filter: &FlowFilter) -> FlowFilter {
    filter.subset(pushable)
}

/// The `filters` query parameter (JSON), or `None` without pushed terms.
pub fn to_filters(filter: &FlowFilter) -> Option<serde_json::Value> {
    if filter.terms.is_empty() {
        return None;
    }
    let mut out = serde_json::Map::new();
    let mut push = |key: &str, values: Vec<serde_json::Value>| {
        let entry = out
            .entry(key.to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        if let serde_json::Value::Array(list) = entry {
            list.extend(values);
        }
    };
    for term in &filter.terms {
        let source = term.side == Side::Source;
        let text = |fuzzy: bool| -> Vec<serde_json::Value> {
            term.values
                .iter()
                .map(|v| json!({ "value": v, "type": if fuzzy { "Fuzzy" } else { "Exact" } }))
                .collect()
        };
        match term.field {
            Field::Namespace => push(
                if source {
                    "source_namespaces"
                } else {
                    "dest_namespaces"
                },
                text(false),
            ),
            // Names are aggregated (`web-574ff6d9fd-*`): match by substring, Kubyl refines.
            Field::Pod | Field::Workload => push(
                if source { "source_names" } else { "dest_names" },
                text(true),
            ),
            Field::Port => push(
                "dest_ports",
                term.exact_numbers()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| json!({ "value": p, "type": "Exact" }))
                    .collect(),
            ),
            Field::Protocol => push(
                "protocols",
                term.values
                    .iter()
                    .map(|v| json!({ "value": v.to_lowercase(), "type": "Exact" }))
                    .collect(),
            ),
            Field::Verdict => push(
                "actions",
                term.verdicts()
                    .unwrap_or_default()
                    .iter()
                    .flat_map(|v| match v {
                        // A flow passed by every tier is allowed by the profile.
                        Verdict::Forwarded => vec![json!("Allow"), json!("Pass")],
                        _ => vec![json!("Deny")],
                    })
                    .collect(),
            ),
            _ => {}
        }
    }
    Some(serde_json::Value::Object(out))
}

// ----- Mapping -----

#[derive(Deserialize)]
struct Record {
    start_time: Option<String>,
    end_time: Option<String>,
    action: Option<String>,
    #[serde(default)]
    source_name: String,
    #[serde(default)]
    source_namespace: String,
    #[serde(default)]
    source_labels: String,
    #[serde(default)]
    dest_name: String,
    #[serde(default)]
    dest_namespace: String,
    #[serde(default)]
    dest_labels: String,
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    dest_port: i64,
    #[serde(default)]
    reporter: String,
    #[serde(default)]
    policies: Trace,
    #[serde(default)]
    packets_in: u64,
    #[serde(default)]
    packets_out: u64,
    #[serde(default)]
    bytes_in: u64,
    #[serde(default)]
    bytes_out: u64,
}

#[derive(Default, Deserialize)]
struct Trace {
    #[serde(default)]
    enforced: Vec<Hit>,
    #[serde(default)]
    pending: Vec<Hit>,
}

#[derive(Clone, Deserialize)]
struct Hit {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    namespace: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    action: String,
    trigger: Option<Box<Hit>>,
}

impl Hit {
    fn policy(&self) -> PolicyRef {
        PolicyRef {
            kind: self.kind.as_str().into(),
            namespace: (!self.namespace.is_empty()).then(|| self.namespace.as_str().into()),
            name: self.name.as_str().into(),
            tier: (!self.tier.is_empty()).then(|| self.tier.as_str().into()),
        }
    }

    /// `web-guard (CalicoNetworkPolicy, tier default): Deny`.
    fn describe(&self) -> String {
        let name = if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.namespace, self.name)
        };
        let mut text = if name.is_empty() {
            self.kind.clone()
        } else {
            format!("{name} ({}", self.kind)
        };
        if !name.is_empty() {
            if !self.tier.is_empty() {
                text.push_str(&format!(", tier {}", self.tier));
            }
            text.push(')');
        }
        if let Some(trigger) = &self.trigger {
            text.push_str(&format!(
                " triggered by {}/{} ({})",
                trigger.namespace, trigger.name, trigger.kind
            ));
        }
        format!("{text}: {}", self.action)
    }
}

/// `app=web | pod-template-hash=574ff6d9fd | projectcalico.org/…` → the plain labels.
fn labels(raw: &str) -> Vec<Text> {
    raw.split('|')
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("projectcalico.org/"))
        .map(Text::from)
        .collect()
}

fn endpoint(name: &str, namespace: &str, labels_raw: &str, port: Option<u16>) -> Endpoint {
    let labels = labels(labels_raw);
    if namespace.is_empty() || namespace == "-" {
        let kind = match name {
            "pub" => EndpointKind::World,
            "pvt" | "" => EndpointKind::Unknown,
            _ => EndpointKind::Host,
        };
        return Endpoint {
            kind,
            node: (kind == EndpointKind::Host).then(|| name.into()),
            names: Vec::new(),
            port,
            labels,
            ..Endpoint::default()
        };
    }
    // `scraper-655c844475-*`: the ReplicaSet's pods; its Deployment when the hash matches.
    let workload = name.strip_suffix("-*").map(|replica_set| {
        let hash = labels
            .iter()
            .find_map(|l| l.strip_prefix("pod-template-hash="))
            .map(str::to_string);
        match hash.and_then(|h| {
            replica_set
                .strip_suffix(&format!("-{h}"))
                .map(str::to_string)
        }) {
            Some(deployment) => Workload {
                kind: "Deployment".into(),
                name: deployment.into(),
            },
            None => Workload {
                kind: "ReplicaSet".into(),
                name: replica_set.into(),
            },
        }
    });
    Endpoint {
        kind: EndpointKind::Pod,
        namespace: Some(namespace.into()),
        pod: Some(name.into()),
        workload,
        port,
        labels,
        ..Endpoint::default()
    }
}

fn time(raw: &Option<String>) -> Option<Timestamp> {
    raw.as_deref()?.parse().ok()
}

fn map_record(record: Record) -> Option<Flow> {
    let end = time(&record.end_time)?;
    let mut flow = Flow::new(end);
    flow.start = time(&record.start_time);
    let port = u16::try_from(record.dest_port).ok().filter(|p| *p > 0);
    flow.source = endpoint(
        &record.source_name,
        &record.source_namespace,
        &record.source_labels,
        None,
    );
    flow.destination = endpoint(
        &record.dest_name,
        &record.dest_namespace,
        &record.dest_labels,
        port,
    );
    flow.protocol = Protocol::from_name(&record.protocol);
    flow.direction = match record.reporter.as_str() {
        "Src" => Direction::Egress,
        "Dst" => Direction::Ingress,
        _ => Direction::Unknown,
    };
    let action = record.action.clone().unwrap_or_default();
    flow.verdict = match action.as_str() {
        "Allow" | "Pass" => Verdict::Forwarded,
        "Deny" => Verdict::Dropped,
        _ => Verdict::Unknown,
    };
    flow.bytes = Some(record.bytes_in + record.bytes_out);
    flow.packets = Some(record.packets_in + record.packets_out);
    flow.event = Some(
        format!(
            "15 s aggregate · reported by {}",
            if record.reporter == "Src" {
                "the source"
            } else {
                "the destination"
            }
        )
        .into(),
    );
    let mut policies = Policies::default();
    let deciding = record
        .policies
        .enforced
        .iter()
        .rev()
        .find(|hit| hit.action == action)
        .or_else(|| record.policies.enforced.last());
    match (flow.verdict, deciding) {
        (Verdict::Dropped, Some(hit)) if hit.kind == "EndOfTier" => {
            policies.isolated = true;
            policies.isolated_by = hit.trigger.iter().map(|t| t.policy()).collect();
            policies.reason = Some(format!("end of tier {}", hit.tier).into());
        }
        (Verdict::Dropped, Some(hit)) => policies.denied_by = vec![hit.policy()],
        (Verdict::Forwarded, _) => {
            policies.allowed_by = record
                .policies
                .enforced
                .iter()
                .filter(|hit| hit.action == "Allow")
                .map(Hit::policy)
                .collect();
        }
        _ => {}
    }
    flow.policies = policies;
    flow.raw = vec![
        ("action".into(), action),
        ("reporter".into(), record.reporter.clone()),
        ("start_time".into(), record.start_time.unwrap_or_default()),
        ("end_time".into(), record.end_time.unwrap_or_default()),
        (
            "source".into(),
            format!("{}/{}", record.source_namespace, record.source_name),
        ),
        ("source_labels".into(), record.source_labels),
        (
            "dest".into(),
            format!("{}/{}", record.dest_namespace, record.dest_name),
        ),
        ("dest_labels".into(), record.dest_labels),
        (
            "packets_in / out".into(),
            format!("{} / {}", record.packets_in, record.packets_out),
        ),
        (
            "bytes_in / out".into(),
            format!("{} / {}", record.bytes_in, record.bytes_out),
        ),
    ];
    for (i, hit) in record.policies.enforced.iter().enumerate() {
        flow.raw
            .push((format!("enforced[{i}]").into(), hit.describe()));
    }
    for (i, hit) in record.policies.pending.iter().enumerate() {
        flow.raw
            .push((format!("pending[{i}]").into(), hit.describe()));
    }
    flow.raw.retain(|(_, v)| !v.is_empty());
    Some(flow)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records as the spike's Calico 3.32.2 cluster sent them.
    const DENIED: &str = r#"{"start_time":"2026-09-27T11:32:15Z","end_time":"2026-09-27T11:32:30Z","action":"Deny","source_name":"scraper-655c844475-*","source_namespace":"storefront","source_labels":"app=scraper | pod-template-hash=655c844475 | projectcalico.org/namespace=storefront | team=storefront","dest_name":"web-574ff6d9fd-*","dest_namespace":"storefront","dest_labels":"app=web | pod-template-hash=574ff6d9fd | team=storefront","protocol":"tcp","dest_port":80,"reporter":"Dst","policies":{"enforced":[{"kind":"CalicoNetworkPolicy","name":"web-guard","namespace":"storefront","tier":"default","action":"Deny","policy_index":0,"rule_index":0,"trigger":null}],"pending":[]},"packets_in":8,"packets_out":0,"bytes_in":592,"bytes_out":0}"#;
    const ISOLATED: &str = r#"{"start_time":"2026-09-27T11:32:15Z","end_time":"2026-09-27T11:32:30Z","action":"Deny","source_name":"shopper-6fd84cfbb4-*","source_namespace":"storefront","source_labels":"app=shopper | pod-template-hash=6fd84cfbb4","dest_name":"ledger-api-5cd68f8d6c-*","dest_namespace":"payments","dest_labels":"app=ledger-api | pod-template-hash=5cd68f8d6c","protocol":"tcp","dest_port":80,"reporter":"Dst","policies":{"enforced":[{"kind":"EndOfTier","name":"","namespace":"","tier":"default","action":"Deny","policy_index":0,"rule_index":-1,"trigger":{"kind":"NetworkPolicy","name":"ledger-api-isolation","namespace":"payments","tier":"default","action":"ActionUnspecified","policy_index":0,"rule_index":0,"trigger":null}}],"pending":[]},"packets_in":30,"packets_out":0,"bytes_in":2220,"bytes_out":0}"#;
    const ALLOWED: &str = r#"{"start_time":"2026-09-27T11:32:00Z","end_time":"2026-09-27T11:32:15Z","action":"Allow","source_name":"scraper-655c844475-*","source_namespace":"storefront","source_labels":"app=scraper","dest_name":"coredns-559f6c778d-*","dest_namespace":"kube-system","dest_labels":"k8s-app=kube-dns","protocol":"udp","dest_port":53,"reporter":"Dst","policies":{"enforced":[{"kind":"CalicoNetworkPolicy","name":"calico-system.cluster-dns","namespace":"kube-system","tier":"calico-system","action":"Pass","policy_index":0,"rule_index":1,"trigger":null},{"kind":"Profile","name":"kns.kube-system","namespace":"","tier":"","action":"Allow","policy_index":1,"rule_index":0,"trigger":null}],"pending":[]},"packets_in":2,"packets_out":2,"bytes_in":156,"bytes_out":297}"#;

    fn map(json: &str) -> Flow {
        map_record(serde_json::from_str(json).unwrap()).unwrap()
    }

    #[test]
    fn deny_rule_isolation_and_profiles() {
        let denied = map(DENIED);
        assert_eq!(denied.verdict, Verdict::Dropped);
        assert_eq!(
            denied.policies.summary(denied.verdict).text(),
            "denied by storefront/web-guard"
        );
        assert_eq!(
            denied.source.workload.as_ref().unwrap().name.as_ref(),
            "scraper"
        );
        assert_eq!(denied.source.pod.as_deref(), Some("scraper-655c844475-*"));
        assert_eq!(denied.destination.port, Some(80));
        assert_eq!((denied.bytes, denied.packets), (Some(592), Some(8)));
        assert_eq!(denied.direction, Direction::Ingress);
        assert!(
            denied
                .source
                .labels
                .iter()
                .all(|l| !l.starts_with("projectcalico.org/"))
        );

        let isolated = map(ISOLATED);
        assert!(isolated.policies.isolated);
        assert_eq!(
            isolated.policies.summary(isolated.verdict).text(),
            "isolated by payments/ledger-api-isolation"
        );
        assert!(
            FlowFilter::parse("policy=ledger-api-isolation")
                .unwrap()
                .matches(&isolated)
        );

        let allowed = map(ALLOWED);
        assert_eq!(allowed.verdict, Verdict::Forwarded);
        assert_eq!(allowed.protocol, Protocol::Udp);
        assert_eq!(
            allowed.policies.summary(allowed.verdict).text(),
            "kns.kube-system"
        );
        assert!(allowed.raw.iter().any(|(k, v)| k.as_ref() == "enforced[0]"
            && v.contains("calico-system.cluster-dns")
            && v.ends_with("Pass")));
    }

    #[test]
    fn networks_and_hosts() {
        let world = endpoint("pub", "", "", Some(443));
        assert_eq!(world.kind, EndpointKind::World);
        let host = endpoint("kind-worker", "", "", None);
        assert_eq!(
            (host.kind, host.node.as_deref()),
            (EndpointKind::Host, Some("kind-worker"))
        );
    }

    #[test]
    fn filters_push_down() {
        let filter = FlowFilter::parse("src.ns=storefront dst.workload=web dst.port=80 proto=tcp verdict=dropped ns=payments policy=web-guard").unwrap();
        let pushed = pushdown(&filter);
        assert_eq!(
            pushed.canonical(),
            "src.ns=storefront dst.workload=web dst.port=80 proto=tcp verdict=dropped"
        );
        let json = to_filters(&pushed).unwrap();
        assert_eq!(json["source_namespaces"][0]["value"], "storefront");
        assert_eq!(json["dest_names"][0]["type"], "Fuzzy");
        assert_eq!(json["dest_ports"][0]["value"], 80);
        assert_eq!(json["protocols"][0]["value"], "tcp");
        assert_eq!(json["actions"], serde_json::json!(["Deny"]));
        let forwarded =
            to_filters(&pushdown(&FlowFilter::parse("verdict=forwarded").unwrap())).unwrap();
        assert_eq!(forwarded["actions"], serde_json::json!(["Allow", "Pass"]));
        assert!(to_filters(&FlowFilter::default()).is_none());
        assert!(pushdown(&FlowFilter::parse("src.port=80 verdict=no-reply").unwrap()).is_empty());
    }
}
