//! The resource engine: watch caches, columns, filters and operations.
//!
//! # For other crates (phases 03–08)
//!
//! - [`store`]: [`ResourceStores::acquire`] returns a shared, live [`StoreHandle`] for a
//!   (cluster, GVR, namespace, selectors) key. Observe its entity to re-render on changes;
//!   updates arrive batched (≤ 60 Hz).
//! - [`selection`]: [`ResourceSelection`] is what the user selected in the focused list.
//!   Action handlers registered by any crate read their targets from it.
//! - [`columns`]: the built-in column sets (registered into `kubyl_core::ResourceColumns`) and
//!   kubectl-identical status helpers ([`columns::pod_status`], [`columns::node_status`]…).
//! - [`table`]: server-side `Table` fetches (CRD printer columns).
//! - [`filter`]: the list filter language.
//! - [`ops`]: delete, scale, restart, undo, cordon, drain, trigger CronJob…
//! - [`metrics`]: the [`metrics::MetricsProvider`] trait phase 07 implements.
//! - [`describe`]: `kubectl describe`-like text.
//! - [`format`]: ages, quantities and JSON helpers.
//! - [`route`]: OpenShift Routes (model, URL, weights, target port resolution, key masking).
//! - [`dra`], [`admission`], [`gateway`], [`vpa`]: device resources, admission policies, the
//!   Gateway API and VPAs (phase 24).
//!
//! The parts without UI live in `kubyl_resources_core` and are re-exported here.

pub mod columns;
pub mod metrics;
pub mod selection;
pub mod store;
pub mod table;

pub use kubyl_resources_core::{
    admission, describe, dra, filter, format, gateway, ops, redact, route, source, vpa,
};

pub use filter::Filter;
pub use selection::{ResourceSelection, Selected};
pub use store::{
    ObjectKey, ResourceStore, ResourceStores, StoreHandle, StoreKey, StoreMode, StoreStatus,
    WatchInfo, key_of, object_key,
};

use gpui::App;

/// Registers the column sets and installs the store registry and selection globals.
///
/// Must run after `kubyl_kube::init` (stores follow the connection manager).
pub fn init(cx: &mut App) {
    columns::register(cx);
    store::init(cx);
    selection::init(cx);
    cx.default_global::<metrics::Metrics>();
}
