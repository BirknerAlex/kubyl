//! The command palette without the UI.
//!
//! - [`command`]: prefixed commands (`:deploy -A`, `@context`, `#namespace`, `>action`…).
//! - [`matcher`]: fuzzy matching and scoring.
//! - [`references`]: references to other objects found in a resource (Secrets, ConfigMaps…).
//!
//! `kubyl_palette` re-exports these modules and adds the palette, its items and recent entries.

pub mod command;
pub mod matcher;
pub mod references;
