//! Cluster updates (phase 13, board 8): the current version, the updates a cluster offers,
//! pre-flight checks, and starting and tracking an update, through one provider per
//! distribution.
//!
//! - [`provider`]: the [`provider::UpdateProvider`] trait; [`detect`]: which provider a
//!   cluster gets; [`providers`]: the cloud providers behind cargo features.
//! - [`model`]: what providers report; [`version`]: version strings; [`removed`]: the bundled
//!   table of removed Kubernetes APIs; [`check`]: pre-flight results.

pub mod check;
pub mod detect;
pub mod model;
pub mod provider;
pub mod providers;
pub mod removed;
pub mod settings;
pub mod version;

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    kubyl_settings::Settings::register::<settings::UpdatesSettings>(cx);
}
