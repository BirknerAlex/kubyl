//! The filter language of the flow table, the topology and the backends' server-side filters
//! (README "Flow filters").
//!
//! Terms separated by spaces are ANDed: `key=value`, `key!=value` (or `!key=value`),
//! `key>n`, `key<n`, `key>=n`, `key<=n` for numbers; `a,b` is OR; `*` globs; `"quoted values"`.
//! `src.` and `dst.` restrict a term to one side, without them either side matches. Other words
//! are free text over every field. [`complete`] suggests keys and the values seen in the buffer.

use std::fmt;
use std::net::IpAddr;
use std::ops::Range;

use crate::model::{Direction, Endpoint, EndpointKind, Flow, L7, Protocol, Verdict};

/// Which end of a flow a term looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Either,
    Source,
    Destination,
}

/// A filterable field of [`Flow`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    Namespace,
    Pod,
    Workload,
    Ip,
    Port,
    Kind,
    Node,
    Service,
    Label,
    Protocol,
    Direction,
    Verdict,
    Policy,
    Reason,
    Bytes,
    Packets,
    HttpMethod,
    HttpCode,
    HttpPath,
    Dns,
}

impl Field {
    pub const ALL: [Field; 20] = [
        Field::Namespace,
        Field::Pod,
        Field::Workload,
        Field::Ip,
        Field::Port,
        Field::Kind,
        Field::Node,
        Field::Service,
        Field::Label,
        Field::Protocol,
        Field::Direction,
        Field::Verdict,
        Field::Policy,
        Field::Reason,
        Field::Bytes,
        Field::Packets,
        Field::HttpMethod,
        Field::HttpCode,
        Field::HttpPath,
        Field::Dns,
    ];

    /// The canonical key (`ns`, `http.code`).
    pub fn key(self) -> &'static str {
        match self {
            Field::Namespace => "ns",
            Field::Pod => "pod",
            Field::Workload => "workload",
            Field::Ip => "ip",
            Field::Port => "port",
            Field::Kind => "kind",
            Field::Node => "node",
            Field::Service => "service",
            Field::Label => "label",
            Field::Protocol => "proto",
            Field::Direction => "dir",
            Field::Verdict => "verdict",
            Field::Policy => "policy",
            Field::Reason => "reason",
            Field::Bytes => "bytes",
            Field::Packets => "packets",
            Field::HttpMethod => "http.method",
            Field::HttpCode => "http.code",
            Field::HttpPath => "http.path",
            Field::Dns => "dns",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Some(match key.to_ascii_lowercase().as_str() {
            "ns" | "namespace" => Field::Namespace,
            "pod" => Field::Pod,
            "workload" | "wl" | "owner" => Field::Workload,
            "ip" | "addr" | "address" => Field::Ip,
            "port" => Field::Port,
            "kind" => Field::Kind,
            "node" => Field::Node,
            "service" | "svc" => Field::Service,
            "label" => Field::Label,
            "proto" | "protocol" => Field::Protocol,
            "dir" | "direction" => Field::Direction,
            "verdict" => Field::Verdict,
            "policy" => Field::Policy,
            "reason" => Field::Reason,
            "bytes" => Field::Bytes,
            "packets" | "pkts" => Field::Packets,
            "http.method" | "method" => Field::HttpMethod,
            "http.code" | "status" | "code" => Field::HttpCode,
            "http.path" | "path" | "url" => Field::HttpPath,
            "dns" | "dns.query" => Field::Dns,
            _ => return None,
        })
    }

    /// Belongs to one end (`src.`/`dst.` apply).
    pub fn sided(self) -> bool {
        matches!(
            self,
            Field::Namespace
                | Field::Pod
                | Field::Workload
                | Field::Ip
                | Field::Port
                | Field::Kind
                | Field::Node
                | Field::Service
                | Field::Label
        )
    }

    /// Compares as a number (`>`, `<`, ranges).
    pub fn numeric(self) -> bool {
        matches!(
            self,
            Field::Port | Field::Bytes | Field::Packets | Field::HttpCode
        )
    }
}

/// How a term compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn text(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
        }
    }
}

/// One `key=value` term.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub side: Side,
    pub field: Field,
    pub op: Op,
    /// Alternatives (OR), as typed.
    pub values: Vec<String>,
    matcher: Matcher,
}

impl Term {
    /// A term from already valid parts (`Term::new(Side::Either, Field::Namespace, Op::Eq,
    /// ["payments"])`).
    pub fn new(
        side: Side,
        field: Field,
        op: Op,
        values: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, String> {
        let values: Vec<String> = values.into_iter().map(Into::into).collect();
        let matcher = Matcher::build(field, op, &values)?;
        Ok(Self {
            side,
            field,
            op,
            values,
            matcher,
        })
    }

    /// Does the flow satisfy this term?
    pub fn matches(&self, flow: &Flow) -> bool {
        let hit = if self.field.sided() {
            match self.side {
                Side::Source => self.matcher.endpoint(self.field, &flow.source),
                Side::Destination => self.matcher.endpoint(self.field, &flow.destination),
                Side::Either => {
                    self.matcher.endpoint(self.field, &flow.source)
                        || self.matcher.endpoint(self.field, &flow.destination)
                }
            }
        } else {
            self.matcher.flow(self.field, flow)
        };
        if self.op == Op::Ne { !hit } else { hit }
    }

    /// Only the positive, single-side part is simple for backends: `Eq` terms.
    pub fn is_eq(&self) -> bool {
        self.op == Op::Eq
    }

    /// Any value uses a `*` glob.
    pub fn has_glob(&self) -> bool {
        self.values.iter().any(|v| v.contains('*'))
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.side {
            Side::Either => {}
            Side::Source => f.write_str("src.")?,
            Side::Destination => f.write_str("dst.")?,
        }
        f.write_str(self.field.key())?;
        f.write_str(self.op.text())?;
        let values: Vec<String> = self.values.iter().map(|v| quote(v)).collect();
        f.write_str(&values.join(","))
    }
}

fn quote(value: &str) -> String {
    if value.is_empty() || value.contains(|c: char| c.is_whitespace() || c == ',' || c == '"') {
        format!("\"{}\"", value.replace('"', ""))
    } else {
        value.to_string()
    }
}

/// A whole filter: terms (AND) and free words (AND).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowFilter {
    pub terms: Vec<Term>,
    /// Lowercase free-text words.
    pub words: Vec<String>,
}

/// Where the filter text is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Range<usize>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl FlowFilter {
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let mut filter = FlowFilter::default();
        for token in tokens(text) {
            match parse_term(&token.text) {
                Ok(Some(term)) => filter.terms.push(term),
                Ok(None) => {
                    let word = token.text.trim_matches('"').to_lowercase();
                    if !word.is_empty() {
                        filter.words.push(word);
                    }
                }
                Err(message) => {
                    return Err(ParseError {
                        message,
                        span: token.span,
                    });
                }
            }
        }
        Ok(filter)
    }

    /// A namespace scope (`ns=payments`, either side).
    pub fn namespace(namespace: &str) -> Self {
        Self {
            terms: vec![
                Term::new(Side::Either, Field::Namespace, Op::Eq, [namespace])
                    .expect("a namespace is a valid value"),
            ],
            words: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.words.is_empty()
    }

    pub fn matches(&self, flow: &Flow) -> bool {
        self.terms.iter().all(|t| t.matches(flow))
            && (self.words.is_empty() || {
                let text = haystack(flow);
                self.words.iter().all(|w| text.contains(w.as_str()))
            })
    }

    /// Both filters at once.
    pub fn and(mut self, other: &FlowFilter) -> Self {
        for term in &other.terms {
            if !self.terms.contains(term) {
                self.terms.push(term.clone());
            }
        }
        for word in &other.words {
            if !self.words.contains(word) {
                self.words.push(word.clone());
            }
        }
        self
    }

    /// The terms `keep` accepts (a backend's server-side part), no free text.
    pub fn subset(&self, mut keep: impl FnMut(&Term) -> bool) -> Self {
        Self {
            terms: self.terms.iter().filter(|t| keep(t)).cloned().collect(),
            words: Vec::new(),
        }
    }

    /// Without its verdict terms, and those terms (the verdict chips count over the rest).
    pub fn split_verdicts(&self) -> (FlowFilter, FlowFilter) {
        let (verdicts, rest): (Vec<Term>, Vec<Term>) = self
            .terms
            .iter()
            .cloned()
            .partition(|t| t.field == Field::Verdict);
        (
            FlowFilter {
                terms: rest,
                words: self.words.clone(),
            },
            FlowFilter {
                terms: verdicts,
                words: Vec::new(),
            },
        )
    }

    /// The canonical text (stable: a stream key).
    pub fn canonical(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for FlowFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = self.terms.iter().map(ToString::to_string).collect();
        parts.extend(self.words.iter().map(|w| quote(w)));
        f.write_str(&parts.join(" "))
    }
}

struct Token {
    text: String,
    span: Range<usize>,
}

/// Splits at whitespace outside double quotes.
fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut start = None;
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        match (start, c) {
            (None, c) if c.is_whitespace() => {}
            (None, c) => {
                start = Some(i);
                quoted = c == '"';
            }
            (Some(_), '"') => quoted = !quoted,
            (Some(s), c) if c.is_whitespace() && !quoted => {
                out.push(Token {
                    text: text[s..i].to_string(),
                    span: s..i,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(Token {
            text: text[s..].to_string(),
            span: s..text.len(),
        });
    }
    out
}

/// The parts of a `[!][src.|dst.]key op value` token, if it has an operator.
struct Parts<'a> {
    negated: bool,
    side: Side,
    key: &'a str,
    op: Op,
    value: &'a str,
}

fn split_token(token: &str) -> Option<Parts<'_>> {
    let (negated, rest) = match token.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let at = rest.find(['=', '!', '>', '<'])?;
    let key_part = &rest[..at];
    if key_part.is_empty()
        || !key_part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return None;
    }
    let after = &rest[at..];
    let (op, len) = if after.starts_with("!=") {
        (Op::Ne, 2)
    } else if after.starts_with(">=") {
        (Op::Ge, 2)
    } else if after.starts_with("<=") {
        (Op::Le, 2)
    } else if after.starts_with('=') {
        (Op::Eq, 1)
    } else if after.starts_with('>') {
        (Op::Gt, 1)
    } else if after.starts_with('<') {
        (Op::Lt, 1)
    } else {
        return None;
    };
    let (side, key) = if let Some(key) = key_part.strip_prefix("src.") {
        (Side::Source, key)
    } else if let Some(key) = key_part.strip_prefix("dst.") {
        (Side::Destination, key)
    } else {
        (Side::Either, key_part)
    };
    Some(Parts {
        negated,
        side,
        key,
        op,
        value: &after[len..],
    })
}

fn parse_term(token: &str) -> Result<Option<Term>, String> {
    let Some(parts) = split_token(token) else {
        return Ok(None);
    };
    let Some(field) = Field::parse(parts.key) else {
        return Err(format!(
            "Unknown field {}. Fields: ns, pod, workload, ip, port, kind, node, service, label, proto, dir, verdict, policy, reason, bytes, packets, http.method, http.code, http.path, dns.",
            parts.key
        ));
    };
    if parts.side != Side::Either && !field.sided() {
        return Err(format!(
            "{} has no source or destination: drop the src. or dst.",
            field.key()
        ));
    }
    let op = match (parts.negated, parts.op) {
        (false, op) => op,
        (true, Op::Eq) => Op::Ne,
        (true, Op::Ne) => Op::Eq,
        (true, _) => return Err("Use ! only with = (like !verdict=forwarded).".into()),
    };
    if matches!(op, Op::Gt | Op::Ge | Op::Lt | Op::Le) && !field.numeric() {
        return Err(format!("{} can't be compared with > or <", field.key()));
    }
    let values: Vec<String> = parts
        .value
        .split(',')
        .map(|v| v.trim().trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
        .collect();
    if values.is_empty() {
        return Err(format!("{} needs a value", field.key()));
    }
    Term::new(parts.side, field, op, values).map(Some)
}

/// Pre-parsed values of a term.
#[derive(Clone, Debug, PartialEq)]
enum Matcher {
    Text(Vec<Pattern>),
    Ip(Vec<IpBlock>),
    Number(Vec<NumberMatch>),
    Verdict(Vec<Verdict>),
    Direction(Vec<Direction>),
    Kind(Vec<EndpointKind>),
    Protocol(Vec<String>),
    Policy(Vec<PolicyMatch>),
}

#[derive(Clone, Debug, PartialEq)]
enum PolicyMatch {
    Isolated,
    None,
    Name(Pattern),
}

impl Matcher {
    fn build(field: Field, op: Op, values: &[String]) -> Result<Self, String> {
        Ok(match field {
            Field::Ip => Matcher::Ip(
                values
                    .iter()
                    .map(|v| IpBlock::parse(v).ok_or_else(|| format!("{v} isn't an IP address or CIDR block")))
                    .collect::<Result<_, _>>()?,
            ),
            Field::Port | Field::Bytes | Field::Packets | Field::HttpCode => Matcher::Number(
                values
                    .iter()
                    .map(|v| NumberMatch::parse(field, op, v))
                    .collect::<Result<_, _>>()?,
            ),
            Field::Verdict => Matcher::Verdict(
                values
                    .iter()
                    .map(|v| {
                        Verdict::parse(v).ok_or_else(|| {
                            format!("Unknown verdict {v}: forwarded, dropped, no-reply, error, audit, redirected, traced, translated")
                        })
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Field::Direction => Matcher::Direction(
                values
                    .iter()
                    .map(|v| {
                        Direction::parse(v)
                            .ok_or_else(|| format!("Unknown direction {v}: ingress or egress"))
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Field::Kind => Matcher::Kind(
                values
                    .iter()
                    .map(|v| {
                        EndpointKind::ALL
                            .into_iter()
                            .find(|k| k.label().eq_ignore_ascii_case(v))
                            .ok_or_else(|| {
                                format!("Unknown kind {v}: pod, service, host, remote-node, kube-apiserver, world, internal, unknown")
                            })
                    })
                    .collect::<Result<_, _>>()?,
            ),
            Field::Protocol => Matcher::Protocol(values.iter().map(|v| v.to_lowercase()).collect()),
            Field::Policy => Matcher::Policy(
                values
                    .iter()
                    .map(|v| match v.to_lowercase().as_str() {
                        "isolated" => PolicyMatch::Isolated,
                        "none" => PolicyMatch::None,
                        _ => PolicyMatch::Name(Pattern::new(v, false)),
                    })
                    .collect(),
            ),
            // Pods and paths match by prefix (`pod=checkout-api` finds its replicas).
            Field::Pod | Field::HttpPath => {
                Matcher::Text(values.iter().map(|v| Pattern::new(v, true)).collect())
            }
            _ => Matcher::Text(values.iter().map(|v| Pattern::new(v, false)).collect()),
        })
    }

    fn endpoint(&self, field: Field, endpoint: &Endpoint) -> bool {
        match (self, field) {
            (Matcher::Text(patterns), Field::Namespace) => endpoint
                .namespace
                .as_deref()
                .is_some_and(|ns| patterns.iter().any(|p| p.matches(ns))),
            (Matcher::Text(patterns), Field::Pod) => {
                patterns.iter().any(|p| match p.split_namespace() {
                    Some((ns, pod)) => {
                        endpoint.namespace.as_deref() == Some(ns.as_str())
                            && endpoint
                                .pod
                                .as_deref()
                                .is_some_and(|name| pod.matches(name))
                    }
                    None => endpoint.pod.as_deref().is_some_and(|name| p.matches(name)),
                })
            }
            (Matcher::Text(patterns), Field::Workload) => patterns.iter().any(|p| {
                let name = endpoint.workload_name();
                match p.split_namespace() {
                    Some((ns, workload)) => {
                        endpoint.namespace.as_deref() == Some(ns.as_str())
                            && name.is_some_and(|n| workload.matches(n))
                    }
                    None => name.is_some_and(|n| p.matches(n)),
                }
            }),
            (Matcher::Text(patterns), Field::Node) => endpoint
                .node
                .as_deref()
                .is_some_and(|n| patterns.iter().any(|p| p.matches(n))),
            (Matcher::Text(patterns), Field::Service) => {
                endpoint.service.as_deref().is_some_and(|svc| {
                    patterns.iter().any(|p| match p.split_namespace() {
                        Some((ns, name)) => {
                            endpoint.namespace.as_deref() == Some(ns.as_str()) && name.matches(svc)
                        }
                        None => p.matches(svc),
                    })
                })
            }
            (Matcher::Text(patterns), Field::Label) => endpoint
                .labels
                .iter()
                .any(|l| patterns.iter().any(|p| p.matches(l))),
            (Matcher::Ip(blocks), _) => endpoint
                .ip
                .is_some_and(|ip| blocks.iter().any(|b| b.contains(ip))),
            (Matcher::Number(ranges), Field::Port) => endpoint
                .port
                .is_some_and(|port| ranges.iter().any(|r| r.matches(u64::from(port)))),
            (Matcher::Kind(kinds), _) => kinds.contains(&endpoint.kind),
            _ => false,
        }
    }

    fn flow(&self, field: Field, flow: &Flow) -> bool {
        match (self, field) {
            (Matcher::Verdict(verdicts), _) => verdicts.contains(&flow.verdict),
            (Matcher::Direction(directions), _) => directions.contains(&flow.direction),
            (Matcher::Protocol(names), _) => names.iter().any(|name| {
                let l4 = flow.protocol.label().to_lowercase();
                *name == l4
                    || (name == "icmp" && flow.protocol == Protocol::IcmpV6)
                    || flow.l7.as_ref().is_some_and(|l7| l7.protocol() == name)
            }),
            (Matcher::Policy(matches), _) => matches.iter().any(|m| match m {
                PolicyMatch::Isolated => flow.policies.isolated,
                PolicyMatch::None => {
                    flow.policies.all().next().is_none() && !flow.policies.isolated
                }
                PolicyMatch::Name(p) => flow
                    .policies
                    .all()
                    .any(|policy| p.matches(&policy.name) || p.matches(&policy.label())),
            }),
            (Matcher::Text(patterns), Field::Reason) => flow
                .policies
                .reason
                .as_deref()
                .is_some_and(|r| patterns.iter().any(|p| p.contains(r))),
            (Matcher::Number(ranges), Field::Bytes) => flow
                .bytes
                .is_some_and(|b| ranges.iter().any(|r| r.matches(b))),
            (Matcher::Number(ranges), Field::Packets) => flow
                .packets
                .is_some_and(|p| ranges.iter().any(|r| r.matches(p))),
            (Matcher::Number(ranges), Field::HttpCode) => match &flow.l7 {
                Some(L7::Http {
                    code: Some(code), ..
                }) => ranges.iter().any(|r| r.matches(u64::from(*code))),
                _ => false,
            },
            (Matcher::Text(patterns), Field::HttpMethod) => match &flow.l7 {
                Some(L7::Http { method, .. }) => patterns.iter().any(|p| p.matches(method)),
                _ => false,
            },
            (Matcher::Text(patterns), Field::HttpPath) => match &flow.l7 {
                Some(L7::Http { url, .. }) => {
                    let path = crate::sanitize::path_of(url);
                    patterns.iter().any(|p| p.matches(path))
                }
                _ => false,
            },
            (Matcher::Text(patterns), Field::Dns) => match &flow.l7 {
                Some(L7::Dns { query, .. }) => {
                    let query = query.trim_end_matches('.');
                    patterns.iter().any(|p| p.matches(query))
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// A case-insensitive text pattern: exact, prefix (`prefix` fields) or a `*` glob.
#[derive(Clone, Debug, PartialEq)]
struct Pattern {
    text: String,
    prefix: bool,
}

impl Pattern {
    fn new(text: &str, prefix: bool) -> Self {
        Self {
            text: text.to_lowercase(),
            prefix,
        }
    }

    fn matches(&self, value: &str) -> bool {
        let value = value.to_lowercase();
        if self.text.contains('*') {
            glob(&self.text, &value)
        } else if self.prefix {
            value.starts_with(&self.text)
        } else {
            value == self.text
        }
    }

    /// Substring (reasons are long constants: `reason=netfilter`).
    fn contains(&self, value: &str) -> bool {
        let value = value.to_lowercase();
        if self.text.contains('*') {
            glob(&self.text, &value)
        } else {
            value.contains(&self.text)
        }
    }

    /// `payments/checkout-api` → (`payments`, `checkout-api`).
    fn split_namespace(&self) -> Option<(String, Pattern)> {
        let (ns, rest) = self.text.split_once('/')?;
        Some((
            ns.to_string(),
            Pattern {
                text: rest.to_string(),
                prefix: self.prefix,
            },
        ))
    }
}

/// `*` matches any run of characters.
fn glob(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = value;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return part.is_empty() || rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    rest.is_empty()
}

/// An address or a CIDR block.
#[derive(Clone, Copy, Debug, PartialEq)]
struct IpBlock {
    ip: IpAddr,
    prefix: u8,
}

impl IpBlock {
    fn parse(text: &str) -> Option<Self> {
        let (ip, prefix) = match text.split_once('/') {
            Some((ip, prefix)) => (ip.parse::<IpAddr>().ok()?, Some(prefix.parse::<u8>().ok()?)),
            None => (text.parse::<IpAddr>().ok()?, None),
        };
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        (prefix <= max).then_some(Self { ip, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.ip, ip) {
            (IpAddr::V4(block), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(block) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(block), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(block) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// A number, a range (`8000-8100`) or a bound (`>1000`), or a glob of a status code (`5*`).
#[derive(Clone, Debug, PartialEq)]
enum NumberMatch {
    Range(u64, u64),
    Glob(String),
}

impl NumberMatch {
    fn parse(field: Field, op: Op, text: &str) -> Result<Self, String> {
        let number = |t: &str| {
            t.trim()
                .parse::<u64>()
                .map_err(|_| format!("{} needs a number, not {t}", field.key()))
        };
        if field == Field::HttpCode && text.contains('*') && matches!(op, Op::Eq | Op::Ne) {
            return Ok(NumberMatch::Glob(text.to_string()));
        }
        Ok(match op {
            Op::Eq | Op::Ne => match text.split_once('-') {
                Some((low, high)) => NumberMatch::Range(number(low)?, number(high)?),
                None => {
                    let n = number(text)?;
                    NumberMatch::Range(n, n)
                }
            },
            Op::Gt => NumberMatch::Range(number(text)?.saturating_add(1), u64::MAX),
            Op::Ge => NumberMatch::Range(number(text)?, u64::MAX),
            Op::Lt => NumberMatch::Range(0, number(text)?.saturating_sub(1)),
            Op::Le => NumberMatch::Range(0, number(text)?),
        })
    }

    fn matches(&self, value: u64) -> bool {
        match self {
            NumberMatch::Range(low, high) => (*low..=*high).contains(&value),
            NumberMatch::Glob(pattern) => glob(pattern, &value.to_string()),
        }
    }

    /// The single number of an `=n` term (what backends can push down).
    pub(crate) fn exact(&self) -> Option<u64> {
        match self {
            NumberMatch::Range(low, high) if low == high => Some(*low),
            _ => None,
        }
    }
}

impl Term {
    /// The exact numbers of an `=` term (`port=80,443`), for backends.
    pub fn exact_numbers(&self) -> Option<Vec<u64>> {
        match &self.matcher {
            Matcher::Number(ranges) if self.op == Op::Eq => {
                ranges.iter().map(NumberMatch::exact).collect()
            }
            _ => None,
        }
    }

    /// The verdicts of a verdict term.
    pub fn verdicts(&self) -> Option<&[Verdict]> {
        match &self.matcher {
            Matcher::Verdict(v) => Some(v),
            _ => None,
        }
    }

    /// The directions of a direction term.
    pub fn directions(&self) -> Option<&[Direction]> {
        match &self.matcher {
            Matcher::Direction(d) => Some(d),
            _ => None,
        }
    }
}

/// Everything free text searches, lowercased.
fn haystack(flow: &Flow) -> String {
    let mut text = String::with_capacity(256);
    for endpoint in [&flow.source, &flow.destination] {
        text.push_str(&endpoint.label());
        text.push(' ');
        if let Some(workload) = &endpoint.workload {
            text.push_str(&workload.name);
            text.push(' ');
        }
        if let Some(ip) = &endpoint.ip {
            text.push_str(&ip.to_string());
            text.push(' ');
        }
        if let Some(service) = &endpoint.service {
            text.push_str(service);
            text.push(' ');
        }
        for name in &endpoint.names {
            text.push_str(name);
            text.push(' ');
        }
        if let Some(port) = endpoint.port {
            text.push_str(&port.to_string());
            text.push(' ');
        }
    }
    text.push_str(flow.verdict.label());
    text.push(' ');
    text.push_str(&flow.protocol_label());
    text.push(' ');
    for policy in flow.policies.all() {
        text.push_str(&policy.label());
        text.push(' ');
    }
    if let Some(reason) = &flow.policies.reason {
        text.push_str(reason);
        text.push(' ');
    }
    if let Some(node) = &flow.node {
        text.push_str(node);
    }
    text.to_lowercase()
}

// ----- Completion -----

/// A completion: replace `range` of the input with `text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub range: Range<usize>,
    pub text: String,
    /// What the list shows (`verdict=dropped`).
    pub label: String,
    /// How often it was seen.
    pub count: Option<u32>,
    pub verdict: Option<Verdict>,
}

/// Values seen in the buffer, for completion.
pub trait SeenValues {
    /// `(value, count)`, most seen first.
    fn values(&self, field: Field) -> Vec<(String, u32)>;
}

/// Suggestions for the token that ends at `cursor`: keys while typing a key, seen values after
/// the operator. At most `limit`.
pub fn complete(
    input: &str,
    cursor: usize,
    seen: &dyn SeenValues,
    limit: usize,
) -> Vec<Suggestion> {
    let cursor = cursor.min(input.len());
    if !input.is_char_boundary(cursor) {
        return Vec::new();
    }
    let start = input[..cursor]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + 1);
    let token = &input[start..cursor];
    if token.is_empty() {
        return Vec::new();
    }
    let range = start..cursor;
    match split_token(token) {
        Some(parts) => {
            let Some(field) = Field::parse(parts.key) else {
                return Vec::new();
            };
            // After the last comma: the value being typed.
            let (done, partial) = match parts.value.rfind(',') {
                Some(i) => (&parts.value[..=i], &parts.value[i + 1..]),
                None => ("", parts.value),
            };
            let head = &token[..token.len() - parts.value.len()];
            let partial = partial.to_lowercase();
            let listed: Vec<&str> = done.split(',').filter(|v| !v.is_empty()).collect();
            let mut values = seen.values(field);
            if field == Field::Verdict && values.is_empty() {
                values = [Verdict::Forwarded, Verdict::Dropped]
                    .iter()
                    .map(|v| (v.key().to_string(), 0))
                    .collect();
            }
            if field == Field::Direction && values.is_empty() {
                values = vec![("ingress".into(), 0), ("egress".into(), 0)];
            }
            let mut out: Vec<(bool, Suggestion)> = values
                .into_iter()
                .filter(|(value, _)| !listed.contains(&value.as_str()))
                .filter_map(|(value, count)| {
                    let lower = value.to_lowercase();
                    let prefix = lower.starts_with(&partial);
                    (prefix || lower.contains(&partial)).then(|| {
                        let text = format!("{head}{done}{}", quote(&value));
                        (
                            prefix,
                            Suggestion {
                                range: range.clone(),
                                label: text.clone(),
                                text,
                                count: (count > 0).then_some(count),
                                verdict: (field == Field::Verdict)
                                    .then(|| Verdict::parse(&value))
                                    .flatten(),
                            },
                        )
                    })
                })
                .collect();
            // Prefix matches first, each group in seen order.
            out.sort_by_key(|(prefix, _)| !*prefix);
            out.into_iter().take(limit).map(|(_, s)| s).collect()
        }
        None => {
            let lower = token.to_lowercase();
            let (side, key) = if let Some(k) = lower.strip_prefix("src.") {
                ("src.", k.to_string())
            } else if let Some(k) = lower.strip_prefix("dst.") {
                ("dst.", k.to_string())
            } else {
                ("", lower.clone())
            };
            Field::ALL
                .into_iter()
                .filter(|f| side.is_empty() || f.sided())
                .filter(|f| f.key().starts_with(&key) && f.key() != key)
                .take(limit)
                .map(|f| {
                    let text = format!("{side}{}=", f.key());
                    Suggestion {
                        range: range.clone(),
                        label: text.clone(),
                        text,
                        count: None,
                        verdict: None,
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Endpoint, Policies, PolicyRef, Workload};

    fn endpoint(ns: &str, pod: &str, workload: &str, ip: &str, port: u16) -> Endpoint {
        Endpoint {
            kind: EndpointKind::Pod,
            namespace: Some(ns.into()),
            pod: Some(pod.into()),
            workload: Some(Workload {
                kind: "Deployment".into(),
                name: workload.into(),
            }),
            ip: Some(ip.parse().unwrap()),
            port: Some(port),
            labels: vec![format!("app={workload}").into()],
            ..Endpoint::default()
        }
    }

    fn flow() -> Flow {
        let mut flow = Flow::new(jiff::Timestamp::UNIX_EPOCH);
        flow.source = endpoint(
            "storefront",
            "shopper-6fd84-8wr4x",
            "shopper",
            "10.244.1.6",
            40522,
        );
        flow.destination = endpoint(
            "payments",
            "ledger-api-5cd68-qmjh7",
            "ledger-api",
            "10.244.1.156",
            80,
        );
        flow.protocol = Protocol::Tcp;
        flow.direction = Direction::Ingress;
        flow.verdict = Verdict::Dropped;
        flow.policies = Policies {
            isolated: true,
            reason: Some("POLICY_DENIED".into()),
            ..Policies::default()
        };
        flow.node = Some("kind-worker".into());
        flow
    }

    fn matches(filter: &str) -> bool {
        FlowFilter::parse(filter).unwrap().matches(&flow())
    }

    #[test]
    fn sides_and_either() {
        assert!(matches("ns=payments"));
        assert!(matches("ns=storefront"));
        assert!(matches("src.ns=storefront dst.ns=payments"));
        assert!(!matches("src.ns=payments"));
        assert!(!matches("ns!=payments"));
        assert!(matches("dst.ns!=storefront"));
        assert!(matches("ns=kube-system,payments"));
        assert!(matches("ns=pay*"));
    }

    #[test]
    fn every_field() {
        assert!(matches("pod=shopper"));
        assert!(matches("pod=payments/ledger-api"));
        assert!(!matches("pod=storefront/ledger-api"));
        assert!(matches("workload=ledger-api"));
        assert!(matches("dst.workload=payments/ledger-api"));
        assert!(matches("ip=10.244.1.156"));
        assert!(matches("src.ip=10.244.0.0/16"));
        assert!(!matches("ip=10.0.0.0/16"));
        assert!(matches("dst.port=80"));
        assert!(matches("port=8000-50000"));
        assert!(matches("dst.port<1024"));
        assert!(!matches("dst.port>80"));
        assert!(matches("kind=pod"));
        assert!(matches("label=app=shopper"));
        assert!(matches("proto=tcp"));
        assert!(!matches("proto=udp"));
        assert!(matches("dir=ingress"));
        assert!(matches("verdict=dropped"));
        assert!(matches("verdict=denied"));
        assert!(!matches("verdict=forwarded"));
        assert!(matches("!verdict=forwarded"));
        assert!(matches("policy=isolated"));
        assert!(!matches("policy=web-guard"));
        assert!(matches("reason=policy_denied"));
        assert!(matches("reason=denied"));
        assert!(!matches("bytes>0"));
    }

    #[test]
    fn policy_names_and_l7() {
        let mut flow = flow();
        flow.policies = Policies {
            denied_by: vec![PolicyRef {
                kind: "CiliumNetworkPolicy".into(),
                namespace: Some("storefront".into()),
                name: "web-guard".into(),
                tier: None,
            }],
            ..Policies::default()
        };
        flow.l7 = Some(L7::Http {
            method: "GET".into(),
            url: "http://web/search?q=…".into(),
            code: Some(404),
            protocol: None,
            latency_ms: None,
            response: true,
            headers: Vec::new(),
        });
        let check = |f: &str| FlowFilter::parse(f).unwrap().matches(&flow);
        assert!(check("policy=web-guard"));
        assert!(check("policy=storefront/web-guard"));
        assert!(check("policy=web-*"));
        assert!(!check("policy=isolated"));
        assert!(check("proto=http"));
        assert!(check("http.code=4*"));
        assert!(check("http.code>=400"));
        assert!(check("http.method=get"));
        assert!(check("http.path=/search"));
        assert!(!check("http.path=/checkout"));
        assert!(check("search"));
    }

    #[test]
    fn free_text_and_combinations() {
        assert!(matches("ledger"));
        assert!(matches("LEDGER shopper"));
        assert!(!matches("ledger checkout"));
        assert!(matches("ns=payments verdict=dropped port=80"));
        assert!(matches(r#""kind-worker""#));
    }

    #[test]
    fn errors_name_the_problem() {
        let err = FlowFilter::parse("ns=payments color=red").unwrap_err();
        assert!(err.message.starts_with("Unknown field color"), "{err}");
        assert_eq!(err.span, 12..21);
        assert!(FlowFilter::parse("src.verdict=dropped").is_err());
        assert!(FlowFilter::parse("verdict=maybe").is_err());
        assert!(FlowFilter::parse("ns>3").is_err());
        assert!(FlowFilter::parse("ip=10.0.0.0/40").is_err());
        assert!(FlowFilter::parse("port=http").is_err());
        assert!(FlowFilter::parse("ns=").is_err());
        // Not a term: free text.
        assert_eq!(
            FlowFilter::parse("a/b=c").unwrap().words,
            vec!["a/b=c".to_string()]
        );
    }

    #[test]
    fn canonical_text_round_trips() {
        let filter =
            FlowFilter::parse(r#"src.ns=payments  !verdict=forwarded port>=8000 "two words""#)
                .unwrap();
        let text = filter.canonical();
        assert_eq!(
            text,
            r#"src.ns=payments verdict!=forwarded port>=8000 "two words""#
        );
        assert_eq!(FlowFilter::parse(&text).unwrap(), filter);
        let (base, verdicts) = filter.split_verdicts();
        assert_eq!(
            base.canonical(),
            r#"src.ns=payments port>=8000 "two words""#
        );
        assert_eq!(verdicts.canonical(), "verdict!=forwarded");
        let scoped = FlowFilter::namespace("payments").and(&filter);
        assert_eq!(scoped.terms.len(), 4);
    }

    struct Seen;

    impl SeenValues for Seen {
        fn values(&self, field: Field) -> Vec<(String, u32)> {
            match field {
                Field::Namespace => vec![
                    ("payments".into(), 40),
                    ("kube-system".into(), 12),
                    ("storefront".into(), 9),
                ],
                Field::Verdict => vec![("forwarded".into(), 50), ("dropped".into(), 3)],
                _ => Vec::new(),
            }
        }
    }

    #[test]
    fn completion() {
        let keys = complete("ns=payments ver", 15, &Seen, 8);
        assert_eq!(keys[0].text, "verdict=");
        assert_eq!(keys[0].range, 12..15);
        let values = complete("verdict=dr", 10, &Seen, 8);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].text, "verdict=dropped");
        assert_eq!(values[0].count, Some(3));
        assert_eq!(values[0].verdict, Some(Verdict::Dropped));
        // Prefix matches first, then substrings; listed values are skipped.
        let ns: Vec<String> = complete("src.ns=payments,s", 17, &Seen, 8)
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(
            ns,
            ["src.ns=payments,storefront", "src.ns=payments,kube-system"]
        );
        let sided: Vec<String> = complete("dst.p", 5, &Seen, 8)
            .into_iter()
            .map(|s| s.text)
            .collect();
        assert_eq!(sided, ["dst.pod=", "dst.port="]);
        assert!(complete("ns=payments ", 12, &Seen, 8).is_empty());
    }

    #[test]
    fn globs_and_blocks() {
        assert!(glob("pay*", "payments"));
        assert!(glob("*ments", "payments"));
        assert!(glob("p*y*s", "payments"));
        assert!(!glob("p*x", "payments"));
        assert!(
            IpBlock::parse("fd00::/8")
                .unwrap()
                .contains("fd12::1".parse().unwrap())
        );
        assert!(
            !IpBlock::parse("10.0.0.0/8")
                .unwrap()
                .contains("fd12::1".parse().unwrap())
        );
        assert!(
            IpBlock::parse("0.0.0.0/0")
                .unwrap()
                .contains("1.2.3.4".parse().unwrap())
        );
    }
}
