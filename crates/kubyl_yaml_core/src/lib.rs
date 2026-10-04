//! YAML without the UI.
//!
//! - [`parse`]: a lossless YAML model with paths, ranges and comments; [`render`]: objects to
//!   YAML the way Kubyl shows them; [`diff`]: line diffs.
//! - [`schema`], [`validate`], [`intel`]: OpenAPI schemas, validation, and completion and
//!   hover data.
//! - [`templates`]: new-resource templates; [`apply`]: server-side apply and conflicts.
//! - [`settings`]: the `"yaml"` settings.json section and the apply history.
//!
//! `kubyl_yaml` re-exports these modules and adds the editor, its LSP adapter and the
//! cluster schema cache.

pub mod apply;
pub mod diff;
pub mod intel;
pub mod parse;
pub mod render;
pub mod schema;
pub mod settings;
pub mod templates;
pub mod validate;
