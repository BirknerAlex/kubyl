//! Operators (OLM) and Helm releases (phase 12, board 7).
//!
//! - [`olm`]: OLM v0 objects and how they join into installed operators, the upgrade review,
//!   OperatorHub's packages, installs and uninstalls; OLM v1 in [`olm::v1`]. Data, no UI.
//! - [`helm`]: Helm releases, read-only: decoding, masking, the metadata watches.
//! - [`service::Olm`]: the shared OLM watches per cluster, OperatorHub's cache, reviews and the
//!   writes that outlive a dialog.
//! - [`api`]: installed operators with their version constraints, for phase 13.
//! - [`view`]: the Operators tab (Installed · Install plans · Subscriptions · Helm releases ·
//!   Extensions). [`hub`]: the OperatorHub tab. [`release`]: a Helm release's tab.
//!   [`dialogs`]: install, review and approve, uninstall, create instance, helm commands.

pub mod api;
pub mod dialogs;
pub mod errors;
pub mod helm;
pub mod hub;
pub mod olm;
pub mod release;
pub mod service;
pub mod view;
mod widgets;

use gpui::App;

pub use service::Olm;

/// Registers this crate's views, actions and services.
///
/// Must run after `kubyl_kube::init`, `kubyl_resources::init`, `kubyl_explorer::init` and
/// `kubyl_yaml::init`.
pub fn init(cx: &mut App) {
    service::Olm::install(true, cx);
    helm::service::Helm::install(true, cx);
    view::init(cx);
    hub::init(cx);
    release::init(cx);
}
