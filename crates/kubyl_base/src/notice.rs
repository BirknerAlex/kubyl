//! User-facing messages from core services, without any UI attached.
//!
//! A service hands a [`Notice`] to its host; the app turns it into a toast
//! (`kubyl_core::Notification`). Messages must never contain tokens, client keys or Secret data.

/// Severity of a notice. Drives the toast's icon and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
}

/// A message for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub level: NotificationLevel,
    pub title: Option<String>,
    pub message: String,
}

impl Notice {
    pub fn new(level: NotificationLevel, message: impl Into<String>) -> Self {
        Self {
            level,
            title: None,
            message: message.into(),
        }
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Info, message)
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Success, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Warning, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Error, message)
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}
