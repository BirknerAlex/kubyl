//! User-facing notifications (toasts).
//!
//! Any crate can post a notification with [`NotificationCenter::push`] (or
//! [`NotifyResultExt::notify_err`] on a `Result`) without a window. Every open window observes the
//! center and shows new entries as toasts, so crates never deal with window or overlay plumbing.

use std::collections::VecDeque;

use gpui::{App, Global, SharedString};

/// Severity of a notification. Drives the toast's icon and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// One toast.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub level: NotificationLevel,
    pub title: Option<SharedString>,
    pub message: SharedString,
}

impl Notification {
    pub fn new(level: NotificationLevel, message: impl Into<SharedString>) -> Self {
        Self {
            level,
            title: None,
            message: message.into(),
        }
    }

    pub fn info(message: impl Into<SharedString>) -> Self {
        Self::new(NotificationLevel::Info, message)
    }

    pub fn success(message: impl Into<SharedString>) -> Self {
        Self::new(NotificationLevel::Success, message)
    }

    pub fn warning(message: impl Into<SharedString>) -> Self {
        Self::new(NotificationLevel::Warning, message)
    }

    pub fn error(message: impl Into<SharedString>) -> Self {
        Self::new(NotificationLevel::Error, message)
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }
}

/// App-wide queue of recent notifications. Windows render entries newer than the last id they saw.
#[derive(Default)]
pub struct NotificationCenter {
    next_id: u64,
    recent: VecDeque<(u64, Notification)>,
}

impl Global for NotificationCenter {}

/// How many notifications are kept for windows that haven't rendered them yet.
const MAX_RECENT: usize = 32;

impl NotificationCenter {
    /// Posts a notification to every open window.
    pub fn push(cx: &mut App, notification: Notification) {
        if notification.level == NotificationLevel::Error {
            tracing::warn!(message = %notification.message, "error notification");
        }
        let center = cx.default_global::<Self>();
        center.next_id += 1;
        let id = center.next_id;
        center.recent.push_back((id, notification));
        while center.recent.len() > MAX_RECENT {
            center.recent.pop_front();
        }
    }

    /// Notifications with an id greater than `after`, oldest first.
    pub fn since(&self, after: u64) -> impl Iterator<Item = &(u64, Notification)> {
        self.recent.iter().filter(move |(id, _)| *id > after)
    }

    /// The id of the newest notification, or 0.
    pub fn latest_id(&self) -> u64 {
        self.next_id
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

/// Shows the error of a `Result` as a toast and turns it into an `Option`.
pub trait NotifyResultExt<T> {
    fn notify_err(self, cx: &mut App) -> Option<T>;
}

impl<T, E: std::fmt::Display> NotifyResultExt<T> for Result<T, E> {
    fn notify_err(self, cx: &mut App) -> Option<T> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                NotificationCenter::push(cx, Notification::error(err.to_string()));
                None
            }
        }
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.default_global::<NotificationCenter>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn windows_see_only_new_notifications(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            NotificationCenter::push(cx, Notification::info("one"));
            let seen = NotificationCenter::global(cx).latest_id();
            NotificationCenter::push(cx, Notification::warning("two"));
            let err: Result<(), &str> = Err("three");
            assert_eq!(err.notify_err(cx), None);

            let new: Vec<_> = NotificationCenter::global(cx)
                .since(seen)
                .map(|(_, n)| n.message.to_string())
                .collect();
            assert_eq!(new, ["two", "three"]);
        });
    }
}
