//! OLM operators, OperatorHub and Helm releases (phase 12, board 7).

pub mod errors;
pub mod helm;
pub mod olm;

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(_cx: &mut App) {}
