//! App-wide actions that are dispatched in one crate and handled in another.
//!
//! Keeping them here avoids dependencies between feature crates: the title bar dispatches
//! [`ToggleCommandPalette`], `kubyl_palette` handles it.

use gpui::{Action, actions};
use serde::Deserialize;

use crate::registry::ViewRequest;

actions!(
    kubyl,
    [
        /// Opens the command palette (phase 03).
        ToggleCommandPalette,
        /// Opens the cluster switcher (phase 01).
        SwitchCluster,
        /// Opens the namespace switcher (phase 02).
        SwitchNamespace,
        /// Opens the "add kubeconfig" flow (phase 01).
        AddKubeconfig,
        /// Shows the notification history.
        ShowNotifications,
        /// Opens settings.json.
        OpenSettings,
        /// Shows the filter of the sidebar (the Explorer header's search button, phase 02).
        FilterSidebar,
    ]
);

/// Opens a tab for `request` in the active pane of the focused window.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct OpenView(pub ViewRequest);

/// Shows the dock that holds the panel with this [`DockPanel::id`](crate::DockPanel::id),
/// makes the panel the dock's active tab and focuses it.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct ActivateDockPanel(pub String);
