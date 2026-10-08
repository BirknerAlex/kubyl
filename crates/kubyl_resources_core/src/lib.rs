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
//! - [`errors`]: user-facing messages for failed API requests (a 403 names the verb, resource and
//!   scope).
//! - [`format`]: ages, quantities and JSON helpers.
//! - [`usage`]: CPU and memory usage as metrics providers report it.
//! - [`route`]: OpenShift Routes (model, URL, weights, target port resolution, key masking).
//! - [`dra`], [`admission`], [`gateway`], [`vpa`]: device resources (DRA), admission policies
//!   and webhooks, the Gateway API and VerticalPodAutoscalers, read the same across their API
//!   versions (phase 24).
//! - [`redact`]: masking for the clipboard and agents (Secret values, Route keys, Helm release
//!   storage, token shapes in text).
//!
//! `kubyl_resources` re-exports these modules and adds the GPUI side: shared store entities,
//! column providers, the selection and the metrics provider registry.

pub mod admission;
pub mod columns;
pub mod describe;
pub mod dra;
pub mod errors;
pub mod filter;
pub mod format;
pub mod gateway;
pub mod ops;
pub mod redact;
pub mod route;
pub mod source;
pub mod status;
pub mod store;
pub mod usage;
pub mod vpa;
