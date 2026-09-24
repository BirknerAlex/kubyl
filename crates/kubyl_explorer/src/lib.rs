//! Sidebar tree, favorites, resource lists and the details dock (phase 02).
//!
//! - [`sidebar`]: the Favorites and Clusters sections (registered as `SidebarSection`s).
//! - [`list::ResourceListView`]: `ViewKind::Table` for any kind, and the Favorites workspace.
//! - [`details`]: the Details dock panel and the Details / Describe tabs.
//! - [`actions`]: delete, kill, scale, restart, undo, pause, cordon, drain, trigger… with key
//!   bindings in the `ResourceList` key context.
//! - [`namespaces`]: the namespace switcher (`SwitchNamespace`).
//! - [`dialogs`]: confirmations (typed on PROD clusters), revision picker; other crates reuse
//!   them for their own confirmations.
//! - [`favorites`]: the favorites model (state.json).

pub mod actions;
pub mod catalog;
pub mod details;
pub mod dialogs;
pub mod favorites;
pub mod list;
pub mod namespaces;
pub mod settings;
pub mod sidebar;

use gpui::{App, AppContext as _, actions};
use kubyl_core::actions::{FilterSidebar, OpenView, SwitchNamespace};
use kubyl_core::{
    ActionRegistry, ActionSpec, ChromeRegistry, ClusterId, Gvr, ResourceRef, ViewKind,
    ViewRegistry, ViewRequest,
};
use kubyl_settings::Settings;

use details::{DetailsDock, DetailsView, Mode, describe_view_kind};
use list::ResourceListView;

actions!(
    explorer,
    [
        /// Opens the Favorites workspace: every favorite's pods in one table.
        OpenFavoritesWorkspace,
    ]
);

/// View kind of the Favorites workspace (merged table across favorites).
pub fn favorites_view_kind() -> ViewKind {
    ViewKind::Custom("favorites".into())
}

/// Registers the explorer's views, sidebar sections, dock panel and actions.
pub fn init(cx: &mut App) {
    Settings::register::<settings::ExplorerSettings>(cx);
    favorites::Favorites::install(cx);
    cx.default_global::<list::PendingFilter>();
    list::init(cx);
    actions::init(cx);
    sidebar::init(cx);
    namespaces::init(cx);

    ViewRegistry::register(cx, ViewKind::Table, |request, window, cx| {
        let target = request.target.clone()?;
        Some(Box::new(
            cx.new(|cx| ResourceListView::new(target, window, cx)),
        ))
    });
    ViewRegistry::register(cx, ViewKind::Details, |request, _, cx| {
        let target = request.target.clone().filter(ResourceRef::is_object)?;
        Some(Box::new(
            cx.new(|cx| DetailsView::new(target, Mode::Summary, cx)),
        ))
    });
    ViewRegistry::register(cx, describe_view_kind(), |request, _, cx| {
        let target = request.target.clone().filter(ResourceRef::is_object)?;
        Some(Box::new(
            cx.new(|cx| DetailsView::new(target, Mode::Describe, cx)),
        ))
    });
    ViewRegistry::register(cx, favorites_view_kind(), |request, window, cx| {
        let gvr = request
            .target
            .as_ref()
            .map(|t| t.gvr.clone())
            .unwrap_or_else(|| Gvr::new("", "v1", "pods"));
        Some(Box::new(
            cx.new(|cx| ResourceListView::favorites(gvr, window, cx)),
        ))
    });
    ChromeRegistry::add_dock_panel(cx, DetailsDock);

    cx.on_action(|_: &SwitchNamespace, cx| actions::with_window(cx, namespaces::open));
    cx.on_action(|_: &FilterSidebar, cx| sidebar::filter(cx));
    cx.on_action(|_: &OpenFavoritesWorkspace, cx| {
        actions::with_window(cx, |window, cx| {
            window.dispatch_action(
                Box::new(OpenView(ViewRequest::for_resource(
                    favorites_view_kind(),
                    ResourceRef::list(ClusterId::new(""), Gvr::new("", "v1", "pods"), None),
                ))),
                cx,
            )
        })
    });
    for spec in [
        ActionSpec::new("Namespaces: Switch Namespace…", SwitchNamespace),
        ActionSpec::new("Favorites: Open Workspace", OpenFavoritesWorkspace),
        ActionSpec::new("Explorer: Filter Kinds", FilterSidebar),
    ] {
        ActionRegistry::register(cx, spec);
    }
}
