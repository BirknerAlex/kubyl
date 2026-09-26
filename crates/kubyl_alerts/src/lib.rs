//! Alerts (phase 14, board 16): what fires in each cluster (Alertmanager, and Prometheus'
//! rules API for pending alerts and rule health), silences, alerting rules.

pub mod client;
pub mod discover;
pub mod matchers;
pub mod merge;
pub mod model;
pub mod settings;

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(_cx: &mut App) {}
