//! The error type shared across crates.
//!
//! Messages end up in toasts and logs, so they must never contain tokens, client keys or
//! Secret data. Wrap sensitive failures in a message that names the operation instead.

/// Errors that cross crate boundaries.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A request to the Kubernetes API failed.
    #[error("Kubernetes API error: {0}")]
    Kube(String),
    /// Authentication failed or needs user interaction (sign-in, exec plugin).
    #[error("authentication failed: {0}")]
    Auth(String),
    /// A kubeconfig, settings or state file is invalid.
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The operation was cancelled, usually because its view was closed.
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
