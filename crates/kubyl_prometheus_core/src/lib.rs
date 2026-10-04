//! The Prometheus web UI without the UI.
//!
//! - [`fetch`], [`model`]: reading a Prometheus' build, runtime, TSDB, targets, rules and
//!   service discovery, and parsing them leniently.
//! - [`complete`]: PromQL completion.
//! - [`settings`]: the `"prometheus"` settings.json section.
//!
//! `kubyl_prometheus` re-exports these modules and adds the service and the tab.

pub mod complete;
pub mod fetch;
pub mod model;
pub mod settings;
