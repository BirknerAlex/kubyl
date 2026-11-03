//! Views owned by the app shell. Feature crates replace the placeholders by registering real
//! views for the same [`ViewKind`]s.

mod placeholder;
mod settings;
mod welcome;

use gpui::{App, AppContext as _};
use kubyl_core::{ViewKind, ViewRegistry};

pub use placeholder::PlaceholderView;
pub use settings::{SettingsView, open_settings_json, settings_view_kind};
pub use welcome::WelcomeView;

pub fn init(cx: &mut App) {
    ViewRegistry::register(cx, ViewKind::Welcome, |_, _, cx| {
        Some(Box::new(cx.new(WelcomeView::new)))
    });
    ViewRegistry::register(cx, settings_view_kind(), |_, window, cx| {
        Some(Box::new(cx.new(|cx| SettingsView::new(window, cx))))
    });
}
