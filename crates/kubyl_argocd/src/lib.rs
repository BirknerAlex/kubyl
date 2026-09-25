//! Argo CD (phase 10, boards 12–15): applications with sync and health, a resource tree,
//! history and rollback, sync and refresh, ApplicationSets and Projects. Nothing shows unless
//! the cluster serves an Argo CD CRD; the UI follows CRDs appearing and disappearing.
//!
//! Two access modes:
//! - **Kubernetes mode** (default): reads the CRDs through Kubyl's watches and acts by patching
//!   the `Application` ([`ops`]), with the user's Kubernetes RBAC.
//! - **API mode** (after signing in): talks to `argocd-server` ([`api`]) for desired-vs-live
//!   diffs, the full resource tree and actions under Argo CD's RBAC. It only signs in to an
//!   install the user confirmed ([`state`], [`settings`]); tokens live in the OS keychain.
//!
//! - [`model`], [`health`], [`tree`], [`diff`], [`windows`], [`links`], [`apps`]: data, no UI.
//! - [`detect`]: where Argo CD is installed. [`run`]: runs an action in the right mode.
//! - [`views`]: Applications, an application's tab, ApplicationSets, Projects. [`dialogs`]:
//!   Sync, Rollback, Delete, sign-in. [`dock`]: details-dock sections and the YAML editor
//!   notice. [`columns`]: the explorer's generic tables. [`actions`]: palette and keys.

pub mod actions;
pub mod api;
pub mod apps;
pub mod columns;
pub mod detect;
pub mod dialogs;
pub mod diff;
pub mod dock;
pub mod health;
pub mod links;
pub mod model;
pub mod nav;
pub mod ops;
pub mod run;
pub mod settings;
pub mod sso;
pub mod state;
pub mod tree;
pub mod views;
pub mod web;
pub mod widgets;
pub mod windows;

use std::sync::Arc;

use gpui::{App, SharedString};
use kubyl_core::ChromeRegistry;
use kubyl_explorer::catalog::{self, TreeGroup};
use kubyl_settings::Settings;
use kubyl_ui::IconName;

use model::GROUP;
use state::ArgoCd;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(cx: &mut App) {
    Settings::register::<settings::ArgoSettings>(cx);
    ArgoCd::install(cx);
    nav::init(cx);
    views::init(cx);
    views::app::init(cx);
    actions::init(cx);
    columns::init(cx);
    web::init(cx);
    ChromeRegistry::add_details_section(cx, dock::ArgoDetails);
    ChromeRegistry::add_edit_notice(cx, dock::ArgoNotice);
    catalog::register_tree_group(
        cx,
        TreeGroup {
            id: "argocd",
            parent: "administration",
            label: "Argo CD",
            kinds: vec![
                catalog::k(GROUP, "applications", "Applications", IconName::Layers),
                catalog::k(
                    GROUP,
                    "applicationsets",
                    "ApplicationSets",
                    IconName::GitFork,
                ),
                catalog::k(GROUP, "appprojects", "Projects", IconName::FolderKanban),
            ],
            // The version of the cluster's Argo CD.
            badge: Some(Arc::new(|cluster, cx| {
                ArgoCd::try_global(cx)?
                    .read(cx)
                    .install_for(cluster, None)?
                    .version
                    .map(SharedString::from)
            })),
        },
    );
}
