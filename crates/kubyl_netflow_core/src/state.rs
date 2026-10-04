//! The flow service's states and connection decisions without the UI.
//! `kubyl_netflow::service::FlowService` runs detection, connections and streams.

use std::sync::Arc;
use std::time::Duration;

use crate::detect::{Detection, RelayTls};
use crate::provider::{BackendKind, BackendStatus, Capabilities, ProviderError};

/// How long streams and the forward stay after the last lease went (tab moved, rebuilt).
pub const GRACE: Duration = Duration::from_secs(2);
/// How often the service looks at leases, expiry and status.
pub const TICK: Duration = Duration::from_secs(1);
/// How often the backend's status (nodes, buffered flows) is read while a view is open.
pub const STATUS_EVERY: Duration = Duration::from_secs(30);
/// How often the metrics topology is read while shown.
pub const GRAPH_EVERY: Duration = Duration::from_secs(30);
/// How long the forward to Relay may take to listen.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);
/// Frames: batches go to the views at most this often.
pub const FRAME: Duration = Duration::from_millis(16);

/// What a view shows for a cluster.
#[derive(Clone)]
pub enum FlowState {
    NotConnected,
    /// Discovery or detection haven't finished.
    Detecting,
    /// `netflow.clusters.<cluster>.backend` is `off`.
    Off,
    /// None of the three backends: what was checked, and the CNI for the hint.
    NoBackend(Arc<Detection>),
    Connecting {
        kind: BackendKind,
        detection: Arc<Detection>,
    },
    Failed {
        kind: BackendKind,
        error: ProviderError,
        detection: Arc<Detection>,
    },
    Ready {
        kind: BackendKind,
        status: BackendStatus,
        capabilities: Capabilities,
        detection: Arc<Detection>,
    },
}

impl FlowState {
    /// The detection behind the state (for "also found" and the empty state).
    pub fn detection(&self) -> Option<&Arc<Detection>> {
        match self {
            FlowState::NoBackend(d)
            | FlowState::Connecting { detection: d, .. }
            | FlowState::Failed { detection: d, .. }
            | FlowState::Ready { detection: d, .. } => Some(d),
            _ => None,
        }
    }
}

/// A stream's state, for the header's live indicator.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamStatus {
    Starting,
    /// History arrived; live flows follow.
    Live,
    /// Failed; retrying at the instant.
    Retrying(ProviderError),
}

/// The CA and server name to check Hubble Relay's certificate with (`None`: plain), or why
/// Kubyl can't talk to it.
pub fn relay_tls(tls: RelayTls) -> Result<Option<(Vec<u8>, String)>, ProviderError> {
    match tls {
        RelayTls::Plain => Ok(None),
        RelayTls::Server {
            ca: Some(ca),
            server_name,
            ..
        } => Ok(Some((ca, server_name))),
        RelayTls::Server {
            ca: None,
            ca_source,
            ..
        } => Err(ProviderError::Unsupported(format!(
            "Hubble Relay serves TLS, but Kubyl has no CA to check its certificate ({ca_source}). Publish Cilium's CA in a ConfigMap (Helm tls.caBundle.enabled, ConfigMap cilium-root-ca.crt) or name a CA file in settings (netflow.clusters.<cluster>.hubble.ca_file). Kubyl doesn't read the CA from Secrets."
        ))),
        RelayTls::Mutual => Err(ProviderError::Unsupported(
            "Hubble Relay wants client certificates (hubble.relay.tls.server.mtls). Kubyl doesn't support that: the certificates are in Secrets, which Kubyl doesn't read.".into(),
        )),
    }
}
