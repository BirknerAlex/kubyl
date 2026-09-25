//! Pod filesystem browser and transfers (board 9).

pub mod entry;
pub mod listing;
pub mod local;
pub mod mounts;
pub mod queue;
pub mod remote;
pub mod settings;
pub mod transfer;

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    kubyl_settings::Settings::register::<settings::FilesSettings>(cx);
    queue::TransferQueue::install(cx);
}
