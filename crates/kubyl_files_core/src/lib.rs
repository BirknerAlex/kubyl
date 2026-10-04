//! Pod files without the UI.
//!
//! - [`listing`], [`entry`]: listing directories in containers (through exec) and their entries.
//! - [`remote`], [`local`], [`mounts`]: the container and local sides, and which paths are
//!   mounted volumes.
//! - [`transfer`]: copying files and folders as tar streams.
//! - [`settings`]: the `"files"` settings.json section.
//!
//! `kubyl_files` re-exports these modules and adds the file browser view and transfer queue.

pub mod entry;
pub mod listing;
pub mod local;
pub mod mounts;
pub mod remote;
pub mod settings;
pub mod transfer;
