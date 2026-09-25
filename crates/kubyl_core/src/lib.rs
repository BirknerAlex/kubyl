//! Shared foundation for every Kubyl crate.
//!
//! - [`runtime`]: the Tokio runtime and [`spawn_kube`], the bridge from GPUI to Kubernetes work.
//! - [`types`]: IDs and references shared across crates ([`ClusterId`], [`ResourceRef`], [`ViewKind`]…).
//! - [`error`] and [`notify`]: the error type and the user-facing notification pipeline.
//! - [`registry`]: extension points that feature crates register into from their `init(cx)`.
//! - [`context`]: the active cluster/namespace shown in the window chrome.
//! - [`actions`]: app-wide actions that several crates dispatch or handle.
//! - [`forwards`]: the port-forwards running now, for crates that show them.

pub mod actions;
pub mod context;
pub mod error;
pub mod forwards;
pub mod notify;
pub mod registry;
pub mod runtime;
pub mod types;

pub use context::{ActiveContext, ClusterBadge};
pub use error::{Error, Result};
pub use notify::{Notification, NotificationCenter, NotificationLevel, NotifyResultExt};
pub use registry::{
    ActionRegistry, ActionSpec, Align, CellValue, ChromeRegistry, ColumnDef, ColumnProvider,
    ColumnWidth, DetailsSection, DockPanel, DockPosition, ResourceColumns, SidebarSection,
    StatusBarItem, StatusBarPosition, TabHandle, TabView, Tone, ViewFactory, ViewRegistry,
    ViewRequest, new_tab,
};
pub use runtime::spawn_kube;
pub use types::{ClusterCaps, ClusterId, ContextName, Gvk, Gvr, ResourceRef, ViewKind};

use gpui::App;

/// Installs the globals this crate owns (registries, notification center, active context).
///
/// Must run before any feature crate's `init`.
pub fn init(cx: &mut App) {
    registry::init(cx);
    notify::init(cx);
    context::init(cx);
    forwards::ActiveForwards::init(cx);
}
