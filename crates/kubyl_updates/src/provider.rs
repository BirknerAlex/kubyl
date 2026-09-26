//! The provider abstraction: every distribution updates differently, so each gets an
//! [`UpdateProvider`] (decided in phase 13, README "Cluster update providers").
//!
//! Calls return futures that run on the shared Tokio runtime (`kubyl_core::spawn_kube`),
//! never on the UI thread. Progress is [`UpdateProvider::read`] again: the service polls while
//! a view shows the cluster, faster while an update runs.

use std::fmt;

use futures::future::BoxFuture;

use crate::check::Check;
use crate::model::{Plan, ProviderKind, Scope, Status};

/// A provider call's future.
pub type ProviderFuture<T> = BoxFuture<'static, T>;

/// Why a provider call failed, in words the view shows as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    /// RBAC (or the cloud's IAM) denies something: the verb and resource that's missing.
    Forbidden {
        verb: String,
        resource: String,
    },
    NotFound(String),
    /// Cloud credentials are missing or expired; `command` fixes it.
    Credentials {
        message: String,
        command: Option<String>,
    },
    /// The provider can't be reached (network, CLI missing).
    Unavailable(String),
    Other(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::Forbidden { verb, resource } => write!(
                f,
                "Forbidden: you can't {verb} {resource}. Ask for a role that can {verb} {resource}."
            ),
            ProviderError::NotFound(what) => write!(f, "Not found: {what}"),
            ProviderError::Credentials { message, .. } => f.write_str(message),
            ProviderError::Unavailable(message) | ProviderError::Other(message) => {
                f.write_str(message)
            }
        }
    }
}

impl ProviderError {
    /// Maps a kube error of `verb` on `resource` (`clusterversions.config.openshift.io`).
    pub fn from_kube(err: &kube::Error, verb: &str, resource: &str) -> Self {
        match err {
            kube::Error::Api(status) if status.code == 403 => ProviderError::Forbidden {
                verb: verb.to_string(),
                resource: resource.to_string(),
            },
            kube::Error::Api(status) if status.code == 404 => {
                ProviderError::NotFound(format!("{resource} ({})", status.message))
            }
            kube::Error::Api(status) => ProviderError::Other(status.message.clone()),
            other => ProviderError::Unavailable(other.to_string()),
        }
    }
}

/// How a distribution reads and updates its version.
pub trait UpdateProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// One snapshot: current version, channel, targets, history, a running update's progress,
    /// control plane and pools, add-ons.
    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>>;

    /// Checks only this provider can make for `target` (add-on compatibility, `Upgradeable`).
    fn preflight_extras(&self, _status: &Status, _target: &str) -> ProviderFuture<Vec<Check>> {
        Box::pin(async { Vec::new() })
    }

    /// What a write would do, for the summary the user confirms. `cluster` is the display name.
    fn plan(
        &self,
        _status: &Status,
        _scope: &Scope,
        _target: &str,
        _cluster: &str,
    ) -> Result<Plan, String> {
        Err("Kubyl can't update this cluster.".into())
    }

    /// Runs a confirmed plan once (never retried). Answers what was done.
    fn start(&self, _plan: &Plan) -> ProviderFuture<Result<String, ProviderError>> {
        Box::pin(async {
            Err(ProviderError::Other(
                "Kubyl can't update this cluster.".into(),
            ))
        })
    }
}
