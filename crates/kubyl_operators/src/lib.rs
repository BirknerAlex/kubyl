//! Operators (OLM) and Helm releases (phase 12, board 7).
//!
//! - [`olm`]: OLM v0 objects and how they join into installed operators, the upgrade review,
//!   OperatorHub's packages, installs and uninstalls; OLM v1 in [`olm::v1`]. Data, no UI.
//! - [`helm`]: Helm releases, from `kubyl_helm` (phase 22 moved them; the paths stay).
//! - [`service::Olm`]: the shared OLM watches per cluster, OperatorHub's cache, reviews and the
//!   writes that outlive a dialog.
//! - [`api`]: installed operators with their version constraints, for phase 13.
//! - [`view`]: the Operators tab (Installed · Install plans · Subscriptions · Helm releases ·
//!   Extensions; the Helm sub-tab renders `kubyl_helm::releases`). [`hub`]: the OperatorHub
//!   tab. [`release`]: a Helm release's tab (`kubyl_helm::release`).
//!   [`dialogs`]: install, review and approve, uninstall, create instance.

pub mod api;

pub use kubyl_operators_core::{errors, olm};
pub mod dialogs;
pub mod hub;
pub mod service;
pub mod view;
mod widgets;

use gpui::App;

pub use kubyl_helm::release;
pub use service::Olm;

/// Helm releases: phase 22 moved them to `kubyl_helm`; the old paths stay.
pub mod helm {
    pub use kubyl_helm::{decode, present, service};
    pub use kubyl_helm_core::release;
}

/// Registers this crate's views, actions and services.
///
/// Must run after `kubyl_kube::init`, `kubyl_resources::init`, `kubyl_explorer::init` and
/// `kubyl_yaml::init`. The Helm sub-tab needs `kubyl_helm::init` (any order).
pub fn init(cx: &mut App) {
    service::Olm::install(true, cx);
    view::init(cx);
    hub::init(cx);
}
