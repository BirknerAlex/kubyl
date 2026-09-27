//! The backend abstraction (README "Flow providers"): every flow source maps its own API onto
//! the [`crate::model::Flow`] model behind a [`FlowProvider`].
//!
//! Calls return futures that run on the shared Tokio runtime (`kubyl_core::spawn_kube`), never
//! on the UI thread. A stream sends batches of parsed, sanitized flows into a bounded channel;
//! the service drains it at most 60 times a second.

use std::fmt;
use std::time::Duration;

use futures::channel::mpsc;
use futures::future::BoxFuture;
use jiff::Timestamp;

use crate::aggregate::{Topology, Zoom};
use crate::filter::FlowFilter;
use crate::model::Flow;

/// A provider call's future.
pub type ProviderFuture<T> = BoxFuture<'static, T>;

/// Which backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Hubble,
    Whisker,
    NetObserv,
}

impl BackendKind {
    pub const ALL: [BackendKind; 3] = [
        BackendKind::Hubble,
        BackendKind::Whisker,
        BackendKind::NetObserv,
    ];

    /// `Hubble Relay`, `Calico Whisker`, `NetObserv`.
    pub fn label(self) -> &'static str {
        match self {
            BackendKind::Hubble => "Hubble Relay",
            BackendKind::Whisker => "Calico Whisker",
            BackendKind::NetObserv => "NetObserv",
        }
    }

    /// The settings value (`hubble`, `whisker`, `netobserv`).
    pub fn key(self) -> &'static str {
        match self {
            BackendKind::Hubble => "hubble",
            BackendKind::Whisker => "whisker",
            BackendKind::NetObserv => "netobserv",
        }
    }
}

/// Why a provider call failed, in words the view shows as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// RBAC denies something: the verb, resource and namespace that are missing.
    Forbidden {
        verb: String,
        resource: String,
        namespace: Option<String>,
    },
    NotFound(String),
    /// The backend can't be reached (port-forward, proxy, no ready endpoints).
    Unavailable(String),
    /// Kubyl can't read this setup (a Relay that wants client certificates, LokiStack).
    Unsupported(String),
    Other(String),
}

impl ProviderError {
    pub fn forbidden(verb: &str, resource: &str, namespace: Option<&str>) -> Self {
        ProviderError::Forbidden {
            verb: verb.into(),
            resource: resource.into(),
            namespace: namespace.map(Into::into),
        }
    }

    /// Maps a kube error of `verb` on `resource` in `namespace`.
    pub fn from_kube(
        err: &kube::Error,
        verb: &str,
        resource: &str,
        namespace: Option<&str>,
    ) -> Self {
        match err {
            kube::Error::Api(status) if status.code == 403 => {
                Self::forbidden(verb, resource, namespace)
            }
            kube::Error::Api(status) if status.code == 404 => {
                ProviderError::NotFound(format!("{resource}: {}", status.message))
            }
            kube::Error::Api(status) => ProviderError::Other(status.message.clone()),
            other => ProviderError::Unavailable(other.to_string()),
        }
    }

    /// `create pods/portforward in kube-system`, for a forbidden call.
    pub fn permission(&self) -> Option<String> {
        match self {
            ProviderError::Forbidden {
                verb,
                resource,
                namespace: Some(ns),
            } => Some(format!("{verb} {resource} in {ns}")),
            ProviderError::Forbidden {
                verb,
                resource,
                namespace: None,
            } => Some(format!("{verb} {resource}")),
            _ => None,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Forbidden {
                verb,
                resource,
                namespace,
            } => {
                let scope = namespace
                    .as_deref()
                    .map(|ns| format!(" in {ns}"))
                    .unwrap_or_default();
                write!(
                    f,
                    "Forbidden: you can't {verb} {resource}{scope}. Ask for a role that can {verb} {resource}{scope}."
                )
            }
            ProviderError::NotFound(what) => write!(f, "Not found: {what}"),
            ProviderError::Unavailable(message)
            | ProviderError::Unsupported(message)
            | ProviderError::Other(message) => f.write_str(message),
        }
    }
}

/// What a backend can tell and do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capabilities {
    /// Names the policies that allowed flows.
    pub names_allows: bool,
    /// Names the policies that denied flows.
    pub names_denies: bool,
    /// Names the policy that isolated an endpoint (Calico's end-of-tier trigger).
    pub names_isolation: bool,
    /// Live stream (`true`) or polled.
    pub live: bool,
    /// How far back history goes, in words (`Relay's buffer`, `Loki's retention`).
    pub history: &'static str,
    pub bytes: bool,
    pub l7: bool,
    /// Records are aggregates over an interval (Whisker: 15 s, NetObserv).
    pub aggregated: bool,
    /// Single flows are readable (NetObserv without Loki: `false`, the graph comes from
    /// metrics).
    pub single_flows: bool,
    /// The topology can come from metrics.
    pub graph_from_metrics: bool,
}

/// The header's status line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackendStatus {
    /// `kube-system/hubble-relay`.
    pub endpoint: String,
    /// How it's reached, for the tooltip (`through a temporary port-forward (127.0.0.1:51234)`).
    pub via: String,
    pub version: Option<String>,
    /// Connected and total nodes (Hubble).
    pub nodes: Option<(u32, u32)>,
    /// Flows the backend buffers (Hubble).
    pub buffered: Option<(u64, u64)>,
    /// Notes the view shows (`Loki is off: no single flows here`).
    pub notes: Vec<String>,
}

/// What a stream should return.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamQuery {
    /// The part of the filter the backend applies itself (see [`FlowProvider::pushdown`]).
    pub filter: FlowFilter,
    /// History from here, then live.
    pub since: Option<Timestamp>,
}

/// What a stream sends.
pub enum StreamEvent {
    Flows(Vec<Flow>),
    /// The history up to now arrived; live flows follow.
    CaughtUp,
    Status(BackendStatus),
}

/// Where a stream sends its batches.
pub type FlowSink = mpsc::Sender<StreamEvent>;

/// Batches sent into a sink: at most this many flows, or this long.
pub const BATCH_FLOWS: usize = 512;
pub const BATCH_WAIT: Duration = Duration::from_millis(50);

/// A flow backend. Implementations are cheap to share (`Arc`).
pub trait FlowProvider: Send + Sync {
    fn kind(&self) -> BackendKind;

    fn capabilities(&self) -> Capabilities;

    /// The terms of `filter` this backend applies server-side (the rest applies in Kubyl; the
    /// whole filter always applies in Kubyl too).
    fn pushdown(&self, filter: &FlowFilter) -> FlowFilter;

    /// Checks the backend answers and describes it.
    fn probe(&self) -> ProviderFuture<Result<BackendStatus, ProviderError>>;

    /// Streams flows (history since `query.since`, then live) into `sink` until the sink
    /// closes or the backend fails.
    fn stream(
        &self,
        query: StreamQuery,
        sink: FlowSink,
    ) -> ProviderFuture<Result<(), ProviderError>>;

    /// The topology over `window` from the backend's metrics, when it has them.
    fn graph(
        &self,
        _window: Duration,
        _zoom: Zoom,
        _filter: FlowFilter,
    ) -> ProviderFuture<Result<Option<Topology>, ProviderError>> {
        Box::pin(async { Ok(None) })
    }
}

/// The sink closed: nobody listens to the stream anymore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkClosed;

/// Sends flows to a sink in batches: [`BATCH_FLOWS`] or [`BATCH_WAIT`], whichever comes first.
/// Backends feed it one flow at a time.
pub struct Batcher {
    sink: FlowSink,
    batch: Vec<Flow>,
    started: Option<std::time::Instant>,
}

impl Batcher {
    pub fn new(sink: FlowSink) -> Self {
        Self {
            sink,
            batch: Vec::new(),
            started: None,
        }
    }

    /// Adds a flow; sends when the batch is full or old. `Err` when the sink closed (nobody
    /// listens anymore: stop).
    pub async fn push(&mut self, flow: Flow) -> Result<(), SinkClosed> {
        self.batch.push(flow);
        let started = *self.started.get_or_insert_with(std::time::Instant::now);
        if self.batch.len() >= BATCH_FLOWS || started.elapsed() >= BATCH_WAIT {
            self.flush().await?;
        }
        Ok(())
    }

    /// Sends what's collected.
    pub async fn flush(&mut self) -> Result<(), SinkClosed> {
        use futures::SinkExt as _;
        self.started = None;
        if self.batch.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.batch);
        self.sink
            .send(StreamEvent::Flows(batch))
            .await
            .map_err(|_| SinkClosed)
    }

    pub async fn send(&mut self, event: StreamEvent) -> Result<(), SinkClosed> {
        use futures::SinkExt as _;
        self.flush().await?;
        self.sink.send(event).await.map_err(|_| SinkClosed)
    }

    /// There's something waiting to go out.
    pub fn pending(&self) -> bool {
        !self.batch.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_names_verb_resource_and_namespace() {
        let err = ProviderError::forbidden("create", "pods/portforward", Some("kube-system"));
        assert_eq!(
            err.to_string(),
            "Forbidden: you can't create pods/portforward in kube-system. Ask for a role that can create pods/portforward in kube-system."
        );
        assert_eq!(
            err.permission().as_deref(),
            Some("create pods/portforward in kube-system")
        );
    }

    #[tokio::test]
    async fn batches_go_out_when_full_or_flushed() {
        use futures::StreamExt as _;
        let (tx, mut rx) = mpsc::channel(8);
        let mut batcher = Batcher::new(tx);
        for i in 0..(BATCH_FLOWS + 3) {
            batcher
                .push(Flow::new(Timestamp::from_second(i as i64).unwrap()))
                .await
                .unwrap();
        }
        match rx.next().await {
            Some(StreamEvent::Flows(flows)) => assert_eq!(flows.len(), BATCH_FLOWS),
            _ => panic!("expected a full batch"),
        }
        assert!(batcher.pending());
        batcher.send(StreamEvent::CaughtUp).await.unwrap();
        assert!(matches!(rx.next().await, Some(StreamEvent::Flows(f)) if f.len() == 3));
        assert!(matches!(rx.next().await, Some(StreamEvent::CaughtUp)));
        drop(rx);
        assert!(batcher.push(Flow::new(Timestamp::UNIX_EPOCH)).await.is_ok());
        assert!(batcher.flush().await.is_err());
    }
}
