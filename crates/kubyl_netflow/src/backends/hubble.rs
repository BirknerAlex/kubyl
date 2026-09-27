//! Cilium's Hubble Relay (README "Flow transports"): gRPC `Observer.GetFlows` over a temporary
//! loopback forward to the Relay Service (gRPC can't pass the API server's service proxy).
//!
//! Plain gRPC on the Service's port 80 by default; with server TLS (port 443) the certificate
//! (`*.hubble-relay.cilium.io`) is verified against a CA from a ConfigMap
//! (`cilium-root-ca.crt`, Cilium's `tls.caBundle`) or a file named in settings. A Relay that
//! wants client certificates is reported, not supported: those live in Secrets.
//!
//! Mapping (verified on Cilium 1.20.2): policies from `ingress/egress_allowed_by` and
//! `ingress/egress_denied_by`; `POLICY_DENIED` means isolated with nothing allowing the flow,
//! `POLICY_DENY` a deny rule. A policy-verdict event names the deny rule, the drop notification
//! of the same packet doesn't: the name is carried over. L7 URLs and headers are sanitized here.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::Duration;

use futures::StreamExt as _;
use jiff::Timestamp;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint as GrpcEndpoint};

use crate::filter::{Field, FlowFilter, Op, Side, Term};
use crate::model::{
    Direction, Endpoint, EndpointKind, Flow, L7, Policies, PolicyRef, Protocol, Text, Verdict,
    Workload,
};
use crate::proto::flow as pb;
use crate::proto::observer::observer_client::ObserverClient;
use crate::proto::observer::{GetFlowsRequest, ServerStatusRequest, get_flows_response};
use crate::provider::{
    BATCH_WAIT, BackendKind, BackendStatus, Batcher, Capabilities, FlowProvider, FlowSink,
    ProviderError, ProviderFuture, StreamEvent, StreamQuery,
};
use crate::sanitize;

/// The name Relay's certificate has (`*.hubble-relay.cilium.io`).
pub const RELAY_SERVER_NAME: &str = "relay.hubble-relay.cilium.io";

/// How long a policy-verdict event's names are kept for the matching drop notification.
const CARRY_FOR: Duration = Duration::from_secs(5);
const CARRY_ENTRIES: usize = 4_096;

/// Where Relay is and how it talks.
#[derive(Clone, PartialEq)]
pub struct HubbleTarget {
    pub namespace: String,
    pub service: String,
    /// The Service port the forward targets.
    pub port: u16,
    /// Server TLS: the CA (PEM) and the name to verify.
    pub tls: Option<(Vec<u8>, String)>,
}

impl std::fmt::Debug for HubbleTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubbleTarget")
            .field("namespace", &self.namespace)
            .field("service", &self.service)
            .field("port", &self.port)
            .field("tls", &self.tls.is_some())
            .finish()
    }
}

pub struct Hubble {
    pub target: HubbleTarget,
    /// The loopback port of the forward to Relay.
    pub local_port: u16,
    pub keep_query_values: bool,
}

impl Hubble {
    async fn client(&self) -> Result<ObserverClient<Channel>, ProviderError> {
        let scheme = if self.target.tls.is_some() {
            "https"
        } else {
            "http"
        };
        let unavailable = |e: tonic::transport::Error| {
            ProviderError::Unavailable(format!("Hubble Relay didn't answer: {e}"))
        };
        let mut endpoint =
            GrpcEndpoint::from_shared(format!("{scheme}://127.0.0.1:{}", self.local_port))
                .map_err(unavailable)?
                .connect_timeout(Duration::from_secs(10))
                .tcp_nodelay(true)
                .http2_keep_alive_interval(Duration::from_secs(30))
                .keep_alive_while_idle(true);
        if let Some((ca, name)) = &self.target.tls {
            endpoint = endpoint
                .tls_config(
                    ClientTlsConfig::new()
                        .ca_certificate(Certificate::from_pem(ca))
                        .domain_name(name.clone()),
                )
                .map_err(unavailable)?;
        }
        let channel = endpoint.connect().await.map_err(|e| {
            ProviderError::Unavailable(format!(
                "Hubble Relay didn't answer through the port-forward: {}",
                error_chain(&e)
            ))
        })?;
        // Flows can be large (L7 headers): allow 16 MiB messages.
        Ok(ObserverClient::new(channel).max_decoding_message_size(16 << 20))
    }

    fn endpoint_label(&self) -> String {
        format!("{}/{}", self.target.namespace, self.target.service)
    }
}

/// `error: cause: cause` (tonic's Display stops at the top).
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

fn status_error(status: &tonic::Status) -> ProviderError {
    match status.code() {
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => {
            ProviderError::Unavailable(format!("Hubble Relay is unavailable: {}", status.message()))
        }
        tonic::Code::Unauthenticated | tonic::Code::PermissionDenied => {
            ProviderError::Unsupported(format!(
                "Hubble Relay refused Kubyl ({}). A Relay that wants client certificates isn't supported: those are in Secrets, which Kubyl doesn't read.",
                status.message()
            ))
        }
        _ => ProviderError::Other(format!("Hubble Relay: {}", status.message())),
    }
}

impl FlowProvider for Hubble {
    fn kind(&self) -> BackendKind {
        BackendKind::Hubble
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            names_allows: true,
            names_denies: true,
            names_isolation: false,
            live: true,
            history: "Relay's buffer (a few thousand flows per node)",
            bytes: false,
            l7: true,
            aggregated: false,
            single_flows: true,
            graph_from_metrics: false,
        }
    }

    fn pushdown(&self, filter: &FlowFilter) -> FlowFilter {
        pushdown(filter)
    }

    fn probe(&self) -> ProviderFuture<Result<BackendStatus, ProviderError>> {
        let this = Hubble {
            target: self.target.clone(),
            local_port: self.local_port,
            keep_query_values: self.keep_query_values,
        };
        Box::pin(async move {
            let mut client = this.client().await?;
            let status = tokio::time::timeout(
                Duration::from_secs(20),
                client.server_status(ServerStatusRequest {}),
            )
            .await
            .map_err(|_| ProviderError::Unavailable("Hubble Relay didn't answer in 20 s".into()))?
            .map_err(|s| status_error(&s))?
            .into_inner();
            let mut notes = Vec::new();
            let unavailable = status.num_unavailable_nodes.unwrap_or(0);
            if unavailable > 0 {
                notes.push(format!(
                    "{unavailable} node{} not reachable from Relay: {}",
                    if unavailable == 1 { " is" } else { "s are" },
                    status.unavailable_nodes.join(", ")
                ));
            }
            let connected = status.num_connected_nodes.unwrap_or(0);
            Ok(BackendStatus {
                endpoint: this.endpoint_label(),
                via: format!(
                    "through a temporary port-forward (127.0.0.1:{}){}",
                    this.local_port,
                    if this.target.tls.is_some() {
                        ", TLS"
                    } else {
                        ""
                    }
                ),
                version: relay_version(&status.version),
                nodes: status
                    .num_connected_nodes
                    .map(|_| (connected, connected + unavailable)),
                buffered: Some((status.num_flows, status.max_flows)),
                notes,
            })
        })
    }

    fn stream(
        &self,
        query: StreamQuery,
        sink: FlowSink,
    ) -> ProviderFuture<Result<(), ProviderError>> {
        let this = Hubble {
            target: self.target.clone(),
            local_port: self.local_port,
            keep_query_values: self.keep_query_values,
        };
        Box::pin(async move {
            let mut client = this.client().await?;
            let (whitelist, blacklist) = to_filters(&query.filter);
            let request = GetFlowsRequest {
                follow: true,
                since: query.since.map(to_proto_time),
                whitelist,
                blacklist,
                ..Default::default()
            };
            let mut responses = client
                .get_flows(request)
                .await
                .map_err(|s| status_error(&s))?
                .into_inner();
            let mut batcher = Batcher::new(sink);
            let mut carry = DenyCarry::default();
            let mut caught_up = query.since.is_none();
            let start = Timestamp::now();
            loop {
                let next = tokio::time::timeout(BATCH_WAIT, responses.next()).await;
                let response = match next {
                    // Nothing for a while: send what's collected.
                    Err(_) => {
                        if batcher.flush().await.is_err() {
                            return Ok(());
                        }
                        if !caught_up {
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
                            "Hubble Relay ended the stream".into(),
                        ));
                    }
                    Ok(Some(Err(status))) => {
                        let _ = batcher.flush().await;
                        return Err(status_error(&status));
                    }
                    Ok(Some(Ok(response))) => response,
                };
                if let Some(get_flows_response::ResponseTypes::Flow(flow)) = response.response_types
                {
                    let Some(flow) = map_flow(flow, this.keep_query_values, &mut carry) else {
                        continue;
                    };
                    if !caught_up && flow.time >= start {
                        caught_up = true;
                        if batcher.send(StreamEvent::CaughtUp).await.is_err() {
                            return Ok(());
                        }
                    }
                    if batcher.push(flow).await.is_err() {
                        return Ok(());
                    }
                }
            }
        })
    }
}

/// `hubble-relay v1.20.2+ge0dc92bd` → `1.20.2`.
fn relay_version(raw: &str) -> Option<String> {
    let version = raw
        .split_whitespace()
        .last()?
        .split('+')
        .next()?
        .trim_start_matches('v');
    (!version.is_empty()).then(|| version.to_string())
}

fn to_proto_time(time: Timestamp) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: time.as_second(),
        nanos: time.subsec_nanosecond(),
    }
}

// ----- Pushdown -----

/// A term Hubble's `FlowFilter` can express.
fn pushable(term: &Term) -> bool {
    if term.op != Op::Eq || term.has_glob() {
        return false;
    }
    match term.field {
        Field::Namespace | Field::Pod | Field::Workload | Field::Ip => true,
        Field::Port => term.exact_numbers().is_some(),
        Field::Protocol => term.values.iter().all(|v| {
            matches!(
                v.to_lowercase().as_str(),
                "tcp" | "udp" | "icmp" | "icmpv4" | "icmpv6" | "sctp" | "http" | "dns" | "kafka"
            )
        }),
        Field::Verdict => term
            .verdicts()
            .is_some_and(|v| v.iter().all(|v| verdict_code(*v).is_some())),
        Field::Direction => term
            .directions()
            .is_some_and(|d| d.iter().all(|d| *d != Direction::Unknown)),
        _ => false,
    }
}

/// The terms Relay applies: every pushable one-sided or flow-wide term, and the first
/// either-side term (which becomes two alternatives: source or destination).
pub fn pushdown(filter: &FlowFilter) -> FlowFilter {
    let mut either_used = false;
    filter.subset(|term| {
        if !pushable(term) {
            return false;
        }
        if term.field.sided() && term.side == Side::Either {
            if either_used {
                return false;
            }
            either_used = true;
        }
        true
    })
}

fn verdict_code(verdict: Verdict) -> Option<pb::Verdict> {
    Some(match verdict {
        Verdict::Forwarded => pb::Verdict::Forwarded,
        Verdict::Dropped => pb::Verdict::Dropped,
        Verdict::Error => pb::Verdict::Error,
        Verdict::Audit => pb::Verdict::Audit,
        Verdict::Redirected => pb::Verdict::Redirected,
        Verdict::Traced => pb::Verdict::Traced,
        Verdict::Translated => pb::Verdict::Translated,
        Verdict::NoReply | Verdict::Unknown => return None,
    })
}

/// Hubble's whitelist (alternatives) and blacklist for a pushed-down filter.
pub fn to_filters(filter: &FlowFilter) -> (Vec<pb::FlowFilter>, Vec<pb::FlowFilter>) {
    if filter.terms.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut base = pb::FlowFilter::default();
    let mut either: Option<&Term> = None;
    for term in &filter.terms {
        if term.field.sided() && term.side == Side::Either {
            either = Some(term);
            continue;
        }
        apply(&mut base, term, term.side);
    }
    let whitelist = match either {
        None => vec![base],
        Some(term) => [Side::Source, Side::Destination]
            .into_iter()
            .map(|side| {
                let mut f = base.clone();
                apply(&mut f, term, side);
                f
            })
            .collect(),
    };
    (whitelist, Vec::new())
}

fn apply(f: &mut pb::FlowFilter, term: &Term, side: Side) {
    let source = side == Side::Source;
    match term.field {
        Field::Namespace => {
            let values = term.values.iter().map(|ns| format!("{ns}/"));
            if source {
                f.source_pod.extend(values);
            } else {
                f.destination_pod.extend(values);
            }
        }
        Field::Pod => {
            // `ns/pod` or `/pod` (any namespace); Hubble matches pod names by prefix.
            let values = term.values.iter().map(|v| {
                if v.contains('/') {
                    v.clone()
                } else {
                    format!("/{v}")
                }
            });
            if source {
                f.source_pod.extend(values);
            } else {
                f.destination_pod.extend(values);
            }
        }
        Field::Workload => {
            let mut namespaces = Vec::new();
            let workloads: Vec<pb::Workload> = term
                .values
                .iter()
                .map(|v| {
                    let name = match v.split_once('/') {
                        Some((ns, name)) => {
                            namespaces.push(format!("{ns}/"));
                            name.to_string()
                        }
                        None => v.clone(),
                    };
                    pb::Workload {
                        name,
                        kind: String::new(),
                    }
                })
                .collect();
            if source {
                f.source_workload.extend(workloads);
                f.source_pod.extend(namespaces);
            } else {
                f.destination_workload.extend(workloads);
                f.destination_pod.extend(namespaces);
            }
        }
        Field::Ip => {
            if source {
                f.source_ip.extend(term.values.iter().cloned());
            } else {
                f.destination_ip.extend(term.values.iter().cloned());
            }
        }
        Field::Port => {
            let ports = term
                .exact_numbers()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.to_string());
            if source {
                f.source_port.extend(ports);
            } else {
                f.destination_port.extend(ports);
            }
        }
        Field::Protocol => f
            .protocol
            .extend(term.values.iter().map(|v| v.to_lowercase())),
        Field::Verdict => f.verdict.extend(
            term.verdicts()
                .unwrap_or_default()
                .iter()
                .filter_map(|v| verdict_code(*v))
                .map(|v| v as i32),
        ),
        Field::Direction => {
            f.traffic_direction
                .extend(
                    term.directions()
                        .unwrap_or_default()
                        .iter()
                        .map(|d| match d {
                            Direction::Ingress => pb::TrafficDirection::Ingress as i32,
                            _ => pb::TrafficDirection::Egress as i32,
                        }),
                )
        }
        _ => {}
    }
}

// ----- Mapping -----

/// A connection: source and destination address, destination port, IP protocol.
type ConnKey = (String, String, u16, u8);

/// Deny-rule names of recent policy-verdict events, by connection, for the drop notifications
/// that follow them without names.
#[derive(Default)]
pub struct DenyCarry {
    entries: HashMap<ConnKey, (Vec<PolicyRef>, Timestamp)>,
    order: VecDeque<ConnKey>,
}

impl DenyCarry {
    fn remember(&mut self, key: ConnKey, policies: Vec<PolicyRef>, at: Timestamp) {
        if self.entries.insert(key.clone(), (policies, at)).is_none() {
            self.order.push_back(key);
            while self.order.len() > CARRY_ENTRIES {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
        }
    }

    fn recall(&self, key: &ConnKey, at: Timestamp) -> Option<Vec<PolicyRef>> {
        let (policies, when) = self.entries.get(key)?;
        let age = at.duration_since(*when);
        (age.abs() <= jiff::SignedDuration::try_from(CARRY_FOR).unwrap_or_default())
            .then(|| policies.clone())
    }
}

fn text(value: &str) -> Option<Text> {
    (!value.is_empty()).then(|| value.into())
}

fn policies(list: &[pb::Policy]) -> Vec<PolicyRef> {
    list.iter()
        .map(|p| PolicyRef {
            kind: if p.kind.is_empty() {
                "CiliumNetworkPolicy".into()
            } else {
                p.kind.as_str().into()
            },
            namespace: text(&p.namespace),
            name: p.name.as_str().into(),
            tier: None,
        })
        .collect()
}

/// Labels without Cilium's internal ones: `k8s:app=web` → `app=web`.
fn labels(raw: &[String]) -> Vec<Text> {
    raw.iter()
        .filter_map(|l| l.strip_prefix("k8s:"))
        .filter(|l| {
            !l.starts_with("io.kubernetes.pod.namespace") && !l.starts_with("io.cilium.k8s.")
        })
        .map(Text::from)
        .collect()
}

fn endpoint_kind(endpoint: &pb::Endpoint) -> EndpointKind {
    let has = |label: &str| endpoint.labels.iter().any(|l| l == label);
    if !endpoint.pod_name.is_empty() {
        EndpointKind::Pod
    } else if has("reserved:host") || endpoint.identity == 1 {
        EndpointKind::Host
    } else if has("reserved:remote-node") || endpoint.identity == 6 {
        EndpointKind::RemoteNode
    } else if has("reserved:kube-apiserver") || endpoint.identity == 7 {
        EndpointKind::KubeApiServer
    } else if has("reserved:world")
        || has("reserved:world-ipv4")
        || has("reserved:world-ipv6")
        || matches!(endpoint.identity, 2 | 9 | 10)
    {
        EndpointKind::World
    } else if endpoint.labels.iter().any(|l| l.starts_with("reserved:")) {
        EndpointKind::Internal
    } else if !endpoint.namespace.is_empty() {
        EndpointKind::Pod
    } else {
        EndpointKind::Unknown
    }
}

fn endpoint(
    raw: Option<&pb::Endpoint>,
    ip: &str,
    port: Option<u16>,
    names: &[String],
    service: Option<&pb::Service>,
) -> Endpoint {
    let mut out = Endpoint {
        ip: ip.parse::<IpAddr>().ok(),
        port,
        names: names
            .iter()
            .map(|n| n.trim_end_matches('.').into())
            .collect(),
        service: service.and_then(|s| text(&s.name)),
        ..Endpoint::default()
    };
    if let Some(raw) = raw {
        out.kind = endpoint_kind(raw);
        out.namespace = text(&raw.namespace);
        out.pod = text(&raw.pod_name);
        out.workload = raw.workloads.first().map(|w| Workload {
            kind: w.kind.as_str().into(),
            name: w.name.as_str().into(),
        });
        out.labels = labels(&raw.labels);
    }
    out
}

fn flags(flags: &pb::TcpFlags) -> String {
    let names = [
        (flags.syn, "SYN"),
        (flags.ack, "ACK"),
        (flags.fin, "FIN"),
        (flags.rst, "RST"),
        (flags.psh, "PSH"),
        (flags.urg, "URG"),
        (flags.ece, "ECE"),
        (flags.cwr, "CWR"),
        (flags.ns, "NS"),
    ];
    names
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `TO_ENDPOINT` → `to-endpoint`.
fn enum_word(name: &str) -> String {
    name.to_ascii_lowercase().replace('_', "-")
}

fn event_name(flow: &pb::Flow) -> Option<String> {
    let event = flow.event_type.as_ref()?;
    Some(match event.r#type {
        1 => "drop".into(),
        4 => match pb::TraceObservationPoint::try_from(flow.trace_observation_point) {
            Ok(point) if point != pb::TraceObservationPoint::UnknownPoint => {
                format!("trace {}", enum_word(point.as_str_name()))
            }
            _ => "trace".into(),
        },
        5 => "policy verdict".into(),
        7 => "trace sock".into(),
        129 => "L7".into(),
        other => format!("event {other}"),
    })
}

/// Maps one Hubble flow (sanitizing L7). `None` for events without a flow (agent noise).
pub fn map_flow(raw: pb::Flow, keep_query_values: bool, carry: &mut DenyCarry) -> Option<Flow> {
    let time = raw
        .time
        .as_ref()
        .and_then(|t| Timestamp::new(t.seconds, t.nanos).ok())?;
    let mut flow = Flow::new(time);
    let (src_ip, dst_ip) = raw
        .ip
        .as_ref()
        .map(|ip| (ip.source.as_str(), ip.destination.as_str()))
        .unwrap_or_default();
    let mut tcp_flags = None;
    let (protocol, src_port, dst_port) = match raw.l4.as_ref().and_then(|l4| l4.protocol.as_ref()) {
        Some(pb::layer4::Protocol::Tcp(tcp)) => {
            tcp_flags = tcp.flags.as_ref().map(flags).filter(|f| !f.is_empty());
            (
                Protocol::Tcp,
                Some(tcp.source_port),
                Some(tcp.destination_port),
            )
        }
        Some(pb::layer4::Protocol::Udp(udp)) => (
            Protocol::Udp,
            Some(udp.source_port),
            Some(udp.destination_port),
        ),
        Some(pb::layer4::Protocol::Sctp(sctp)) => (
            Protocol::Sctp,
            Some(sctp.source_port),
            Some(sctp.destination_port),
        ),
        Some(pb::layer4::Protocol::IcmPv4(_)) => (Protocol::Icmp, None, None),
        Some(pb::layer4::Protocol::IcmPv6(_)) => (Protocol::IcmpV6, None, None),
        Some(pb::layer4::Protocol::Vrrp(_)) => (Protocol::Other(112), None, None),
        Some(pb::layer4::Protocol::Igmp(_)) => (Protocol::Other(2), None, None),
        None => (Protocol::Unknown, None, None),
    };
    let port = |p: Option<u32>| p.and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0);
    flow.protocol = protocol;
    flow.source = endpoint(
        raw.source.as_ref(),
        src_ip,
        port(src_port),
        &raw.source_names,
        raw.source_service.as_ref(),
    );
    flow.destination = endpoint(
        raw.destination.as_ref(),
        dst_ip,
        port(dst_port),
        &raw.destination_names,
        raw.destination_service.as_ref(),
    );
    flow.verdict = match pb::Verdict::try_from(raw.verdict).unwrap_or(pb::Verdict::Unknown) {
        pb::Verdict::Forwarded => Verdict::Forwarded,
        pb::Verdict::Dropped => Verdict::Dropped,
        pb::Verdict::Error => Verdict::Error,
        pb::Verdict::Audit => Verdict::Audit,
        pb::Verdict::Redirected => Verdict::Redirected,
        pb::Verdict::Traced => Verdict::Traced,
        pb::Verdict::Translated => Verdict::Translated,
        _ => Verdict::Unknown,
    };
    flow.direction = match pb::TrafficDirection::try_from(raw.traffic_direction) {
        Ok(pb::TrafficDirection::Ingress) => Direction::Ingress,
        Ok(pb::TrafficDirection::Egress) => Direction::Egress,
        _ => Direction::Unknown,
    };
    flow.reply = raw.is_reply;
    flow.tcp_flags = tcp_flags.map(Text::from);
    flow.node = text(&raw.node_name);
    flow.event = event_name(&raw).map(Text::from);

    let reason = pb::DropReason::try_from(raw.drop_reason_desc)
        .ok()
        .filter(|r| *r != pb::DropReason::Unknown)
        .map(|r| r.as_str_name());
    let mut denied_by = policies(&raw.ingress_denied_by);
    denied_by.extend(policies(&raw.egress_denied_by));
    let mut allowed_by = policies(&raw.ingress_allowed_by);
    allowed_by.extend(policies(&raw.egress_allowed_by));
    let key = (
        src_ip.to_string(),
        dst_ip.to_string(),
        flow.destination.port.unwrap_or(0),
        protocol_number(protocol),
    );
    if !denied_by.is_empty() {
        carry.remember(key.clone(), denied_by.clone(), time);
    } else if reason == Some("POLICY_DENY")
        && let Some(carried) = carry.recall(&key, time)
    {
        denied_by = carried;
        flow.raw.push((
            "policy".into(),
            "named by the policy-verdict event of this connection".into(),
        ));
    }
    flow.policies = Policies {
        isolated: reason == Some("POLICY_DENIED"),
        allowed_by,
        denied_by,
        isolated_by: Vec::new(),
        reason: reason.map(Text::from),
    };

    if let Some(l7) = &raw.l7 {
        let response = l7.r#type == pb::L7FlowType::Response as i32;
        let latency_ms = (l7.latency_ns > 0).then(|| l7.latency_ns as f64 / 1e6);
        flow.l7 = match &l7.record {
            Some(pb::layer7::Record::Http(http)) => Some(L7::Http {
                method: http.method.as_str().into(),
                url: sanitize::url(&http.url, keep_query_values),
                code: u16::try_from(http.code).ok().filter(|c| *c > 0),
                protocol: text(&http.protocol),
                latency_ms,
                response,
                headers: sanitize::headers(
                    http.headers
                        .iter()
                        .map(|h| (h.key.as_str(), h.value.as_str())),
                ),
            }),
            Some(pb::layer7::Record::Dns(dns)) => Some(L7::Dns {
                query: dns.query.trim_end_matches('.').to_string(),
                rcode: response.then(|| rcode(dns.rcode).into()),
                answers: dns.ips.clone(),
                response,
            }),
            #[allow(deprecated)]
            Some(pb::layer7::Record::Kafka(kafka)) => Some(L7::Other {
                protocol: "kafka".into(),
                summary: format!("{} {}", kafka.api_key, kafka.topic),
            }),
            None => None,
        };
    }
    flow.raw.extend(raw_fields(&raw));
    Some(flow)
}

fn protocol_number(protocol: Protocol) -> u8 {
    match protocol {
        Protocol::Tcp => 6,
        Protocol::Udp => 17,
        Protocol::Icmp => 1,
        Protocol::IcmpV6 => 58,
        Protocol::Sctp => 132,
        Protocol::Other(n) => n,
        Protocol::Unknown => 0,
    }
}

fn rcode(code: u32) -> &'static str {
    match code {
        0 => "NoError",
        1 => "FormErr",
        2 => "ServFail",
        3 => "NXDomain",
        4 => "NotImp",
        5 => "Refused",
        _ => "Error",
    }
}

/// Hubble's own fields for the detail panel (no L7 values: those are in the L7 section,
/// sanitized).
fn raw_fields(raw: &pb::Flow) -> Vec<(Text, String)> {
    let mut out: Vec<(Text, String)> = Vec::new();
    let mut add = |key: &str, value: String| {
        if !value.is_empty() {
            out.push((key.into(), value));
        }
    };
    if let Some(event) = &raw.event_type {
        add(
            "event_type",
            match event_name(raw) {
                Some(name) => format!("{} · {name}", event.r#type),
                None => event.r#type.to_string(),
            },
        );
    }
    add(
        "verdict",
        pb::Verdict::try_from(raw.verdict)
            .map(|v| v.as_str_name().to_string())
            .unwrap_or_default(),
    );
    if let Ok(dir) = pb::TrafficDirection::try_from(raw.traffic_direction)
        && dir != pb::TrafficDirection::Unknown
    {
        add("traffic_direction", dir.as_str_name().into());
    }
    if let Ok(reason) = pb::DropReason::try_from(raw.drop_reason_desc)
        && reason != pb::DropReason::Unknown
    {
        add("drop_reason_desc", reason.as_str_name().into());
    }
    if raw.policy_match_type != 0 {
        let kind = match raw.policy_match_type {
            1 => "L3",
            2 => "L3/L4",
            3 => "L4",
            4 => "all",
            _ => "",
        };
        add(
            "policy_match_type",
            format!("{} · {kind}", raw.policy_match_type),
        );
    }
    let list = |policies: &[pb::Policy]| {
        policies
            .iter()
            .map(|p| {
                let name = if p.namespace.is_empty() {
                    p.name.clone()
                } else {
                    format!("{}/{}", p.namespace, p.name)
                };
                format!("{name} ({}, rev {})", p.kind, p.revision)
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    add("ingress_allowed_by", list(&raw.ingress_allowed_by));
    add("egress_allowed_by", list(&raw.egress_allowed_by));
    add("ingress_denied_by", list(&raw.ingress_denied_by));
    add("egress_denied_by", list(&raw.egress_denied_by));
    if let Ok(point) = pb::TraceObservationPoint::try_from(raw.trace_observation_point)
        && point != pb::TraceObservationPoint::UnknownPoint
    {
        add("trace_observation_point", point.as_str_name().into());
    }
    if let Ok(reason) = pb::TraceReason::try_from(raw.trace_reason)
        && reason != pb::TraceReason::Unknown
    {
        add("trace_reason", reason.as_str_name().into());
    }
    add("node_name", raw.node_name.clone());
    if let Some(source) = &raw.source {
        add("source.identity", source.identity.to_string());
        add("source.labels", source.labels.join(", "));
    }
    if let Some(destination) = &raw.destination {
        add("destination.identity", destination.identity.to_string());
        add("destination.labels", destination.labels.join(", "));
    }
    add("source_names", raw.source_names.join(", "));
    add("destination_names", raw.destination_names.join(", "));
    if let Some(service) = &raw.destination_service {
        add(
            "destination_service",
            if service.namespace.is_empty() {
                service.name.clone()
            } else {
                format!("{}/{}", service.namespace, service.name)
            },
        );
    }
    if let Some(ip) = &raw.ip {
        if ip.encrypted {
            add("IP.encrypted", "true".into());
        }
        add("IP.source_xlated", ip.source_xlated.clone());
    }
    if let Some(interface) = &raw.interface {
        add(
            "interface",
            format!("{} ({})", interface.name, interface.index),
        );
    }
    if raw.proxy_port != 0 {
        add("proxy_port", raw.proxy_port.to_string());
    }
    if let Some(reply) = raw.is_reply {
        add("is_reply", reply.to_string());
    }
    add(
        "Type",
        pb::FlowType::try_from(raw.r#type)
            .map(|t| t.as_str_name().to_string())
            .unwrap_or_default(),
    );
    add("uuid", raw.uuid.clone());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint_pb(ns: &str, pod: &str, workload: &str, labels: &[&str]) -> pb::Endpoint {
        pb::Endpoint {
            identity: 31_844,
            namespace: ns.into(),
            pod_name: pod.into(),
            workloads: if workload.is_empty() {
                Vec::new()
            } else {
                vec![pb::Workload {
                    name: workload.into(),
                    kind: "Deployment".into(),
                }]
            },
            labels: labels.iter().map(|l| l.to_string()).collect(),
            ..Default::default()
        }
    }

    fn tcp(dport: u32, syn: bool) -> pb::Layer4 {
        pb::Layer4 {
            protocol: Some(pb::layer4::Protocol::Tcp(pb::Tcp {
                source_port: 46_224,
                destination_port: dport,
                flags: Some(pb::TcpFlags {
                    syn,
                    ..Default::default()
                }),
            })),
        }
    }

    fn base(event: i32, verdict: pb::Verdict, reason: pb::DropReason) -> pb::Flow {
        pb::Flow {
            time: Some(prost_types::Timestamp {
                seconds: 1_790_000_000,
                nanos: 5,
            }),
            verdict: verdict as i32,
            drop_reason_desc: reason as i32,
            ip: Some(pb::Ip {
                source: "10.244.1.205".into(),
                destination: "10.244.1.117".into(),
                ..Default::default()
            }),
            l4: Some(tcp(80, true)),
            source: Some(endpoint_pb(
                "storefront",
                "scraper-655c844475-v6zbq",
                "scraper",
                &[
                    "k8s:app=scraper",
                    "k8s:io.kubernetes.pod.namespace=storefront",
                ],
            )),
            destination: Some(endpoint_pb(
                "storefront",
                "web-574ff6d9fd-6hd5l",
                "web",
                &["k8s:app=web"],
            )),
            event_type: Some(pb::CiliumEventType {
                r#type: event,
                sub_type: 0,
            }),
            traffic_direction: pb::TrafficDirection::Ingress as i32,
            node_name: "kind-worker".into(),
            ..Default::default()
        }
    }

    fn web_guard() -> pb::Policy {
        pb::Policy {
            name: "web-guard".into(),
            namespace: "storefront".into(),
            labels: Vec::new(),
            revision: 3,
            kind: "CiliumNetworkPolicy".into(),
        }
    }

    /// The spike's two cases: a deny rule (named by the policy-verdict event, carried over to
    /// the drop notification) and isolation (named by nothing).
    #[test]
    fn deny_rules_are_named_and_isolation_is_explained() {
        let mut carry = DenyCarry::default();
        let mut verdict_event = base(5, pb::Verdict::Dropped, pb::DropReason::PolicyDeny);
        verdict_event.ingress_denied_by = vec![web_guard()];
        verdict_event.policy_match_type = 1;
        let first = map_flow(verdict_event, false, &mut carry).unwrap();
        assert_eq!(first.verdict, Verdict::Dropped);
        assert_eq!(first.event.as_deref(), Some("policy verdict"));
        assert_eq!(
            first.policies.summary(first.verdict).text(),
            "denied by storefront/web-guard"
        );
        let drop = map_flow(
            base(1, pb::Verdict::Dropped, pb::DropReason::PolicyDeny),
            false,
            &mut carry,
        )
        .unwrap();
        assert_eq!(drop.event.as_deref(), Some("drop"));
        assert_eq!(
            drop.policies.summary(drop.verdict).text(),
            "denied by storefront/web-guard"
        );
        let isolated = map_flow(
            base(1, pb::Verdict::Dropped, pb::DropReason::PolicyDenied),
            false,
            &mut carry,
        )
        .unwrap();
        assert!(isolated.policies.isolated);
        assert_eq!(
            isolated.policies.summary(isolated.verdict).text(),
            "denied: isolated, no policy allows it"
        );
        assert_eq!(isolated.policies.reason.as_deref(), Some("POLICY_DENIED"));
    }

    #[test]
    fn endpoints_ports_and_labels() {
        let flow = map_flow(
            base(4, pb::Verdict::Forwarded, pb::DropReason::Unknown),
            false,
            &mut DenyCarry::default(),
        )
        .unwrap();
        assert_eq!(flow.source.label(), "storefront/scraper-655c844475-v6zbq");
        assert_eq!(
            flow.source.workload.as_ref().unwrap().name.as_ref(),
            "scraper"
        );
        assert_eq!(flow.source.labels, vec![Text::from("app=scraper")]);
        assert_eq!(flow.destination.port, Some(80));
        assert_eq!(flow.source.port, Some(46_224));
        assert_eq!(flow.protocol_label(), "TCP :80");
        assert_eq!(flow.tcp_flags.as_deref(), Some("SYN"));
        assert_eq!(flow.direction, Direction::Ingress);
        assert_eq!(flow.node.as_deref(), Some("kind-worker"));
        assert!(
            flow.raw
                .iter()
                .any(|(k, v)| k.as_ref() == "event_type" && v == "4 · trace")
        );

        let mut world = base(4, pb::Verdict::Forwarded, pb::DropReason::Unknown);
        world.destination = Some(pb::Endpoint {
            identity: 2,
            labels: vec!["reserved:world".into()],
            ..Default::default()
        });
        world.destination_names = vec!["api.bank.example.com.".into()];
        let world = map_flow(world, false, &mut DenyCarry::default()).unwrap();
        assert_eq!(world.destination.kind, EndpointKind::World);
        assert_eq!(world.destination.label(), "api.bank.example.com");
    }

    #[test]
    fn http_is_sanitized() {
        let mut raw = base(129, pb::Verdict::Forwarded, pb::DropReason::Unknown);
        raw.l7 = Some(pb::Layer7 {
            r#type: pb::L7FlowType::Request as i32,
            latency_ns: 0,
            record: Some(pb::layer7::Record::Http(pb::Http {
                code: 0,
                method: "GET".into(),
                url: "http://web/search?q=shoes&token=not-a-real-token".into(),
                protocol: "HTTP/1.1".into(),
                headers: vec![
                    pb::HttpHeader {
                        key: "Authorization".into(),
                        value: "Bearer secret".into(),
                    },
                    pb::HttpHeader {
                        key: "User-Agent".into(),
                        value: "Wget".into(),
                    },
                ],
            })),
        });
        let flow = map_flow(raw.clone(), false, &mut DenyCarry::default()).unwrap();
        let Some(L7::Http { url, headers, .. }) = &flow.l7 else {
            panic!("no HTTP");
        };
        assert_eq!(url, "http://web/search?q=…&token=…");
        assert!(headers.iter().all(|(_, v)| !v.contains("secret")));
        assert_eq!(flow.protocol_label(), "HTTP GET /search?q=…&token=…");
        let everything = flow
            .raw
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<String>();
        assert!(!everything.contains("not-a-real-token") && !everything.contains("secret"));
        let kept = map_flow(raw, true, &mut DenyCarry::default()).unwrap();
        assert!(
            matches!(&kept.l7, Some(L7::Http { url, .. }) if url.ends_with("token=not-a-real-token"))
        );
    }

    #[test]
    fn relay_versions() {
        assert_eq!(
            relay_version("hubble-relay v1.20.2+ge0dc92bd").as_deref(),
            Some("1.20.2")
        );
        assert_eq!(relay_version("1.19.0").as_deref(), Some("1.19.0"));
        assert_eq!(relay_version(""), None);
    }

    #[test]
    fn filters_push_down() {
        let filter = FlowFilter::parse(
            "ns=storefront dst.port=80 verdict=dropped policy=web-guard pod=web dir=ingress",
        )
        .unwrap();
        let pushed = pushdown(&filter);
        // policy isn't a Hubble filter; the second either-side term (pod) stays in Kubyl.
        assert_eq!(
            pushed.canonical(),
            "ns=storefront dst.port=80 verdict=dropped dir=ingress"
        );
        let (whitelist, blacklist) = to_filters(&pushed);
        assert!(blacklist.is_empty());
        assert_eq!(whitelist.len(), 2);
        assert_eq!(whitelist[0].source_pod, vec!["storefront/".to_string()]);
        assert!(whitelist[0].destination_pod.is_empty());
        assert_eq!(
            whitelist[1].destination_pod,
            vec!["storefront/".to_string()]
        );
        for f in &whitelist {
            assert_eq!(f.destination_port, vec!["80".to_string()]);
            assert_eq!(f.verdict, vec![pb::Verdict::Dropped as i32]);
            assert_eq!(
                f.traffic_direction,
                vec![pb::TrafficDirection::Ingress as i32]
            );
        }
        let workload = pushdown(
            &FlowFilter::parse("src.workload=payments/checkout-api proto=http,dns ns!=x pod=a*")
                .unwrap(),
        );
        assert_eq!(
            workload.canonical(),
            "src.workload=payments/checkout-api proto=http,dns"
        );
        let (whitelist, _) = to_filters(&workload);
        assert_eq!(whitelist[0].source_workload[0].name, "checkout-api");
        assert_eq!(whitelist[0].source_pod, vec!["payments/".to_string()]);
        assert_eq!(
            whitelist[0].protocol,
            vec!["http".to_string(), "dns".to_string()]
        );
        assert!(to_filters(&FlowFilter::default()).0.is_empty());
        // "no reply" isn't a Hubble verdict.
        assert!(pushdown(&FlowFilter::parse("verdict=no-reply").unwrap()).is_empty());
    }
}
