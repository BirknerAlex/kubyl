//! Kubeconfig editing without the UI.
//!
//! - [`yaml`], [`model`]: a lossless kubeconfig document and its typed model; [`files`]: which
//!   files may be edited, backups and writes.
//! - [`validate`], [`schema`]: problems and hover docs.
//! - [`certs`], [`tls`]: certificates, and probing a server's TLS.
//! - [`conntest`]: the connection test.
//!
//! `kubyl_kubeconfig` re-exports these modules and adds the editor, the wizard and dialogs.

pub mod certs;
pub mod conntest;
pub mod files;
pub mod model;
pub mod schema;
pub mod tls;
pub mod validate;
pub mod yaml;
