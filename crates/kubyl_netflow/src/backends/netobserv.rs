//! NetObserv (README "Flow transports"): its eBPF agents push to flowlogs-pipeline, which writes
//! flow records to Loki and metrics to Prometheus. There's no pull API for single flows, so the
//! table polls Loki's `query_range` through the API server's service proxy every few seconds,
//! and the topology can come from the `netobserv_*` metrics without Loki.
//!
//! Namespace, owner, type and direction are Loki stream labels; the JSON line has addresses,
//! ports, protocol, bytes, packets, TCP flags, pod and node names, times and drop fields.
//! NetObserv sees drops only where the kernel reports them; on CNIs that drop through
//! NetworkPolicies without reporting it (kindnet), an isolated call is a TCP attempt that never
//! got past `SYN`: "no reply". OVN-Kubernetes' network events name policies.

use std::collections::{BTreeMap, HashSet};
use std::net::IpAddr;
use std::time::Duration;

use jiff::Timestamp;
use kubyl_metrics::prometheus::PromClient;
use kubyl_metrics::transport::Transport;
use serde_json::Value;

use crate::aggregate::{self, Topology, Zoom};
use crate::detect::ServiceTarget;
use crate::filter::{Field, FlowFilter, Op, Side, Term};
use crate::model::{
    Direction, Endpoint, EndpointKind, Flow, L7, Policies, PolicyRef, Protocol, Text, Verdict,
    Workload,
};
use crate::provider::{
    BackendKind, BackendStatus, Batcher, Capabilities, FlowProvider, FlowSink, ProviderError,
    ProviderFuture, StreamEvent, StreamQuery,
};

/// How often Loki is asked for new records.
pub const POLL: Duration = Duration::from_secs(5);
/// Records per Loki request.
const LIMIT: usize = 5_000;
/// NetObserv's stream selector.
const SELECTOR: &str = r#"app="netobserv-flowcollector""#;

/// How Kubyl reaches NetObserv's Loki.
#[derive(Clone)]
pub enum LokiAccess {
    Ready {
        transport: Transport,
        label: String,
    },
    /// Why there are no single flows (Loki off, a LokiStack gateway…).
    Missing(String),
}

pub struct NetObserv {
    pub loki: LokiAccess,
    pub prometheus: Option<PromClient>,
}

impl NetObserv {
    pub fn loki_target(client: kube::Client, target: &ServiceTarget) -> LokiAccess {
        LokiAccess::Ready {
            transport: Transport::service_proxy(
                client,
                &target.namespace,
                &target.service,
                &target.port,
                &target.scheme,
                &target.path,
            ),
            label: format!(
                "Loki {} through the API server's service proxy",
                target.label()
            ),
        }
    }
}

fn loki_error(err: kubyl_metrics::transport::PromError, label: &str) -> ProviderError {
    match err {
        kubyl_metrics::transport::PromError::Http(403, _) => ProviderError::Other(format!(
            "{label}: forbidden (needs get services/proxy in Loki's namespace)"
        )),
        other => ProviderError::Unavailable(format!("{label}: {other}")),
    }
}

impl FlowProvider for NetObserv {
    fn kind(&self) -> BackendKind {
        BackendKind::NetObserv
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            names_allows: false,
            names_denies: true,
            names_isolation: false,
            live: false,
            history: "Loki's retention",
            bytes: true,
            l7: false,
            aggregated: true,
            single_flows: matches!(self.loki, LokiAccess::Ready { .. }),
            graph_from_metrics: self.prometheus.is_some(),
        }
    }

    fn pushdown(&self, filter: &FlowFilter) -> FlowFilter {
        pushdown(filter)
    }

    fn probe(&self) -> ProviderFuture<Result<BackendStatus, ProviderError>> {
        let loki = self.loki.clone();
        let prometheus = self.prometheus.clone();
        Box::pin(async move {
            let mut notes = Vec::new();
            let mut via = Vec::new();
            match &loki {
                LokiAccess::Ready { transport, label } => {
                    transport
                        .get("/loki/api/v1/labels", &[])
                        .await
                        .map_err(|e| loki_error(e, label))?;
                    via.push(label.clone());
                }
                LokiAccess::Missing(reason) => notes.push(reason.clone()),
            }
            if let Some(prometheus) = &prometheus {
                match prometheus
                    .query("count(netobserv_workload_ingress_bytes_total)")
                    .await
                {
                    Ok(samples) if !samples.is_empty() => via.push(format!(
                        "metrics from Prometheus {}",
                        prometheus.target().label()
                    )),
                    Ok(_) => notes.push(format!(
                        "Prometheus {} has no NetObserv metrics.",
                        prometheus.target().label()
                    )),
                    Err(err) => notes.push(format!(
                        "Prometheus {} didn't answer: {err}.",
                        prometheus.target().label()
                    )),
                }
            }
            if via.is_empty() {
                return Err(ProviderError::Unavailable(
                    notes.join(" ").trim().to_string(),
                ));
            }
            Ok(BackendStatus {
                endpoint: "FlowCollector cluster".into(),
                via: via.join("; "),
                version: None,
                nodes: None,
                buffered: None,
                notes,
            })
        })
    }

    fn stream(
        &self,
        query: StreamQuery,
        sink: FlowSink,
    ) -> ProviderFuture<Result<(), ProviderError>> {
        let loki = self.loki.clone();
        Box::pin(async move {
            let mut batcher = Batcher::new(sink.clone());
            let LokiAccess::Ready { transport, label } = loki else {
                // No single flows: say so once and hold the stream until nobody listens.
                if batcher.send(StreamEvent::CaughtUp).await.is_err() {
                    return Ok(());
                }
                while !sink.is_closed() {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                return Ok(());
            };
            let logql = logql(&query.filter);
            let now = Timestamp::now();
            let start = query
                .since
                .unwrap_or_else(|| now - jiff::SignedDuration::from_mins(5));
            // History: the newest records of the window.
            let (mut records, _) =
                fetch(&transport, &label, &logql, start, now, "backward").await?;
            records.sort_by_key(|r| r.0);
            let mut cursor = records.last().map(|r| r.0).unwrap_or(nanos(now));
            let mut boundary: HashSet<u64> = records
                .iter()
                .filter(|r| r.0 == cursor)
                .map(|r| r.2)
                .collect();
            for (_, flow, _) in records {
                if batcher.push(flow).await.is_err() {
                    return Ok(());
                }
            }
            if batcher.send(StreamEvent::CaughtUp).await.is_err() {
                return Ok(());
            }
            loop {
                tokio::time::sleep(POLL).await;
                loop {
                    let end = Timestamp::now();
                    let Ok(start) = Timestamp::from_nanosecond(i128::from(cursor)) else {
                        break;
                    };
                    let (records, full) =
                        fetch(&transport, &label, &logql, start, end, "forward").await?;
                    for (ns, flow, hash) in records {
                        // Records at the cursor were sent last time.
                        if ns == cursor && boundary.contains(&hash) {
                            continue;
                        }
                        if ns > cursor {
                            cursor = ns;
                            boundary.clear();
                        }
                        boundary.insert(hash);
                        if batcher.push(flow).await.is_err() {
                            return Ok(());
                        }
                    }
                    if batcher.flush().await.is_err() {
                        return Ok(());
                    }
                    if !full {
                        break;
                    }
                }
            }
        })
    }

    fn graph(
        &self,
        window: Duration,
        zoom: Zoom,
        filter: FlowFilter,
    ) -> ProviderFuture<Result<Option<Topology>, ProviderError>> {
        let prometheus = self.prometheus.clone();
        Box::pin(async move {
            let Some(prometheus) = prometheus else {
                return Ok(None);
            };
            let by = match zoom {
                // The type keeps nodes apart from the world outside.
                Zoom::Namespaces => "SrcK8S_Namespace, SrcK8S_Type, DstK8S_Namespace, DstK8S_Type",
                Zoom::Workloads => {
                    "SrcK8S_Namespace, SrcK8S_OwnerName, SrcK8S_Type, DstK8S_Namespace, DstK8S_OwnerName, DstK8S_Type"
                }
            };
            let promql = format!(
                "sum by ({by}) (increase(netobserv_workload_ingress_bytes_total[{}s]))",
                window.as_secs().max(60)
            );
            let samples = prometheus.query(&promql).await.map_err(|e| {
                ProviderError::Unavailable(format!(
                    "NetObserv's metrics in Prometheus {}: {e}",
                    prometheus.target().label()
                ))
            })?;
            let flows: Vec<Flow> = samples
                .into_iter()
                .filter(|s| s.value >= 1.0)
                .map(|s| metric_flow(&s.labels, s.value))
                .filter(|f| filter.matches(f))
                .collect();
            let mut topology = aggregate::aggregate(&flows, zoom, aggregate::NODE_LIMIT);
            topology.bytes_only = true;
            Ok(Some(topology))
        })
    }
}

fn nanos(time: Timestamp) -> u64 {
    u64::try_from(time.as_nanosecond()).unwrap_or(0)
}

/// Records `(timestamp ns, flow, hash of the line)` between `start` and `end`, and whether the
/// answer hit the limit.
async fn fetch(
    transport: &Transport,
    label: &str,
    logql: &str,
    start: Timestamp,
    end: Timestamp,
    direction: &str,
) -> Result<(Vec<(u64, Flow, u64)>, bool), ProviderError> {
    let body = transport
        .get(
            "/loki/api/v1/query_range",
            &[
                ("query", logql.to_string()),
                ("start", nanos(start).to_string()),
                ("end", nanos(end).to_string()),
                ("limit", LIMIT.to_string()),
                ("direction", direction.to_string()),
            ],
        )
        .await
        .map_err(|e| loki_error(e, label))?;
    let mut out = Vec::new();
    for stream in body
        .pointer("/data/result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let labels: BTreeMap<String, String> = stream
            .get("stream")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        for value in stream
            .get("values")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(ns), Some(line)) = (
                value
                    .get(0)
                    .and_then(Value::as_str)
                    .and_then(|n| n.parse::<u64>().ok()),
                value.get(1).and_then(Value::as_str),
            ) else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(flow) = map_record(&labels, &record) {
                out.push((ns, flow, hash(line)));
            }
        }
    }
    let full = out.len() >= LIMIT;
    Ok((out, full))
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

// ----- Pushdown -----

fn pushable(term: &Term) -> bool {
    if term.op != Op::Eq || term.has_glob() {
        return false;
    }
    match term.field {
        Field::Namespace => true,
        Field::Workload => term.values.iter().all(|v| v.matches('/').count() <= 1),
        Field::Direction => term
            .directions()
            .is_some_and(|d| d.iter().all(|d| *d != Direction::Unknown)),
        _ => false,
    }
}

/// Namespaces and owners (either side too, as a label filter with `or`) and the direction.
pub fn pushdown(filter: &FlowFilter) -> FlowFilter {
    filter.subset(pushable)
}

fn regex_escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push_str("\\\\");
        }
        out.push(c);
    }
    out
}

/// `label=~"^(a|b)$"`.
fn matcher(label: &str, values: &[String]) -> String {
    let alternatives: Vec<String> = values.iter().map(|v| regex_escape(v)).collect();
    format!(r#"{label}=~"^({})$""#, alternatives.join("|"))
}

/// The LogQL query of a pushed-down filter.
pub fn logql(filter: &FlowFilter) -> String {
    let mut selector = vec![SELECTOR.to_string()];
    let mut expressions: Vec<String> = Vec::new();
    for term in &filter.terms {
        match term.field {
            Field::Namespace | Field::Workload => {
                let side_matchers = |prefix: &str| -> Vec<String> {
                    if term.field == Field::Namespace {
                        return vec![matcher(&format!("{prefix}K8S_Namespace"), &term.values)];
                    }
                    // `ns/name` puts the namespace next to the owner.
                    let mut owners = Vec::new();
                    let mut namespaces = Vec::new();
                    for value in &term.values {
                        match value.split_once('/') {
                            Some((ns, name)) => {
                                namespaces.push(ns.to_string());
                                owners.push(name.to_string());
                            }
                            None => owners.push(value.clone()),
                        }
                    }
                    let mut out = vec![matcher(&format!("{prefix}K8S_OwnerName"), &owners)];
                    if !namespaces.is_empty() && namespaces.len() == owners.len() {
                        out.push(matcher(&format!("{prefix}K8S_Namespace"), &namespaces));
                    }
                    out
                };
                match term.side {
                    Side::Source => selector.extend(side_matchers("Src")),
                    Side::Destination => selector.extend(side_matchers("Dst")),
                    Side::Either => expressions.push(format!(
                        "({} or {})",
                        side_matchers("Src").join(" and "),
                        side_matchers("Dst").join(" and ")
                    )),
                }
            }
            Field::Direction => {
                let values: Vec<String> = term
                    .directions()
                    .unwrap_or_default()
                    .iter()
                    .map(|d| if *d == Direction::Ingress { "0" } else { "1" }.to_string())
                    .collect();
                selector.push(matcher("FlowDirection", &values));
            }
            _ => {}
        }
    }
    let mut query = format!("{{{}}}", selector.join(", "));
    for expression in expressions {
        query.push_str(" | ");
        query.push_str(&expression);
    }
    query
}

// ----- Mapping -----

fn string(labels: &BTreeMap<String, String>, record: &Value, key: &str) -> Option<String> {
    labels
        .get(key)
        .cloned()
        .or_else(|| record.get(key).and_then(Value::as_str).map(str::to_string))
        .filter(|v| !v.is_empty())
}

fn number(record: &Value, key: &str) -> Option<u64> {
    let value = record.get(key)?;
    value.as_u64().or_else(|| value.as_f64().map(|f| f as u64))
}

/// A private or special address (not the internet).
fn private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1])
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

fn endpoint(
    labels: &BTreeMap<String, String>,
    record: &Value,
    side: &str,
    port: Option<u16>,
) -> Endpoint {
    let get = |key: &str| string(labels, record, &format!("{side}{key}"));
    let ip = record
        .get(format!("{side}Addr"))
        .and_then(Value::as_str)
        .and_then(|a| a.parse::<IpAddr>().ok());
    let namespace = get("K8S_Namespace");
    let name = get("K8S_Name");
    let owner = get("K8S_OwnerName");
    let kind_name = get("K8S_Type").unwrap_or_default();
    let node = get("K8S_HostName");
    let mut endpoint = Endpoint {
        ip,
        port,
        node: node.map(Text::from),
        ..Endpoint::default()
    };
    match kind_name.as_str() {
        "Pod" => {
            endpoint.kind = EndpointKind::Pod;
            endpoint.namespace = namespace.map(Text::from);
            endpoint.pod = name.map(Text::from);
            endpoint.workload = owner.map(|owner| Workload {
                kind: get("K8S_OwnerType").unwrap_or_else(|| "Pod".into()).into(),
                name: owner.into(),
            });
        }
        "Service" => {
            endpoint.kind = EndpointKind::Service;
            endpoint.namespace = namespace.map(Text::from);
            endpoint.service = name.or(owner).map(Text::from);
        }
        "Node" => {
            endpoint.kind = EndpointKind::Host;
            endpoint.node = name.or(owner).map(Text::from).or(endpoint.node);
        }
        _ => {
            endpoint.namespace = namespace.map(Text::from);
            endpoint.kind = match (&endpoint.namespace, &ip) {
                (Some(_), _) => EndpointKind::Pod,
                (None, Some(ip)) if !private(ip) => EndpointKind::World,
                _ => EndpointKind::Unknown,
            };
        }
    }
    endpoint
}

/// TCP flags as names: NetObserv sends a list (`["SYN","ACK"]`) or a bit field.
fn tcp_flags(record: &Value) -> Vec<String> {
    match record.get("Flags") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::Number(n)) => {
            let bits = n.as_u64().unwrap_or(0);
            [
                (1, "FIN"),
                (2, "SYN"),
                (4, "RST"),
                (8, "PSH"),
                (16, "ACK"),
                (32, "URG"),
                (64, "ECE"),
                (128, "CWR"),
                (256, "SYN_ACK"),
                (512, "FIN_ACK"),
                (1024, "RST_ACK"),
            ]
            .iter()
            .filter(|(bit, _)| bits & bit != 0)
            .map(|(_, name)| name.to_string())
            .collect()
        }
        _ => Vec::new(),
    }
}

/// One Loki record (stream labels and JSON line).
pub fn map_record(labels: &BTreeMap<String, String>, record: &Value) -> Option<Flow> {
    let end = number(record, "TimeFlowEndMs")
        .or_else(|| number(record, "TimeReceived").map(|s| s * 1000))?;
    let time = Timestamp::from_millisecond(i64::try_from(end).ok()?).ok()?;
    let mut flow = Flow::new(time);
    flow.start = number(record, "TimeFlowStartMs")
        .and_then(|ms| i64::try_from(ms).ok())
        .and_then(|ms| Timestamp::from_millisecond(ms).ok());
    let port = |key: &str| {
        number(record, key)
            .and_then(|p| u16::try_from(p).ok())
            .filter(|p| *p > 0)
    };
    flow.source = endpoint(labels, record, "Src", port("SrcPort"));
    flow.destination = endpoint(labels, record, "Dst", port("DstPort"));
    flow.protocol = number(record, "Proto").map_or(Protocol::Unknown, Protocol::from_number);
    flow.direction = match string(labels, record, "FlowDirection").as_deref() {
        Some("0") => Direction::Ingress,
        Some("1") => Direction::Egress,
        _ => Direction::Unknown,
    };
    flow.bytes = number(record, "Bytes");
    flow.packets = number(record, "Packets");
    flow.node = record
        .get("AgentIP")
        .and_then(Value::as_str)
        .map(Text::from);
    let flags = tcp_flags(record);
    if !flags.is_empty() {
        flow.tcp_flags = Some(flags.join(", ").into());
    }
    flow.verdict = Verdict::Forwarded;
    let dropped = number(record, "PktDropPackets").unwrap_or(0);
    let cause = record
        .get("PktDropLatestDropCause")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());
    let mut policies = Policies::default();
    if dropped > 0 && flow.packets.is_some_and(|p| dropped >= p) {
        flow.verdict = Verdict::Dropped;
        policies.reason = cause.map(Text::from);
    }
    // A TCP attempt that never got an answer.
    if flow.verdict == Verdict::Forwarded
        && flow.protocol == Protocol::Tcp
        && flags.iter().any(|f| f == "SYN")
        && !flags.iter().any(|f| f.contains("ACK"))
    {
        flow.verdict = Verdict::NoReply;
    }
    // OVN-Kubernetes' network events name the policy behind a verdict.
    for event in record
        .get("NetworkEvents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |key: &str| event.get(key).and_then(Value::as_str).unwrap_or_default();
        let name = field("Name");
        if name.is_empty() {
            continue;
        }
        let policy = PolicyRef {
            kind: if field("Type").is_empty() {
                "NetworkPolicy"
            } else {
                field("Type")
            }
            .into(),
            namespace: (!field("Namespace").is_empty()).then(|| field("Namespace").into()),
            name: name.into(),
            tier: None,
        };
        match field("Action").to_ascii_lowercase().as_str() {
            "drop" | "deny" => {
                flow.verdict = Verdict::Dropped;
                policies.denied_by.push(policy);
            }
            "allow" | "allow-related" | "pass" => policies.allowed_by.push(policy),
            _ => {}
        }
    }
    flow.policies = policies;
    if let Some(query) = record
        .get("DnsName")
        .and_then(Value::as_str)
        .filter(|q| !q.is_empty())
    {
        flow.l7 = Some(L7::Dns {
            query: query.trim_end_matches('.').to_string(),
            rcode: record
                .get("DnsFlagsResponseCode")
                .and_then(Value::as_str)
                .map(Text::from),
            answers: Vec::new(),
            response: true,
        });
    }
    flow.event = Some("flow record".into());
    flow.raw = raw_fields(labels, record);
    Some(flow)
}

/// The record's own fields for the detail panel.
fn raw_fields(labels: &BTreeMap<String, String>, record: &Value) -> Vec<(Text, String)> {
    let mut out: Vec<(Text, String)> = labels
        .iter()
        .filter(|(k, _)| k.as_str() != "app")
        .map(|(k, v)| (Text::from(k.as_str()), v.clone()))
        .collect();
    if let Some(object) = record.as_object() {
        for (key, value) in object {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            if text.is_empty() || text == "[\"\"]" {
                continue;
            }
            out.push((key.as_str().into(), text));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A pseudo-flow for one metric sample (the graph goes through the same filter and
/// aggregation as flows).
fn metric_flow(labels: &BTreeMap<String, String>, bytes: f64) -> Flow {
    let side = |prefix: &str| {
        let get = |key: &str| {
            labels
                .get(&format!("{prefix}{key}"))
                .filter(|v| !v.is_empty())
        };
        let namespace = get("K8S_Namespace").map(|n| Text::from(n.as_str()));
        let owner = get("K8S_OwnerName").map(|o| Text::from(o.as_str()));
        let kind = get("K8S_Type").map(String::as_str);
        Endpoint {
            kind: match (kind, &namespace) {
                (Some("Node"), _) => EndpointKind::Host,
                (Some("Service"), _) => EndpointKind::Service,
                (_, Some(_)) => EndpointKind::Pod,
                (_, None) if owner.is_some() => EndpointKind::Host,
                _ => EndpointKind::World,
            },
            namespace: namespace.clone(),
            workload: owner
                .clone()
                .filter(|_| namespace.is_some())
                .map(|name| Workload {
                    kind: "Workload".into(),
                    name,
                }),
            service: (kind == Some("Service")).then(|| owner.clone()).flatten(),
            node: (namespace.is_none()).then_some(owner).flatten(),
            ..Endpoint::default()
        }
    };
    let mut flow = Flow::new(Timestamp::now());
    flow.source = side("Src");
    flow.destination = side("Dst");
    flow.verdict = Verdict::Forwarded;
    flow.bytes = Some(bytes as u64);
    flow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Records as the spike's NetObserv 2.0 cluster wrote them.
    #[test]
    fn records_from_loki() {
        let stream = labels(&[
            ("app", "netobserv-flowcollector"),
            ("SrcK8S_Namespace", "storefront"),
            ("SrcK8S_OwnerName", "shopper"),
            ("SrcK8S_Type", "Pod"),
            ("DstK8S_Namespace", "payments"),
            ("DstK8S_OwnerName", "ledger-api"),
            ("DstK8S_Type", "Service"),
            ("FlowDirection", "1"),
            ("K8S_FlowLayer", "app"),
        ]);
        let line = serde_json::json!({
            "SrcAddr": "10.244.1.39", "DstAddr": "10.96.186.230", "SrcPort": 51764, "DstPort": 80, "Proto": 6,
            "Bytes": 148, "Packets": 2, "Flags": ["SYN"], "SrcK8S_Name": "shopper-6fd84cfbb4-z7kvr",
            "SrcK8S_OwnerType": "Deployment", "DstK8S_Name": "ledger-api", "TimeFlowStartMs": 1790509255823_u64,
            "TimeFlowEndMs": 1790509270234_u64, "AgentIP": "172.22.0.7", "Udns": [""]
        });
        let flow = map_record(&stream, &line).unwrap();
        assert_eq!(flow.verdict, Verdict::NoReply);
        assert_eq!(flow.source.label(), "storefront/shopper-6fd84cfbb4-z7kvr");
        assert_eq!(
            flow.source.workload.as_ref().unwrap().name.as_ref(),
            "shopper"
        );
        assert_eq!(flow.destination.kind, EndpointKind::Service);
        assert_eq!(flow.destination.service.as_deref(), Some("ledger-api"));
        assert_eq!(flow.direction, Direction::Egress);
        assert_eq!((flow.bytes, flow.packets), (Some(148), Some(2)));
        assert_eq!(
            flow.policies.summary(flow.verdict).text(),
            "no answer: dropped or nothing listening"
        );
        assert!(
            FlowFilter::parse("verdict=no-reply dst.ns=payments src.pod=shopper")
                .unwrap()
                .matches(&flow)
        );
        assert!(
            !flow
                .raw
                .iter()
                .any(|(k, _)| k.as_ref() == "app" || k.as_ref() == "Udns")
        );

        // Answered, with a teardown packet dropped by the kernel: still forwarded.
        let answered = serde_json::json!({
            "SrcAddr": "10.244.1.39", "DstAddr": "10.244.1.41", "SrcPort": 1, "DstPort": 80, "Proto": 6,
            "Bytes": 543, "Packets": 7, "Flags": ["SYN", "ACK", "FIN_ACK"], "PktDropPackets": 1,
            "PktDropLatestDropCause": "SKB_DROP_UNKNOWN_CAUSE", "TimeFlowEndMs": 1790509270234_u64
        });
        let flow = map_record(&stream, &answered).unwrap();
        assert_eq!(flow.verdict, Verdict::Forwarded);
        assert!(
            flow.raw
                .iter()
                .any(|(k, v)| k.as_ref() == "PktDropLatestDropCause"
                    && v == "SKB_DROP_UNKNOWN_CAUSE")
        );

        // OVN network events name a denying policy.
        let ovn = serde_json::json!({
            "SrcAddr": "10.128.2.5", "DstAddr": "10.128.3.9", "DstPort": 8080, "Proto": 6, "Packets": 3,
            "Flags": 2, "TimeFlowEndMs": 1790509270234_u64,
            "NetworkEvents": [{"Action": "drop", "Type": "NetworkPolicy", "Feature": "acl", "Direction": "Ingress", "Name": "deny-all", "Namespace": "payments"}]
        });
        let flow = map_record(&stream, &ovn).unwrap();
        assert_eq!(flow.verdict, Verdict::Dropped);
        assert_eq!(
            flow.policies.summary(flow.verdict).text(),
            "denied by payments/deny-all"
        );
        assert_eq!(flow.tcp_flags.as_deref(), Some("SYN"));

        let world = map_record(&BTreeMap::new(), &serde_json::json!({"SrcAddr": "203.0.113.24", "DstAddr": "10.0.0.1", "TimeFlowEndMs": 1_u64})).unwrap();
        assert_eq!(world.source.kind, EndpointKind::World);
        assert_eq!(world.destination.kind, EndpointKind::Unknown);
    }

    #[test]
    fn logql_of_filters() {
        let filter = FlowFilter::parse("src.ns=storefront dst.workload=payments/ledger-api ns=kube-system,a.b dir=egress verdict=dropped").unwrap();
        let pushed = pushdown(&filter);
        assert_eq!(
            pushed.canonical(),
            "src.ns=storefront dst.workload=payments/ledger-api ns=kube-system,a.b dir=egress"
        );
        assert_eq!(
            logql(&pushed),
            r#"{app="netobserv-flowcollector", SrcK8S_Namespace=~"^(storefront)$", DstK8S_OwnerName=~"^(ledger-api)$", DstK8S_Namespace=~"^(payments)$", FlowDirection=~"^(1)$"} | (SrcK8S_Namespace=~"^(kube-system|a\\.b)$" or DstK8S_Namespace=~"^(kube-system|a\\.b)$")"#
        );
        assert_eq!(
            logql(&FlowFilter::default()),
            r#"{app="netobserv-flowcollector"}"#
        );
    }

    #[test]
    fn metric_samples_become_edges() {
        let flows: Vec<Flow> = [
            (
                labels(&[
                    ("SrcK8S_Namespace", "storefront"),
                    ("DstK8S_Namespace", "payments"),
                ]),
                5000.0,
            ),
            (
                labels(&[("SrcK8S_Namespace", ""), ("DstK8S_Namespace", "ingress")]),
                300.0,
            ),
        ]
        .iter()
        .map(|(l, v)| metric_flow(l, *v))
        .collect();
        let topology = aggregate::aggregate(&flows, Zoom::Namespaces, 10);
        assert_eq!(topology.nodes.len(), 4);
        assert_eq!(topology.edges[0].bytes, 5000);
        assert!(topology.nodes.iter().any(|n| n.id == "world"));
        assert!(FlowFilter::parse("ns=payments").unwrap().matches(&flows[0]));
    }
}
