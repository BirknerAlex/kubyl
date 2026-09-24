//! The cluster and namespace the user is working in, shown by the title and status bars.
//!
//! `kubyl_kube`/`kubyl_explorer` set it; the chrome in `kubyl` only reads it.

use gpui::{App, Global, Hsla, SharedString};

use crate::types::ClusterId;

/// How a cluster is presented in the chrome.
#[derive(Clone, Debug, PartialEq)]
pub struct ClusterBadge {
    pub id: ClusterId,
    /// Display name, usually the context name.
    pub name: SharedString,
    /// Accent color of the cluster root (sidebar icon, favorites dot).
    pub color: Hsla,
    /// Shows the red PROD badge.
    pub production: bool,
    /// Short description such as `EKS · v1.30.4`.
    pub meta: Option<SharedString>,
    /// Connection is healthy (green dot) or not (red dot).
    pub connected: bool,
}

/// The active cluster and namespace. `None` means nothing is selected yet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActiveContext {
    pub cluster: Option<ClusterBadge>,
    /// `None` means all namespaces.
    pub namespace: Option<SharedString>,
}

impl Global for ActiveContext {}

impl ActiveContext {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Replaces the active context. Observers (`cx.observe_global::<ActiveContext>`) re-render.
    pub fn set(cx: &mut App, context: ActiveContext) {
        cx.set_global(context);
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<ActiveContext>();
}
