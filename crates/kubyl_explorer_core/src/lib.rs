//! The resource explorer without the UI.
//!
//! - [`rbac`]: the access checks behind actions.
//! - [`settings`]: the `"explorer"` settings.json section.
//!
//! `kubyl_explorer` re-exports these modules and adds the sidebar, lists, details and dialogs.

pub mod rbac;
pub mod settings;
