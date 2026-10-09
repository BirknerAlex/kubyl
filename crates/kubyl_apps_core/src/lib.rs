//! The Applications view without the UI (phase 25).
//!
//! Kubernetes has no "application" object, but the recommended labels
//! (`app.kubernetes.io/instance`, `name`, `version`, `component`, `part-of`, `managed-by`) name
//! one. [`build`] groups the objects that carry `app.kubernetes.io/instance` by namespace and
//! instance:
//!
//! - [`kinds`]: which kinds count as members and how the cluster serves them.
//! - [`health`]: workload health the way `kubectl rollout status` reads it.
//! - [`model`]: [`App`], its [`model::Member`]s and [`model::Manager`] (Helm, Argo CD, Flux, or
//!   the raw `managed-by` label), reusing the owners' own `managed_by` helpers.
//! - [`table`]: the columns and the CSV records of the table.
//!
//! `kubyl_apps` adds the tab, the sidebar row and the actions.

pub mod health;
pub mod kinds;
pub mod model;
pub mod table;

pub use health::Health;
pub use kinds::Kind;
pub use model::{App, Manager, Member, build};

/// `app.kubernetes.io/instance`: the unique name of an application instance, and the label an
/// object needs to be part of an application here.
pub const INSTANCE: &str = "app.kubernetes.io/instance";
pub const NAME: &str = "app.kubernetes.io/name";
pub const VERSION: &str = "app.kubernetes.io/version";
pub const COMPONENT: &str = "app.kubernetes.io/component";
pub const PART_OF: &str = "app.kubernetes.io/part-of";
pub const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
