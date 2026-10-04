//! Self-update without the UI.
//!
//! - [`manifest`], [`verify`]: the ed25519-signed update manifest and its check.
//! - [`download`], [`apply`]: downloading an artifact and replacing the running binary.
//! - [`installed`]: how this copy was installed (package manager installs don't self-update).
//! - [`service`]: [`SelfUpdateCore`](service::SelfUpdateCore), the check/download state machine.
//! - [`settings`]: the `"self_update"` settings.json section.
//!
//! `kubyl_selfupdate` re-exports these modules and adds the update service and its UI.

pub mod apply;
pub mod download;
pub mod installed;
pub mod manifest;
pub mod service;
pub mod settings;
pub mod verify;
