//! Network flows (phase 16, board 18): live flows from Cilium's Hubble Relay, Calico's Whisker
//! or NetObserv in one table and one topology, whichever backend the cluster has.
//!
//! - [`model`]: the internal [`model::Flow`]; [`filter`]: the filter language and completion;
//!   [`sanitize`]: what L7 data loses before anything stores it.
//! - [`provider`]: the [`provider::FlowProvider`] trait; [`detect`]: which backends a cluster
//!   has; [`backends`]: Hubble Relay (gRPC over a temporary forward), Calico Whisker and NetObserv
//!   (the API server's service proxy).
//! - [`service`]: the demand-driven [`service::FlowService`] (streams, ring buffers, the forward);
//!   [`buffer`]: the ring buffer; [`aggregate`]: the topology.
//! - [`view`]: the Network Flows tab; `chrome`: the sidebar row, the favorites entry and the
//!   details section; `actions`: keys and palette actions.

mod actions;
mod chrome;
pub mod service;
pub mod view;

pub use kubyl_netflow_core::{
    aggregate, backends, buffer, detect, filter, model, proto, provider, rows, sanitize, settings,
};

use gpui::App;

pub use actions::query_for;
pub use service::FlowService;
pub use view::{Pending, Tab, open};

/// Registers settings, installs the service, the view, actions and chrome contributions.
///
/// Must run after `kubyl_kube::init`, `kubyl_explorer::init`, `kubyl_portforward::init` and
/// `kubyl_metrics::init`.
pub fn init(cx: &mut App) {
    kubyl_settings::Settings::register::<settings::NetflowSettings>(cx);
    FlowService::install(true, cx);
    view::init(cx);
    actions::init(cx);
    chrome::init(cx);
}
