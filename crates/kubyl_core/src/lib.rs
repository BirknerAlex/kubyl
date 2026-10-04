//! Shared foundation for every Kubyl crate.
//!
//! - [`runtime`]: the Tokio runtime and [`spawn_kube`], the bridge from GPUI to Kubernetes work.
//! - [`types`], [`error`], [`notice`]: re-exported from `kubyl_base`, the GPUI-free foundation
//!   ([`ClusterId`], [`ResourceRef`], [`ViewKind`]…, the error type, plain notices).
//! - [`notify`]: the user-facing notification pipeline (toasts).
//! - [`host`]: [`GpuiHost`](host::GpuiHost), which runs `kubyl_base` core services in entities.
//! - [`registry`]: extension points that feature crates register into from their `init(cx)`.
//! - [`context`]: the active cluster/namespace shown in the window chrome.
//! - [`actions`]: app-wide actions that several crates dispatch or handle.
//! - [`forwards`]: the port-forwards running now, for crates that show them.
//! - [`cluster_ids`]: resolving cluster ids that may be out of date (grouped contexts).

pub mod actions;
pub mod cluster_ids;
pub mod context;
pub mod forwards;
pub mod host;
pub mod notify;
pub mod registry;
pub mod runtime;

pub use kubyl_base::{error, notice, types};

pub use cluster_ids::ClusterIds;
pub use context::{ActiveContext, ClusterBadge, TabContext, TabNamespace};
pub use kubyl_base::{Error, Notice, Result};
pub use notify::{
    Notification, NotificationAction, NotificationCenter, NotificationLevel, NotifyResultExt,
};
pub use registry::{
    ActionRegistry, ActionSpec, Align, CellAction, CellButton, CellValue, ChromeRegistry,
    ColumnDef, ColumnProvider, ColumnWidth, DetailsSection, DockPanel, DockPosition, EditNotice,
    OverviewSection, ResourceColumns, SidebarSection, StatusBarItem, StatusBarPosition, TabHandle,
    TabView, Tone, ViewFactory, ViewRegistry, ViewRequest, new_tab,
};
pub use runtime::spawn_kube;
pub use types::{ArgoCdCaps, ClusterCaps, ClusterId, ContextName, Gvk, Gvr, ResourceRef, ViewKind};

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
