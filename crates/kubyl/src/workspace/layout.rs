//! The window layout persisted in `state.json`.

use kubyl_core::{ViewKind, ViewRequest};
use kubyl_settings::StateSection;
use kubyl_ui::sizes;
use serde::{Deserialize, Serialize};

/// Open/closed state and size of a dock (unscaled pixels).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DockLayout {
    pub visible: bool,
    pub size: f32,
}

/// Split direction of a pane group node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    /// Side by side.
    Horizontal,
    /// Stacked.
    Vertical,
}

/// The center area: a tree of splits with panes of tabs at the leaves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PaneLayout {
    Pane {
        tabs: Vec<ViewRequest>,
        #[serde(default)]
        active: usize,
    },
    Split {
        axis: SplitAxis,
        children: Vec<PaneLayout>,
    },
}

/// Everything about a window that survives a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceLayout {
    /// Window size in logical pixels. The window opens centered.
    pub window_size: Option<(f32, f32)>,
    pub window_maximized: bool,
    pub sidebar: DockLayout,
    pub right_dock: DockLayout,
    pub bottom_dock: DockLayout,
    pub center: PaneLayout,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            window_size: None,
            window_maximized: false,
            sidebar: DockLayout {
                visible: true,
                size: sizes::SIDEBAR_WIDTH,
            },
            right_dock: DockLayout {
                visible: true,
                size: sizes::RIGHT_DOCK_WIDTH,
            },
            bottom_dock: DockLayout {
                visible: false,
                size: sizes::BOTTOM_DOCK_HEIGHT,
            },
            center: PaneLayout::Pane {
                tabs: vec![ViewRequest::new(ViewKind::Welcome)],
                active: 0,
            },
        }
    }
}

impl StateSection for WorkspaceLayout {
    const KEY: &'static str = "workspace";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips_through_json() {
        let layout = WorkspaceLayout {
            center: PaneLayout::Split {
                axis: SplitAxis::Horizontal,
                children: vec![
                    PaneLayout::Pane {
                        tabs: vec![ViewRequest::new(ViewKind::Welcome)],
                        active: 0,
                    },
                    PaneLayout::Pane {
                        tabs: vec![ViewRequest::new(ViewKind::Custom("x".into()))],
                        active: 0,
                    },
                ],
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&layout).unwrap();
        assert_eq!(
            serde_json::from_str::<WorkspaceLayout>(&json).unwrap(),
            layout
        );
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let layout: WorkspaceLayout =
            serde_json::from_str(r#"{ "sidebar": { "visible": false, "size": 300 } }"#).unwrap();
        assert!(!layout.sidebar.visible);
        assert_eq!(layout.right_dock, WorkspaceLayout::default().right_dock);
    }
}
