//! Views owned by the app shell. Feature crates replace the placeholders by registering real
//! views for the same [`ViewKind`]s.

mod placeholder;
mod welcome;

use gpui::{App, AppContext as _};
use kubyl_core::{ViewKind, ViewRegistry};

pub use placeholder::PlaceholderView;
pub use welcome::WelcomeView;

pub fn init(cx: &mut App) {
    ViewRegistry::register(cx, ViewKind::Welcome, |_, _, cx| {
        Some(Box::new(cx.new(WelcomeView::new)))
    });
}
