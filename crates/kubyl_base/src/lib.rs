//! The GPUI-free foundation every Kubyl crate builds on.
//!
//! - [`types`]: IDs and references shared across crates ([`ClusterId`], [`ResourceRef`], [`ViewKind`]…).
//! - [`error`]: the error type that crosses crate boundaries.
//! - [`runtime`]: the shared Tokio runtime.
//! - [`notice`]: messages for the user, without any UI attached.
//! - [`host`]: the [`Host`](host::Host) trait core services run on.
//!
//! `kubyl_core` re-exports all of it next to the GPUI parts (registries, `spawn_kube`).

pub mod error;
pub mod host;
pub mod notice;
pub mod runtime;
pub mod types;

pub use error::{Error, Result};
pub use host::{Flow, Host, HostExt, Pace, Service, TaskHandle};
pub use notice::{Notice, NotificationLevel};
pub use types::{
    ArgoCdCaps, ClusterCaps, ClusterId, ContextName, Gvk, Gvr, ResourceRef, TabContext,
    TabNamespace, ViewKind,
};
