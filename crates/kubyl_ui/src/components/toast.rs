use gpui::{App, Window};
use gpui_component::WindowExt as _;
use gpui_component::notification::{Notification as Toast, NotificationType};
use kubyl_core::{Notification, NotificationLevel};

/// Shows a [`kubyl_core::Notification`] as a toast in `window`.
///
/// The window must have gpui-component's `Root` as its root view.
pub fn show_notification(notification: &Notification, window: &mut Window, cx: &mut App) {
    let kind = match notification.level {
        NotificationLevel::Info => NotificationType::Info,
        NotificationLevel::Success => NotificationType::Success,
        NotificationLevel::Warning => NotificationType::Warning,
        NotificationLevel::Error => NotificationType::Error,
    };
    let mut toast = Toast::new()
        .message(notification.message.clone())
        .with_type(kind);
    if let Some(title) = &notification.title {
        toast = toast.title(title.clone());
    }
    window.push_notification(toast, cx);
}
