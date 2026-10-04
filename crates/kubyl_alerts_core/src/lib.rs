//! Alerts without the UI.
//!
//! - [`cache`]: the alerts cache's data: counts, phases, sources, and reading and merging.
//! - [`discover`], [`client`]: finding Alertmanagers and Prometheus rule APIs and talking to them.
//! - [`model`], [`matchers`], [`merge`]: alerts, silences and rules, label matchers, merging
//!   several sources.
//! - [`service`]: [`service::AlertsCore`], the demand-driven cache on a `kubyl_base::Host`.
//! - [`rows`]: the alert list's rows and filters.
//! - [`settings`]: the `"alerts"` settings.json section.
//!
//! `kubyl_alerts` re-exports these modules and wraps the service in an entity with the views and
//! dialogs.

pub mod cache;
pub mod client;
pub mod discover;
pub mod matchers;
pub mod merge;
pub mod model;
pub mod rows;
pub mod service;
pub mod settings;
