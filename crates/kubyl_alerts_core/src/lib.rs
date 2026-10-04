//! Alerts without the UI.
//!
//! - [`discover`], [`client`]: finding Alertmanagers and Prometheus rule APIs and talking to them.
//! - [`model`], [`matchers`], [`merge`]: alerts, silences and rules, label matchers, merging
//!   several sources.
//! - [`rows`]: the alert list's rows and filters.
//! - [`settings`]: the `"alerts"` settings.json section.
//!
//! `kubyl_alerts` re-exports these modules and adds the alerts service, views and dialogs.

pub mod client;
pub mod discover;
pub mod matchers;
pub mod merge;
pub mod model;
pub mod rows;
pub mod settings;
