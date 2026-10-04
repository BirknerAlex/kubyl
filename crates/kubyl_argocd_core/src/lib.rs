//! Argo CD without the UI.
//!
//! - [`detect`]: finding Argo CD installs (API server, or Kubernetes mode without one).
//! - [`api`], [`sso`]: the Argo CD API and its sign-in; [`ops`]: sync, refresh, rollback,
//!   delete in Kubernetes mode.
//! - [`model`], [`health`], [`tree`], [`diff`], [`links`], [`windows`]: applications,
//!   health, resource trees, live/desired diffs, deep links and sync windows.
//!
//! `kubyl_argocd` re-exports these modules and adds the Argo CD state, views, dialogs and dock.

pub mod api;
pub mod apps;
pub mod detect;
pub mod diff;
pub mod health;
pub mod links;
pub mod model;
pub mod ops;
pub mod run;
pub mod settings;
pub mod sso;
pub mod tree;
pub mod windows;
