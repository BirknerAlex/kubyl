//! Kubeconfig sources, auth, clients and discovery, without the UI.
//!
//! - [`kubeconfig`]: sources (`~/.kube/config`, `$KUBECONFIG`, added files/folders, pasted),
//!   loading and merging contexts (collisions get `@<file-stem>`); [`groups`]: one entry per
//!   cluster and user.
//! - [`settings`]: the `"kubernetes"` settings.json section (sources, per-context overrides).
//! - [`auth`]: auth method detection, exec plugins, OIDC sign-in/refresh, keychain store.
//! - [`client`], [`transport`]: building and probing clients on Kubyl's HTTP/2 stack.
//! - [`discovery`], [`cluster_info`], [`access`], [`openapi`], [`watches`]: what the cluster
//!   serves and the watches every connection runs.
//!
//! `kubyl_kube` re-exports these modules and adds the GPUI `ConnectionManager` and the UI.

pub mod access;
pub mod auth;
pub mod client;
pub mod cluster_info;
pub mod discovery;
pub mod groups;
pub mod kubeconfig;
pub mod openapi;
pub mod settings;
pub mod transport;
pub mod watches;
