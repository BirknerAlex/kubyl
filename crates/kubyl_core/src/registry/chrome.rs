use std::sync::Arc;

use gpui::{AnyView, App, Global, SharedString, Window};

use super::views::TabHandle;
use crate::types::ResourceRef;

/// Which side of the status bar an item sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusBarPosition {
    Left,
    Right,
}

/// An item in the status bar (watch count, Prometheus state, port-forwards…).
///
/// `build` runs once per window; the returned view renders itself and re-renders on its own
/// `cx.notify()`.
pub trait StatusBarItem: 'static {
    fn id(&self) -> &'static str;
    fn position(&self) -> StatusBarPosition;
    /// Lower comes first within a side.
    fn order(&self) -> i32 {
        0
    }
    fn build(&self, window: &mut Window, cx: &mut App) -> AnyView;
}

/// Where a dock panel lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    Left,
    Right,
    Bottom,
}

/// A panel in the right or bottom dock (details, events, port-forwards…).
///
/// Panels are tabs of their dock. `build` runs once per window.
pub trait DockPanel: 'static {
    fn id(&self) -> &'static str;
    fn position(&self) -> DockPosition;
    fn order(&self) -> i32 {
        0
    }
    fn build(&self, window: &mut Window, cx: &mut App) -> Box<dyn TabHandle>;
}

/// A section of the left sidebar below the Explorer header (Favorites, Clusters…).
pub trait SidebarSection: 'static {
    fn id(&self) -> &'static str;
    fn title(&self) -> SharedString;
    fn order(&self) -> i32 {
        0
    }
    fn build(&self, window: &mut Window, cx: &mut App) -> AnyView;
}

/// A section other crates add to the Summary of an object's details (metrics charts…).
///
/// `build` runs once per shown object; the view lives while the details show it, so it can
/// keep its own state and subscriptions.
pub trait DetailsSection: 'static {
    fn id(&self) -> &'static str;
    /// Lower comes first.
    fn order(&self) -> i32 {
        0
    }
    /// The section for an object of `kind` (`Pod`, `Node`…), or `None` when it doesn't apply.
    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView>;
}

/// Contributions to the window chrome.
#[derive(Default)]
pub struct ChromeRegistry {
    status_items: Vec<Arc<dyn StatusBarItem>>,
    dock_panels: Vec<Arc<dyn DockPanel>>,
    sidebar_sections: Vec<Arc<dyn SidebarSection>>,
    details_sections: Vec<Arc<dyn DetailsSection>>,
}

impl Global for ChromeRegistry {}

impl ChromeRegistry {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn add_status_item(cx: &mut App, item: impl StatusBarItem) {
        let items = &mut cx.default_global::<Self>().status_items;
        items.push(Arc::new(item));
        items.sort_by_key(|i| i.order());
    }

    pub fn add_dock_panel(cx: &mut App, panel: impl DockPanel) {
        let panels = &mut cx.default_global::<Self>().dock_panels;
        panels.push(Arc::new(panel));
        panels.sort_by_key(|p| p.order());
    }

    pub fn add_sidebar_section(cx: &mut App, section: impl SidebarSection) {
        let sections = &mut cx.default_global::<Self>().sidebar_sections;
        sections.push(Arc::new(section));
        sections.sort_by_key(|s| s.order());
    }

    pub fn add_details_section(cx: &mut App, section: impl DetailsSection) {
        let sections = &mut cx.default_global::<Self>().details_sections;
        sections.push(Arc::new(section));
        sections.sort_by_key(|s| s.order());
    }

    pub fn details_sections(&self) -> &[Arc<dyn DetailsSection>] {
        &self.details_sections
    }

    pub fn status_items(
        &self,
        position: StatusBarPosition,
    ) -> impl Iterator<Item = &Arc<dyn StatusBarItem>> {
        self.status_items
            .iter()
            .filter(move |i| i.position() == position)
    }

    pub fn dock_panels(&self, position: DockPosition) -> impl Iterator<Item = &Arc<dyn DockPanel>> {
        self.dock_panels
            .iter()
            .filter(move |p| p.position() == position)
    }

    pub fn sidebar_sections(&self) -> &[Arc<dyn SidebarSection>] {
        &self.sidebar_sections
    }
}
