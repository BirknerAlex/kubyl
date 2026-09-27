//! The ring buffer a stream fills (README "Flow buffer and streaming"): bounded by count and
//! age, with sequence numbers so views keep stable references while old flows fall out, and the
//! values seen in it for completion. Memory only: nothing is written anywhere.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;

use crate::filter::{Field, SeenValues};
use crate::model::{Direction, Flow, L7};

/// Distinct values kept per field (IPs and pods can be many).
const MAX_VALUES: usize = 2_000;

pub struct FlowBuffer {
    flows: VecDeque<Arc<Flow>>,
    /// The sequence number of `flows[0]`.
    first: u64,
    max_flows: usize,
    max_age: Duration,
    seen: Seen,
}

/// Flows at one moment, for background work (re-filtering, aggregation).
#[derive(Clone)]
pub struct Snapshot {
    pub first: u64,
    pub flows: Arc<[Arc<Flow>]>,
}

impl Snapshot {
    pub fn iter(&self) -> impl Iterator<Item = (u64, &Arc<Flow>)> {
        self.flows
            .iter()
            .enumerate()
            .map(|(i, f)| (self.first + i as u64, f))
    }

    pub fn end(&self) -> u64 {
        self.first + self.flows.len() as u64
    }
}

impl FlowBuffer {
    pub fn new(max_flows: usize, max_age: Duration) -> Self {
        Self {
            flows: VecDeque::new(),
            first: 0,
            max_flows: max_flows.max(100),
            max_age,
            seen: Seen::default(),
        }
    }

    pub fn set_limits(&mut self, max_flows: usize, max_age: Duration) {
        self.max_flows = max_flows.max(100);
        self.max_age = max_age;
    }

    pub fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Appends flows (oldest first) and drops what's over the count limit. Returns the new
    /// flows' sequence numbers.
    pub fn push(&mut self, flows: Vec<Flow>) -> Range<u64> {
        let start = self.range().end;
        for flow in flows {
            self.seen.add(&flow);
            self.flows.push_back(Arc::new(flow));
        }
        while self.flows.len() > self.max_flows {
            self.pop_front();
        }
        start.max(self.first)..self.range().end
    }

    /// Drops flows older than the age limit (from the front: the buffer is in arrival order,
    /// which is time order within a few seconds).
    pub fn expire(&mut self, now: Timestamp) -> usize {
        let cutoff = now
            .checked_sub(jiff::SignedDuration::try_from(self.max_age).unwrap_or_default())
            .unwrap_or(Timestamp::UNIX_EPOCH);
        let mut dropped = 0;
        while self.flows.front().is_some_and(|f| f.time < cutoff) {
            self.pop_front();
            dropped += 1;
        }
        dropped
    }

    fn pop_front(&mut self) {
        if let Some(flow) = self.flows.pop_front() {
            self.seen.remove(&flow);
            self.first += 1;
        }
    }

    pub fn get(&self, seq: u64) -> Option<&Arc<Flow>> {
        let index = seq.checked_sub(self.first)?;
        self.flows.get(usize::try_from(index).ok()?)
    }

    /// The sequence numbers held.
    pub fn range(&self) -> Range<u64> {
        self.first..self.first + self.flows.len() as u64
    }

    pub fn len(&self) -> usize {
        self.flows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flows.is_empty()
    }

    /// The flows from `seq` on (clamped to what's held).
    pub fn since(&self, seq: u64) -> impl Iterator<Item = (u64, &Arc<Flow>)> {
        let skip = seq.saturating_sub(self.first) as usize;
        self.flows
            .iter()
            .enumerate()
            .skip(skip)
            .map(|(i, f)| (self.first + i as u64, f))
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            first: self.first,
            flows: self.flows.iter().cloned().collect(),
        }
    }

    pub fn seen(&self) -> &Seen {
        &self.seen
    }

    /// The newest flow's time.
    pub fn newest(&self) -> Option<Timestamp> {
        self.flows.back().map(|f| f.time)
    }
}

/// How often each value was seen in the buffer, per field.
#[derive(Default)]
pub struct Seen {
    maps: HashMap<Field, HashMap<String, u32>>,
}

impl Seen {
    fn add(&mut self, flow: &Flow) {
        for (field, value) in keys(flow) {
            let map = self.maps.entry(field).or_default();
            if let Some(count) = map.get_mut(&value) {
                *count += 1;
            } else if map.len() < MAX_VALUES {
                map.insert(value, 1);
            }
        }
    }

    fn remove(&mut self, flow: &Flow) {
        for (field, value) in keys(flow) {
            if let Some(map) = self.maps.get_mut(&field)
                && let Some(count) = map.get_mut(&value)
            {
                *count -= 1;
                if *count == 0 {
                    map.remove(&value);
                }
            }
        }
    }
}

impl SeenValues for Seen {
    fn values(&self, field: Field) -> Vec<(String, u32)> {
        let mut values: Vec<(String, u32)> = self
            .maps
            .get(&field)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        values
    }
}

/// The completable values of a flow, each once.
fn keys(flow: &Flow) -> Vec<(Field, String)> {
    let mut out: Vec<(Field, String)> = Vec::with_capacity(16);
    let mut add = |field: Field, value: &str| {
        if !value.is_empty() && !out.iter().any(|(f, v)| *f == field && v == value) {
            out.push((field, value.to_string()));
        }
    };
    for endpoint in [&flow.source, &flow.destination] {
        if let Some(ns) = &endpoint.namespace {
            add(Field::Namespace, ns);
        }
        if let Some(pod) = &endpoint.pod {
            add(Field::Pod, pod);
        }
        if let Some(workload) = &endpoint.workload {
            add(Field::Workload, &workload.name);
        }
        if let Some(ip) = &endpoint.ip {
            add(Field::Ip, &ip.to_string());
        }
        if let Some(service) = &endpoint.service {
            add(Field::Service, service);
        }
        add(Field::Kind, endpoint.kind.label());
    }
    if let Some(port) = flow.destination.port {
        add(Field::Port, &port.to_string());
    }
    add(Field::Protocol, &flow.protocol.label().to_lowercase());
    if let Some(l7) = &flow.l7 {
        add(Field::Protocol, l7.protocol());
        if let L7::Dns { query, .. } = l7 {
            add(Field::Dns, query.trim_end_matches('.'));
        }
    }
    match flow.direction {
        Direction::Ingress => add(Field::Direction, "ingress"),
        Direction::Egress => add(Field::Direction, "egress"),
        Direction::Unknown => {}
    }
    add(Field::Verdict, flow.verdict.key());
    for policy in flow.policies.all() {
        add(Field::Policy, &policy.label());
    }
    if flow.policies.isolated {
        add(Field::Policy, "isolated");
    }
    if let Some(reason) = &flow.policies.reason {
        add(Field::Reason, reason);
    }
    if let Some(node) = &flow.node {
        add(Field::Node, node);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Endpoint, EndpointKind, Verdict};

    fn flow(second: i64, ns: &str, verdict: Verdict) -> Flow {
        let mut flow = Flow::new(Timestamp::from_second(second).unwrap());
        flow.source = Endpoint {
            kind: EndpointKind::Pod,
            namespace: Some(ns.into()),
            ..Endpoint::default()
        };
        flow.verdict = verdict;
        flow
    }

    #[test]
    fn sequence_numbers_survive_eviction() {
        let mut buffer = FlowBuffer::new(100, Duration::from_secs(600));
        let first = buffer.push(
            (0..80)
                .map(|i| flow(1_000 + i, "a", Verdict::Forwarded))
                .collect(),
        );
        assert_eq!(first, 0..80);
        let second = buffer.push(
            (0..50)
                .map(|i| flow(1_100 + i, "b", Verdict::Dropped))
                .collect(),
        );
        assert_eq!(second, 80..130);
        assert_eq!(buffer.range(), 30..130);
        assert!(buffer.get(29).is_none());
        assert_eq!(buffer.get(30).unwrap().time.as_second(), 1_030);
        assert_eq!(buffer.since(128).count(), 2);
        // Counts follow evictions.
        let ns = buffer.seen().values(Field::Namespace);
        assert_eq!(ns, vec![("a".to_string(), 50), ("b".to_string(), 50)]);
        let snapshot = buffer.snapshot();
        assert_eq!(snapshot.end(), 130);
        assert_eq!(snapshot.iter().next().unwrap().0, 30);
    }

    #[test]
    fn old_flows_expire() {
        let mut buffer = FlowBuffer::new(1_000, Duration::from_secs(60));
        buffer.push(
            (0..10)
                .map(|i| flow(1_000 + i * 10, "a", Verdict::Forwarded))
                .collect(),
        );
        let dropped = buffer.expire(Timestamp::from_second(1_125).unwrap());
        // Older than 1,065: 1,000 … 1,060.
        assert_eq!(dropped, 7);
        assert_eq!(buffer.range(), 7..10);
        assert_eq!(
            buffer.seen().values(Field::Verdict),
            vec![("forwarded".to_string(), 3)]
        );
    }
}
