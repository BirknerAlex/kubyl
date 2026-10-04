//! The resource engine without the UI.
//!
//! - [`store`]: [`StoreCore`](store::StoreCore), the watch cache of one (cluster, GVR, namespace,
//!   selectors, mode), and its keys and status.
//! - [`source`]: how a service gets shared watch caches ([`source::StoreSource`]) and reads them.
//! - [`columns`]: the column sets and cells of the core kinds.
//! - [`status`]: kubectl-identical status helpers ([`status::pod_status`],
//!   [`status::node_status`]…) and [`status::is_failing`].
//! - [`filter`]: the list filter language.
//! - [`ops`]: delete, scale, restart, undo, cordon, drain, trigger CronJob…
//! - [`describe`]: `kubectl describe`-like text.
//! - [`format`]: ages, quantities and JSON helpers.
//! - [`usage`]: CPU and memory usage as metrics providers report it.
//! - [`route`]: OpenShift Routes (model, URL, weights, target port resolution, key masking).
//!
//! `kubyl_resources` re-exports these modules and adds the GPUI side: shared store entities,
//! column providers, the selection and the metrics provider registry.

pub mod columns;
pub mod describe;
pub mod filter;
pub mod format;
pub mod ops;
pub mod route;
pub mod source;
pub mod status;
pub mod store;
pub mod usage;
