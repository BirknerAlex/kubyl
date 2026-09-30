//! Prometheus (phase 18): the Prometheus web UI inside Kubyl, for every cluster that has a
//! Prometheus, Thanos Query or VictoriaMetrics.
//!
//! - [`service`]: which servers a cluster has ([`service::PrometheusService`]); the sidebar row
//!   shows while there is at least one.
//! - [`fetch`] and [`model`]: reads of the HTTP API (status, targets, rules, queries) and their
//!   lenient parsing.
//! - [`view`]: the tab with Overview, Query, Targets, Rules and Service Discovery, and the
//!   selector between several servers.
//! - `chrome`: the sidebar row; `actions`: palette actions.

mod actions;
mod chrome;
pub mod complete;
pub mod fetch;
pub mod model;
pub mod service;
pub mod settings;
pub mod view;

use gpui::App;
use kubyl_settings::Settings;

pub use service::PrometheusService;
pub use view::{Tab, open};

/// Registers settings, installs the service, the view, actions and the sidebar row.
///
/// Must run after `kubyl_kube::init`, `kubyl_explorer::init` and `kubyl_metrics::init`.
pub fn init(cx: &mut App) {
    Settings::register::<settings::PrometheusSettings>(cx);
    PrometheusService::install(true, cx);
    view::init(cx);
    actions::init(cx);
    chrome::init(cx);
}

/// Tells the sidebar that the servers of a cluster changed.
pub(crate) fn changed(cx: &mut App) {
    kubyl_explorer::catalog::view_rows_changed(cx);
}
