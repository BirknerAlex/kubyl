//! The explorer's sidebar sections: Favorites, then Clusters.

mod clusters;
mod favorites;

use gpui::{AnyView, App, AppContext as _, SharedString, Window};
use kubyl_core::{ChromeRegistry, SidebarSection};

pub use clusters::ClustersSection;
pub use favorites::{FavoritesSection, open_favorite};

struct FavoritesDef;

impl SidebarSection for FavoritesDef {
    fn id(&self) -> &'static str {
        "favorites"
    }

    fn title(&self) -> SharedString {
        "Favorites".into()
    }

    fn order(&self) -> i32 {
        0
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(FavoritesSection::new).into()
    }
}

struct ClustersDef;

impl SidebarSection for ClustersDef {
    fn id(&self) -> &'static str {
        "clusters"
    }

    fn title(&self) -> SharedString {
        "Clusters".into()
    }

    fn order(&self) -> i32 {
        1
    }

    fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| ClustersSection::new(window, cx)).into()
    }
}

pub(crate) fn init(cx: &mut App) {
    clusters::bind_keys(cx);
    ChromeRegistry::add_sidebar_section(cx, FavoritesDef);
    ChromeRegistry::add_sidebar_section(cx, ClustersDef);
    cx.default_global::<clusters::Sections>();
}

/// Shows the kind filter of the focused window's Clusters section.
pub(crate) fn filter(cx: &mut App) {
    // Deferred: the action handler runs inside the dispatching window's update.
    cx.defer(|cx| {
        let Some(window) = cx.active_window() else {
            return;
        };
        let section = cx
            .global::<clusters::Sections>()
            .0
            .iter()
            .find(|(handle, section)| *handle == window && section.upgrade().is_some())
            .map(|(_, section)| section.clone());
        if let Some(section) = section {
            window
                .update(cx, |_, window, cx| {
                    section
                        .update(cx, |section, cx| section.start_filter(window, cx))
                        .ok();
                })
                .ok();
        }
    });
}
