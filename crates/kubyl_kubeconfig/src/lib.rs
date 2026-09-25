//! Kubeconfig editor: clusters, credentials, contexts, connection test and the "New
//! kubeconfig" wizard.

pub mod certs;
pub mod files;
pub mod model;
pub mod tls;
pub mod yaml;

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(_cx: &mut App) {}
