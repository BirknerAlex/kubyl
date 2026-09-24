//! Views owned by the app shell. Feature crates replace the placeholders by registering real
//! views for the same [`ViewKind`]s.

mod placeholder;
mod welcome;

use gpui::{Action, App, AppContext as _, Window};
use kubyl_core::{Notification, NotificationCenter, ViewKind, ViewRegistry};

pub use placeholder::PlaceholderView;
pub use welcome::WelcomeView;

pub fn init(cx: &mut App) {
    ViewRegistry::register(cx, ViewKind::Welcome, |_, _, cx| {
        Some(Box::new(cx.new(WelcomeView::new)))
    });
}

/// Dispatches `action`, or explains in a toast that `feature` isn't available yet when no
/// crate handles it.
pub fn dispatch_or_explain(
    action: Box<dyn Action>,
    feature: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if window.is_action_available(action.as_ref(), cx) {
        window.dispatch_action(action, cx);
    } else {
        NotificationCenter::push(
            cx,
            Notification::info(format!("{feature} is not available in this build yet.")),
        );
    }
}
