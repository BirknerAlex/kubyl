//! Service web views without the UI.
//!
//! - [`target`]: what a web view opens (Service port, Ingress, Route) and whether it speaks
//!   HTTP.
//! - [`cert`]: certificates of HTTPS targets.
//!
//! `kubyl_webview` re-exports these modules and adds the native web views, forwards and
//! pickers.

pub mod cert;
pub mod target;
