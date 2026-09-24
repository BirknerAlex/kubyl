//! Zed-like components, styled after the mockups.

mod button;
mod chip;
mod data_table;
mod dock;
mod keys;
mod modal;
mod sidebar;
mod status;
mod status_bar;
mod tab_bar;
mod title_bar;
mod toast;

pub use button::{Button, ButtonVariant, IconButton};
pub use chip::Chip;
pub use data_table::{DataTable, DataTableEvent, TableDelegate};
pub use dock::DockHeader;
pub use keys::{Kbd, KeyHints, format_keystroke};
pub use modal::Modal;
pub use sidebar::{PanelHeader, SectionHeader, TreeRow};
pub use status::{ProgressBar, StatusDot, StatusPill, tone_color};
pub use status_bar::{StatusBar, StatusBarText};
pub use tab_bar::{Tab, TabBar};
pub use title_bar::{ProdBadge, TitleBar};
pub use toast::show_notification;

use gpui::App;

/// Re-exported layout helpers from gpui-component.
pub use gpui_component::{h_flex, v_flex};

pub(crate) fn init(cx: &mut App) {
    data_table::init(cx);
}
