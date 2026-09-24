//! Extension points. Feature crates register into these from their own `init(cx)`, so
//! parallel phases never edit the same files.
//!
//! | Registry | Used for |
//! |---|---|
//! | [`ViewRegistry`] | building a tab for a [`ViewRequest`] (resource + [`ViewKind`](crate::ViewKind)) |
//! | [`ActionRegistry`] | named actions with key bindings, availability and palette/key-hint metadata |
//! | [`ResourceColumns`] | per-kind table columns (server-side `Table` is the fallback) |
//! | [`ChromeRegistry`] | [`StatusBarItem`], [`DockPanel`] and [`SidebarSection`] contributions |

mod actions;
mod chrome;
mod columns;
mod views;

pub use actions::{ActionRegistry, ActionSpec, Availability};
pub use chrome::{
    ChromeRegistry, DockPanel, DockPosition, SidebarSection, StatusBarItem, StatusBarPosition,
};
pub use columns::{
    Align, CellValue, ColumnDef, ColumnProvider, ColumnWidth, ResourceColumns, Tone,
};
pub use views::{TabHandle, TabView, ViewFactory, ViewRegistry, ViewRequest, new_tab};

use gpui::App;

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<ViewRegistry>();
    cx.default_global::<ActionRegistry>();
    cx.default_global::<ResourceColumns>();
    cx.default_global::<ChromeRegistry>();
}
