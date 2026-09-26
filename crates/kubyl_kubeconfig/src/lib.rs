//! Kubeconfig editor (phase 11, board 11): create and edit kubeconfigs in a GUI, test a
//! context before saving, and save without breaking the file for kubectl.
//!
//! - Data, no UI: [`model`] (the document), [`yaml`] (comment-preserving writer), [`files`]
//!   (atomic saves, backups, hash check), [`certs`], [`validate`], [`schema`] (YAML tab),
//!   [`tls`] (TLS checks, CA fetch), [`conntest`] (the step-by-step connection test).
//! - [`state::Kubeconfigs`]: tests, OIDC sign-ins, exec consent and drafts; they outlive dialogs.
//! - UI: [`editor`] (the tab; rendering in `editor_ui`, `forms`, `yaml_tab`), [`dialogs`],
//!   [`wizard`] ("New kubeconfig"), [`import`] (service accounts, cloud CLIs), [`panel`]
//!   (test results).
//!
//! Other crates open it with `kubyl_core::actions::{NewKubeconfig, EditKubeconfig}`.

pub mod actions;
pub mod certs;
pub mod conntest;
mod dialogs;
pub mod editor;
#[cfg(test)]
mod editor_tests;
mod editor_ui;
pub mod files;
pub(crate) mod forms;
pub mod import;
#[cfg(test)]
mod live_ui_tests;
pub mod model;
pub mod panel;
pub mod schema;
pub mod settings;
pub mod state;
pub mod tls;
pub mod validate;
pub mod widgets;
pub mod wizard;
pub mod yaml;
mod yaml_tab;

use gpui::{App, AppContext as _};
use kubyl_core::ViewRegistry;
use kubyl_settings::Settings;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    Settings::register::<settings::KubeconfigSettings>(cx);
    state::Kubeconfigs::install(cx);
    ViewRegistry::register(cx, editor::view_kind(), |request, window, cx| {
        let path = request.path.clone()?;
        Some(Box::new(
            cx.new(|cx| editor::KubeconfigEditor::new(path, window, cx)),
        ))
    });
    actions::init(cx);
}
