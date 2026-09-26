//! Alerts (phase 14, board 16): what fires in each cluster (Alertmanager, and Prometheus'
//! rules API for pending alerts and rule health), silences, alerting rules.
//!
//! - [`discover`] and [`client`]: finding and reaching Alertmanager (service proxy, OpenShift
//!   Route or a temporary forward for trusted Services only, external URLs with a keychain
//!   header).
//! - [`model`], [`matchers`], [`merge`]: parsing, label matchers, one list from both sources.
//! - [`service`]: the demand-driven cache ([`service::AlertsService`]).
//! - [`view`]: the Alerts tab (Alerts, Silences, Rules) and the all-clusters tab.
//! - [`silence`]: the silence editor, its summary and the writes.
//! - `chrome`: the sidebar row, root marker, status bar item, details section, overview card.

mod actions;
mod chrome;
pub mod client;
pub mod discover;
pub mod matchers;
pub mod merge;
pub mod model;
pub mod service;
pub mod settings;
pub mod silence;
pub mod view;

use gpui::App;
use kubyl_settings::Settings;

pub use service::AlertsService;

/// Registers settings, installs the service, the views, actions and chrome contributions.
///
/// Must run after `kubyl_kube::init`, `kubyl_explorer::init` and `kubyl_metrics::init`.
pub fn init(cx: &mut App) {
    Settings::register::<settings::AlertsSettings>(cx);
    AlertsService::install(true, cx);
    view::init(cx);
    actions::init(cx);
    chrome::init(cx);
}

/// Tells the chrome (sidebar badges and markers) that alert data changed.
pub(crate) fn changed(cx: &mut App) {
    kubyl_explorer::catalog::view_rows_changed(cx);
}
