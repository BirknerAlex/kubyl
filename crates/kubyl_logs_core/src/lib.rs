//! Log streams without the UI.
//!
//! - [`stream`]: multi-pod log streaming with reconnects at `sinceTime` and selector sources.
//! - [`ring`], [`line`]: the bounded line buffer and its lines.
//! - [`level`], [`json`]: level detection and JSON log parsing.
//! - [`search`]: find in the buffer.
//! - [`settings`]: the `"logs"` settings.json section.
//!
//! `kubyl_logs` re-exports these modules and adds the log view, the dock and the active
//! sessions registry.

pub mod json;
pub mod level;
pub mod line;
pub mod ring;
pub mod search;
pub mod settings;
pub mod stream;
