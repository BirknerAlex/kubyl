//! The internal flow model every backend maps its records onto (README "Network flows").
//!
//! A backend leaves out what it doesn't know rather than guessing. Flows are sensitive
//! (endpoint identities, and on backends with L7 visibility request paths and DNS names): the
//! `Debug` impls here never print their contents, and nothing in this crate logs them.

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;

use jiff::Timestamp;

/// Shared text: backends intern repeated names (namespaces, workloads).
pub type Text = Arc<str>;

/// What an endpoint is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EndpointKind {
    #[default]
    Unknown,
    Pod,
    /// A Service's cluster IP (NetObserv sees traffic before it's translated).
    Service,
    /// The node the flow was seen on (Hubble's `reserved:host`).
    Host,
    RemoteNode,
    KubeApiServer,
    /// Outside the cluster.
    World,
    /// Cilium's health checks and similar internals.
    Internal,
}

impl EndpointKind {
    pub fn label(self) -> &'static str {
        match self {
            EndpointKind::Unknown => "unknown",
            EndpointKind::Pod => "pod",
            EndpointKind::Service => "service",
            EndpointKind::Host => "host",
            EndpointKind::RemoteNode => "remote-node",
            EndpointKind::KubeApiServer => "kube-apiserver",
            EndpointKind::World => "world",
            EndpointKind::Internal => "internal",
        }
    }

    pub const ALL: [EndpointKind; 8] = [
        EndpointKind::Pod,
        EndpointKind::Service,
        EndpointKind::Host,
        EndpointKind::RemoteNode,
        EndpointKind::KubeApiServer,
        EndpointKind::World,
        EndpointKind::Internal,
        EndpointKind::Unknown,
    ];
}

/// The workload a pod belongs to (`Deployment` `checkout-api`).
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Workload {
    pub kind: Text,
    pub name: Text,
}

/// One end of a flow.
#[derive(Clone, Default, PartialEq)]
pub struct Endpoint {
    pub kind: EndpointKind,
    pub namespace: Option<Text>,
    /// The pod, or Whisker's aggregate `checkout-api-7d9f8c6b5-*`.
    pub pod: Option<Text>,
    pub workload: Option<Workload>,
    pub ip: Option<IpAddr>,
    pub port: Option<u16>,
    pub service: Option<Text>,
    pub node: Option<Text>,
    /// DNS names the address answered to (world endpoints).
    pub names: Vec<Text>,
    /// `key=value` labels (without the backend's internal prefixes).
    pub labels: Vec<Text>,
}

impl Endpoint {
    /// `payments/checkout-api-7d9f…`, `world api.bank.example.com`, `host`, `10.0.3.7`.
    pub fn label(&self) -> String {
        match (self.kind, &self.namespace, &self.pod) {
            (_, Some(ns), Some(pod)) => format!("{ns}/{pod}"),
            (EndpointKind::Service, Some(ns), None) => match &self.service {
                Some(service) => format!("{ns}/{service}"),
                None => ns.to_string(),
            },
            (EndpointKind::World, _, _) => match (self.names.first(), &self.ip) {
                (Some(name), _) => name.to_string(),
                (None, Some(ip)) => ip.to_string(),
                (None, None) => "world".into(),
            },
            (EndpointKind::Host | EndpointKind::RemoteNode, _, _) => match (&self.node, &self.ip) {
                (Some(node), _) => node.to_string(),
                (None, Some(ip)) => ip.to_string(),
                _ => self.kind.label().into(),
            },
            (_, Some(ns), None) => match &self.workload {
                Some(workload) => format!("{ns}/{}", workload.name),
                None => ns.to_string(),
            },
            _ => match &self.ip {
                Some(ip) => ip.to_string(),
                None => self.kind.label().into(),
            },
        }
    }

    /// The workload name, else the pod's.
    pub fn workload_name(&self) -> Option<&str> {
        self.workload
            .as_ref()
            .map(|w| w.name.as_ref())
            .or(self.pod.as_deref())
    }

    /// In the cluster (a pod or Service with a namespace).
    pub fn in_cluster(&self) -> bool {
        self.namespace.is_some()
    }
}

/// What happened to the flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verdict {
    Forwarded,
    Dropped,
    /// A TCP attempt that never got past `SYN` (NetObserv on CNIs that report no drops).
    NoReply,
    Error,
    Audit,
    Redirected,
    Traced,
    Translated,
    Unknown,
}

impl Verdict {
    pub const ALL: [Verdict; 9] = [
        Verdict::Forwarded,
        Verdict::Dropped,
        Verdict::NoReply,
        Verdict::Error,
        Verdict::Audit,
        Verdict::Redirected,
        Verdict::Traced,
        Verdict::Translated,
        Verdict::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Forwarded => "forwarded",
            Verdict::Dropped => "dropped",
            Verdict::NoReply => "no reply",
            Verdict::Error => "error",
            Verdict::Audit => "audit",
            Verdict::Redirected => "redirected",
            Verdict::Traced => "traced",
            Verdict::Translated => "translated",
            Verdict::Unknown => "unknown",
        }
    }

    /// The filter value (`no-reply`).
    pub fn key(self) -> &'static str {
        match self {
            Verdict::NoReply => "no-reply",
            other => other.label(),
        }
    }

    /// A filter value, with the words other tools use (`allowed`, `denied`, `drop`).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Some(match text.as_str() {
            "forwarded" | "forward" | "allowed" | "allow" | "accepted" => Verdict::Forwarded,
            "dropped" | "drop" | "denied" | "deny" | "blocked" => Verdict::Dropped,
            "no-reply" | "noreply" | "unanswered" => Verdict::NoReply,
            "error" => Verdict::Error,
            "audit" => Verdict::Audit,
            "redirected" | "redirect" => Verdict::Redirected,
            "traced" | "trace" => Verdict::Traced,
            "translated" => Verdict::Translated,
            "unknown" => Verdict::Unknown,
            _ => return None,
        })
    }

    /// Blocked: dropped, or never answered.
    pub fn blocked(self) -> bool {
        matches!(self, Verdict::Dropped | Verdict::NoReply)
    }
}

/// Seen from the endpoint that reported the flow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    #[default]
    Unknown,
    Ingress,
    Egress,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::Unknown => "—",
            Direction::Ingress => "in",
            Direction::Egress => "out",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "ingress" | "in" => Some(Direction::Ingress),
            "egress" | "out" => Some(Direction::Egress),
            "unknown" => Some(Direction::Unknown),
            _ => None,
        }
    }
}

/// The L4 protocol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    IcmpV6,
    Sctp,
    /// An IP protocol number without a name here.
    Other(u8),
    #[default]
    Unknown,
}

impl Protocol {
    pub fn label(self) -> String {
        match self {
            Protocol::Tcp => "TCP".into(),
            Protocol::Udp => "UDP".into(),
            Protocol::Icmp => "ICMP".into(),
            Protocol::IcmpV6 => "ICMPv6".into(),
            Protocol::Sctp => "SCTP".into(),
            Protocol::Other(n) => format!("IP {n}"),
            Protocol::Unknown => "—".into(),
        }
    }

    /// From an IP protocol number (NetObserv's `Proto`).
    pub fn from_number(n: u64) -> Self {
        match n {
            6 => Protocol::Tcp,
            17 => Protocol::Udp,
            1 => Protocol::Icmp,
            58 => Protocol::IcmpV6,
            132 => Protocol::Sctp,
            n => u8::try_from(n).map_or(Protocol::Unknown, Protocol::Other),
        }
    }

    /// From a name (`tcp`, `UDP`, `icmpv6`).
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "tcp" => Protocol::Tcp,
            "udp" => Protocol::Udp,
            "icmp" | "icmpv4" => Protocol::Icmp,
            "icmpv6" | "icmp6" => Protocol::IcmpV6,
            "sctp" => Protocol::Sctp,
            _ => Protocol::Unknown,
        }
    }
}

/// A policy a backend named.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct PolicyRef {
    /// `CiliumNetworkPolicy`, `NetworkPolicy`, `CalicoNetworkPolicy`, `GlobalNetworkPolicy`,
    /// `Profile`…
    pub kind: Text,
    pub namespace: Option<Text>,
    pub name: Text,
    /// Calico's tier.
    pub tier: Option<Text>,
}

impl PolicyRef {
    /// `storefront/web-guard`, or `web-guard` for cluster-wide policies.
    pub fn label(&self) -> String {
        match &self.namespace {
            Some(ns) if !ns.is_empty() => format!("{ns}/{}", self.name),
            _ => self.name.to_string(),
        }
    }

    /// The API groups (most likely first) and kind that serve this policy, for opening it.
    /// `None` for what isn't an object (Calico profiles, end of tier, OVN ACLs).
    pub fn api(&self) -> Option<(&'static [&'static str], &'static str)> {
        const CILIUM: &[&str] = &["cilium.io"];
        const CALICO: &[&str] = &["projectcalico.org", "crd.projectcalico.org"];
        const NETWORKING: &[&str] = &["networking.k8s.io"];
        const POLICY: &[&str] = &["policy.networking.k8s.io"];
        Some(match self.kind.as_ref() {
            "CiliumNetworkPolicy" => (CILIUM, "CiliumNetworkPolicy"),
            "CiliumClusterwideNetworkPolicy" => (CILIUM, "CiliumClusterwideNetworkPolicy"),
            "NetworkPolicy" => (NETWORKING, "NetworkPolicy"),
            "CalicoNetworkPolicy" => (CALICO, "NetworkPolicy"),
            "GlobalNetworkPolicy" => (CALICO, "GlobalNetworkPolicy"),
            "StagedNetworkPolicy" => (CALICO, "StagedNetworkPolicy"),
            "StagedGlobalNetworkPolicy" => (CALICO, "StagedGlobalNetworkPolicy"),
            "StagedKubernetesNetworkPolicy" => (CALICO, "StagedKubernetesNetworkPolicy"),
            "AdminNetworkPolicy" => (POLICY, "AdminNetworkPolicy"),
            "BaselineAdminNetworkPolicy" => (POLICY, "BaselineAdminNetworkPolicy"),
            "ClusterNetworkPolicy" => (POLICY, "ClusterNetworkPolicy"),
            _ => return None,
        })
    }
}

/// A backend's drop reason for people: `UNSUPPORTED_L3_PROTOCOL` → `unsupported L3 protocol`,
/// `SKB_DROP_REASON_NETFILTER_DROP` → `netfilter drop`.
pub fn reason_text(raw: &str) -> String {
    const UPPER: &[&str] = &[
        "ARP", "BPF", "CIDR", "CT", "DNS", "DSR", "ENI", "FIB", "GRE", "ICMP", "ID", "IP", "LB",
        "MAC", "MTU", "NAT", "OVN", "SCTP", "SNAT", "DNAT", "SKB", "TCP", "TTL", "UDP", "VLAN",
        "VTEP", "VXLAN", "XDP",
    ];
    let raw = raw.strip_prefix("SKB_DROP_REASON_").unwrap_or(raw);
    raw.split(['_', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let upper = word.to_ascii_uppercase();
            match upper.as_str() {
                "IPV4" => "IPv4".to_string(),
                "IPV6" => "IPv6".to_string(),
                "ICMPV6" => "ICMPv6".to_string(),
                "IPSEC" => "IPsec".to_string(),
                _ if UPPER.contains(&upper.as_str()) => upper,
                // L3, L7, SRV6: short tokens with digits read as acronyms.
                _ if word.len() <= 4 && word.chars().any(|c| c.is_ascii_digit()) => upper,
                _ => word.to_ascii_lowercase(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The policies behind a verdict, as far as the backend attributes them.
#[derive(Clone, Default, PartialEq)]
pub struct Policies {
    pub allowed_by: Vec<PolicyRef>,
    pub denied_by: Vec<PolicyRef>,
    /// Dropped because the endpoint is isolated and nothing allowed the flow: a plain
    /// NetworkPolicy has no deny rules. `isolated_by` names the policy that isolated it when
    /// the backend knows (Calico's end-of-tier trigger).
    pub isolated: bool,
    pub isolated_by: Vec<PolicyRef>,
    /// The backend's drop reason (`POLICY_DENIED`, `SKB_DROP_REASON_NETFILTER_DROP`).
    pub reason: Option<Text>,
}

/// What the Policy column says about a flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicySummary {
    DeniedBy(String),
    Isolated(Option<String>),
    AllowedBy(String),
    /// Blocked without a policy (a kernel drop, no reply).
    Reason(String),
    None,
}

impl PolicySummary {
    pub fn text(&self) -> String {
        match self {
            PolicySummary::DeniedBy(name) => format!("denied by {name}"),
            PolicySummary::Isolated(Some(name)) => format!("isolated by {name}"),
            PolicySummary::Isolated(None) => "denied: isolated, no policy allows it".into(),
            PolicySummary::AllowedBy(name) => name.clone(),
            PolicySummary::Reason(reason) => reason.clone(),
            PolicySummary::None => String::new(),
        }
    }
}

impl Policies {
    pub fn is_empty(&self) -> bool {
        self.allowed_by.is_empty()
            && self.denied_by.is_empty()
            && self.isolated_by.is_empty()
            && !self.isolated
            && self.reason.is_none()
    }

    /// Every policy named, for filters and completion.
    pub fn all(&self) -> impl Iterator<Item = &PolicyRef> {
        self.allowed_by
            .iter()
            .chain(&self.denied_by)
            .chain(&self.isolated_by)
    }

    pub fn summary(&self, verdict: Verdict) -> PolicySummary {
        let names = |policies: &[PolicyRef]| {
            let mut names: Vec<String> = policies.iter().map(PolicyRef::label).collect();
            names.dedup();
            names.join(", ")
        };
        if !self.denied_by.is_empty() {
            return PolicySummary::DeniedBy(names(&self.denied_by));
        }
        if self.isolated {
            return PolicySummary::Isolated(
                (!self.isolated_by.is_empty()).then(|| names(&self.isolated_by)),
            );
        }
        if verdict.blocked() {
            if let Some(reason) = &self.reason {
                return PolicySummary::Reason(reason_text(reason));
            }
            if verdict == Verdict::NoReply {
                return PolicySummary::Reason("no answer: dropped or nothing listening".into());
            }
        }
        if !self.allowed_by.is_empty() {
            return PolicySummary::AllowedBy(names(&self.allowed_by));
        }
        PolicySummary::None
    }
}

/// Application-layer data, sanitized while parsing (see `sanitize`).
#[derive(Clone, PartialEq)]
pub enum L7 {
    Http {
        method: Text,
        /// Query values replaced by `…` unless `netflow.keep_query_values`.
        url: String,
        code: Option<u16>,
        protocol: Option<Text>,
        latency_ms: Option<f64>,
        response: bool,
        /// Credentials never kept (`Authorization`, `Cookie`…: values dropped).
        headers: Vec<(String, String)>,
    },
    Dns {
        query: String,
        rcode: Option<Text>,
        answers: Vec<String>,
        response: bool,
    },
    Other {
        protocol: Text,
        summary: String,
    },
}

impl L7 {
    /// The protocol, for filters (`http`, `dns`, `kafka`).
    pub fn protocol(&self) -> &str {
        match self {
            L7::Http { .. } => "http",
            L7::Dns { .. } => "dns",
            L7::Other { protocol, .. } => protocol,
        }
    }

    /// `HTTP GET /search?q=…  404`, `DNS ledger-api.payments.svc.cluster.local`.
    pub fn summary(&self) -> String {
        match self {
            L7::Http {
                method, url, code, ..
            } => {
                let path = crate::sanitize::path_of(url);
                match code {
                    Some(code) if *code > 0 => format!("HTTP {method} {path} {code}"),
                    _ => format!("HTTP {method} {path}"),
                }
            }
            L7::Dns { query, rcode, .. } => match rcode.as_deref() {
                Some(rcode) if !rcode.is_empty() && rcode != "NoError" => {
                    format!("DNS {query} {rcode}")
                }
                _ => format!("DNS {query}"),
            },
            L7::Other { protocol, summary } => format!("{protocol} {summary}"),
        }
    }
}

/// One flow (or one aggregated flow record) from any backend.
#[derive(Clone, PartialEq)]
pub struct Flow {
    pub time: Timestamp,
    /// Aggregated records (Whisker, NetObserv): when the record's interval began.
    pub start: Option<Timestamp>,
    pub source: Endpoint,
    pub destination: Endpoint,
    pub protocol: Protocol,
    pub direction: Direction,
    pub verdict: Verdict,
    pub policies: Policies,
    pub bytes: Option<u64>,
    pub packets: Option<u64>,
    pub l7: Option<L7>,
    /// The backend's event (`policy verdict`, `trace to-endpoint`, `drop`).
    pub event: Option<Text>,
    pub reply: Option<bool>,
    /// `SYN, ACK`.
    pub tcp_flags: Option<Text>,
    /// The node that observed the flow.
    pub node: Option<Text>,
    /// The backend's own fields for the detail panel (sanitized like the rest).
    pub raw: Vec<(Text, String)>,
}

impl Flow {
    /// A flow at `time` with nothing else known (backends fill it in).
    pub fn new(time: Timestamp) -> Self {
        Self {
            time,
            start: None,
            source: Endpoint::default(),
            destination: Endpoint::default(),
            protocol: Protocol::Unknown,
            direction: Direction::Unknown,
            verdict: Verdict::Unknown,
            policies: Policies::default(),
            bytes: None,
            packets: None,
            l7: None,
            event: None,
            reply: None,
            tcp_flags: None,
            node: None,
            raw: Vec::new(),
        }
    }

    /// `TCP :443`, `UDP :53`, or the L7 summary.
    pub fn protocol_label(&self) -> String {
        if let Some(l7) = &self.l7 {
            return l7.summary();
        }
        match self.destination.port {
            Some(port) => format!("{} :{port}", self.protocol.label()),
            None => self.protocol.label(),
        }
    }
}

/// Flow data stays out of `Debug` output (logs, panics).
macro_rules! redacted_debug {
    ($($ty:ty),*) => {
        $(impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), " { .. }"))
            }
        })*
    };
}

redacted_debug!(Flow, Endpoint, Workload, PolicyRef, Policies, L7);

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(ns: &str, name: &str, workload: &str) -> Endpoint {
        Endpoint {
            kind: EndpointKind::Pod,
            namespace: Some(ns.into()),
            pod: Some(name.into()),
            workload: Some(Workload {
                kind: "Deployment".into(),
                name: workload.into(),
            }),
            ..Endpoint::default()
        }
    }

    #[test]
    fn debug_output_never_shows_flow_data() {
        let mut flow = Flow::new(Timestamp::UNIX_EPOCH);
        flow.source = pod("payments", "checkout-api-1", "checkout-api");
        flow.l7 = Some(L7::Http {
            method: "GET".into(),
            url: "http://web/search?q=…".into(),
            code: Some(200),
            protocol: None,
            latency_ms: None,
            response: true,
            headers: Vec::new(),
        });
        let debug = format!("{flow:?} {:?} {:?}", flow.source, flow.l7);
        assert_eq!(debug, "Flow { .. } Endpoint { .. } Some(L7 { .. })");
    }

    #[test]
    fn drop_reasons_read_as_words() {
        assert_eq!(
            reason_text("UNSUPPORTED_L3_PROTOCOL"),
            "unsupported L3 protocol"
        );
        assert_eq!(
            reason_text("SKB_DROP_REASON_NETFILTER_DROP"),
            "netfilter drop"
        );
        assert_eq!(
            reason_text("CT_MAP_INSERTION_FAILED"),
            "CT map insertion failed"
        );
        assert_eq!(
            reason_text("INVALID_IPV6_EXTENSION_HEADER"),
            "invalid IPv6 extension header"
        );
        assert_eq!(reason_text("POLICY_DENY"), "policy deny");
    }

    #[test]
    fn policy_summaries() {
        let web_guard = PolicyRef {
            kind: "CiliumNetworkPolicy".into(),
            namespace: Some("storefront".into()),
            name: "web-guard".into(),
            tier: None,
        };
        let denied = Policies {
            denied_by: vec![web_guard.clone()],
            reason: Some("POLICY_DENY".into()),
            ..Policies::default()
        };
        assert_eq!(
            denied.summary(Verdict::Dropped).text(),
            "denied by storefront/web-guard"
        );
        let isolated = Policies {
            isolated: true,
            reason: Some("POLICY_DENIED".into()),
            ..Policies::default()
        };
        assert_eq!(
            isolated.summary(Verdict::Dropped).text(),
            "denied: isolated, no policy allows it"
        );
        let calico = Policies {
            isolated: true,
            isolated_by: vec![PolicyRef {
                kind: "NetworkPolicy".into(),
                namespace: Some("payments".into()),
                name: "ledger-api-isolation".into(),
                tier: Some("default".into()),
            }],
            ..Policies::default()
        };
        assert_eq!(
            calico.summary(Verdict::Dropped).text(),
            "isolated by payments/ledger-api-isolation"
        );
        let allowed = Policies {
            allowed_by: vec![web_guard],
            ..Policies::default()
        };
        assert_eq!(
            allowed.summary(Verdict::Forwarded).text(),
            "storefront/web-guard"
        );
        assert_eq!(
            Policies::default().summary(Verdict::NoReply).text(),
            "no answer: dropped or nothing listening"
        );
    }

    #[test]
    fn verdict_words() {
        assert_eq!(Verdict::parse("denied"), Some(Verdict::Dropped));
        assert_eq!(Verdict::parse("ALLOWED"), Some(Verdict::Forwarded));
        assert_eq!(Verdict::parse("no reply"), Some(Verdict::NoReply));
        assert_eq!(Verdict::parse("no_reply"), Some(Verdict::NoReply));
        assert_eq!(Verdict::parse("maybe"), None);
        assert_eq!(Verdict::NoReply.key(), "no-reply");
    }

    #[test]
    fn endpoint_labels() {
        assert_eq!(
            pod("payments", "checkout-api-1", "checkout-api").label(),
            "payments/checkout-api-1"
        );
        let world = Endpoint {
            kind: EndpointKind::World,
            ip: Some("203.0.113.24".parse().unwrap()),
            ..Endpoint::default()
        };
        assert_eq!(world.label(), "203.0.113.24");
        let named = Endpoint {
            names: vec!["api.bank.example.com".into()],
            ..world
        };
        assert_eq!(named.label(), "api.bank.example.com");
        let host = Endpoint {
            kind: EndpointKind::Host,
            node: Some("ip-10-0-12-41".into()),
            ..Endpoint::default()
        };
        assert_eq!(host.label(), "ip-10-0-12-41");
    }
}
