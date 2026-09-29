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

/// The last change of [`ActiveContext`] followed an activated tab.
#[derive(Default)]
struct FollowsTab(bool);

impl Global for FollowsTab {}

impl ActiveContext {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Replaces the active context. Observers (`cx.observe_global::<ActiveContext>`) re-render.
    pub fn set(cx: &mut App, context: ActiveContext) {
        cx.set_global(FollowsTab(false));
        cx.set_global(context);
    }

    /// Replaces the active context because a tab of it was activated. Views that follow the
    /// title bar's namespace keep theirs (see [`Self::follows_tab`]).
    pub fn set_from_tab(cx: &mut App, context: ActiveContext) {
        cx.set_global(FollowsTab(true));
        cx.set_global(context);
    }

    /// The last change only showed the context of an activated tab: views in other tabs don't
    /// re-scope to it.
    pub fn follows_tab(cx: &App) -> bool {
        cx.try_global::<FollowsTab>().is_some_and(|f| f.0)
    }
}

/// The cluster and namespace a tab shows, for the title bar to follow when it's activated.
#[derive(Clone, Debug, PartialEq)]
pub struct TabContext {
    pub cluster: ClusterId,
    pub namespace: TabNamespace,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TabNamespace {
    /// The view follows the title bar's namespace (or has none): leave it.
    Keep,
    All,
    One(String),
}

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<ActiveContext>();
}
