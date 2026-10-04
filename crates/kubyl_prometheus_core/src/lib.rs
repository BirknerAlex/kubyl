//! The Prometheus web UI without the UI.
//!
//! - [`fetch`], [`model`]: reading a Prometheus' build, runtime, TSDB, targets, rules and
//!   service discovery, and parsing them leniently.
//! - [`servers`]: the Prometheus-compatible servers of a cluster, found and probed.
//! - [`service`]: the servers per cluster, discovery and sign-in, as a service on any
//!   [`Host`](kubyl_base::host::Host).
//! - [`complete`]: PromQL completion.
//! - [`settings`]: the `"prometheus"` settings.json section.
//!
//! `kubyl_prometheus` re-exports these modules and adds the service and the tab.

pub mod complete;
pub mod fetch;
pub mod model;
pub mod servers;
pub mod service;
pub mod settings;
