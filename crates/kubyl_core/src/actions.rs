//! App-wide actions that are dispatched in one crate and handled in another.
//!
//! Keeping them here avoids dependencies between feature crates: the title bar dispatches
//! [`ToggleCommandPalette`], `kubyl_palette` handles it.

use gpui::{Action, actions};
use serde::Deserialize;

use crate::registry::ViewRequest;
use crate::types::{ClusterId, ResourceRef};

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
        /// Opens the "New kubeconfig" wizard (phase 11).
        NewKubeconfig,
        /// Saves the table in focus as a CSV file (phase 25). Tables handle it in their own
        /// key context; see `kubyl_base::csv`.
        ExportCsv,
    ]
);

/// Opens a kubeconfig in the kubeconfig editor (phase 11), optionally with a context selected.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct EditKubeconfig {
    pub path: std::path::PathBuf,
    pub context: Option<String>,
}

/// Opens a tab for `request` in the active pane of the focused window.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct OpenView(pub ViewRequest);

/// Shows the dock that holds the panel with this [`DockPanel::id`](crate::DockPanel::id),
/// makes the panel the dock's active tab and focuses it.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct ActivateDockPanel(pub String);

/// Starts a port-forward to `port` of a pod, Service or workload (`kubyl_portforward` handles
/// it). A forward that already runs for the same port is reported instead of started twice.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct ForwardPort {
    pub target: ResourceRef,
    pub port: u16,
}

/// Stops the port-forward with this [`ActiveForward::id`](crate::forwards::ActiveForward::id).
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct StopForward(pub u64);

/// Asks the user's agent about something (phase 21, `kubyl_agent` handles it): selected log
/// lines, an alert, an object. Opens the agent panel with it attached to the prompt. `text` is
/// what the agent gets; the handler masks token shapes again, but senders must not put Secret
/// values in it.
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct AskAgent {
    pub cluster: ClusterId,
    /// Shown on the attachment, e.g. `42 log lines of shop/web-0`.
    pub label: String,
    /// `kubyl://…`, identifies what was attached.
    pub uri: String,
    pub text: String,
}

/// Asks the user's agent one of the canned questions about an object (phase 25, `kubyl_agent`
/// handles it): opens the Agent panel with the question in the composer, ready to send. The
/// question carries only a reference to the object (cluster, kind, namespace, name); the agent
/// reads the rest through Kubyl's read-only tools. `prompt` is a
/// `kubyl_agent_core::prompts::Prompt` id (`summarize`, `events`, `logs`, `metrics`, `related`).
#[derive(Clone, PartialEq, Debug, Deserialize, Action)]
#[action(namespace = kubyl, no_json)]
pub struct AskAgentAbout {
    pub target: ResourceRef,
    /// The object's kind (`Deployment`).
    pub kind: String,
    pub prompt: String,
}
