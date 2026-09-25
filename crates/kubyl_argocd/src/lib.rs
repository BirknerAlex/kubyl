//! Argo CD (phase 10): applications with sync and health, a resource tree, history and
//! rollback, sync and refresh, ApplicationSets and Projects. Shown only when the cluster serves
//! an Argo CD CRD.

pub mod api;
pub mod detect;
pub mod diff;
pub mod health;
pub mod links;
pub mod model;
pub mod ops;
pub mod windows;

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(_cx: &mut App) {}
