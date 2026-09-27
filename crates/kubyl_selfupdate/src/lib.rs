//! Kubyl's own self-update: a signed manifest check, background download and verify, and a
//! "Restart to update" status bar item once a new version is staged.
//!
//! - [`manifest`]: the signed update manifest format and this build's platform key into it.
//! - [`verify`]: checks the manifest's ed25519 (minisign) signature against Kubyl's release key.
//! - [`download`]: fetches the manifest and, once verified, the platform artifact.
//! - [`installed`]: whether this install is a package manager's (Homebrew, apt/dnf/pacman,
//!   winget, Flatpak, snap) — those skip self-update entirely and point at their own tool.
//! - [`apply`]: extracts the new binary and replaces the running executable with it.
//! - [`service`]: the app-wide [`service::SelfUpdate`] global — the poll loop and state machine.
//! - [`settings`]: the `updates_app` (channel, auto-check) `settings.json` section.
//! - [`ui`]: the status bar item.
//!
//! Linux and Windows package-manager installs (`.deb`/`.rpm`/`.pkg.tar.zst`, winget) and
//! Flatpak/snap never self-update — `installed::detect` recognizes them and the service just
//! never checks. A manual `.dmg`/`.zip`/`.tar.gz` install self-updates on a 6-hour poll when
//! `updates_app.auto_check` is on (the default).

pub mod apply;
pub mod download;
pub mod installed;
pub mod manifest;
pub mod service;
pub mod settings;
pub mod ui;
pub mod verify;

use gpui::App;
use kubyl_core::ChromeRegistry;

pub use manifest::{Channel, UpdateManifest};
pub use service::{SelfUpdate, UpdateState};

pub fn init(cx: &mut App) {
    kubyl_settings::Settings::register::<settings::SelfUpdateSettings>(cx);
    SelfUpdate::install(true, cx);
    ChromeRegistry::add_status_item(cx, ui::SelfUpdateStatusItem);
}
