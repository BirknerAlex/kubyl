//! Settings and UI state persistence.
//!
//! Two JSON files live in the platform config dir (`dirs::config_dir()/kubyl/`, or
//! `$KUBYL_CONFIG_DIR` when set):
//!
//! - `settings.json`: user-editable. A JSON schema (`settings.schema.json`) is written next to
//!   it, and the file is reloaded when it changes on disk. Access with [`Settings`].
//! - `state.json`: what the app remembers (window layout, open tabs, favorites). Written by the
//!   app, debounced. Access with [`State`].
//!
//! Both are split into typed sections. A crate defines a section once and reads it anywhere:
//!
//! ```ignore
//! #[derive(Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
//! #[serde(default)]
//! struct LogSettings { wrap_lines: bool }
//!
//! impl SettingsSection for LogSettings {
//!     const KEY: Option<&'static str> = Some("logs");
//! }
//!
//! Settings::register::<LogSettings>(cx);
//! let wrap = Settings::get::<LogSettings>(cx).wrap_lines;
//! Settings::observe::<LogSettings>(cx, |settings, cx| { /* re-render */ }).detach();
//! ```
//!
//! Never store tokens, credentials or Secret data in either file.

mod paths;
mod state;
mod store;

pub use paths::config_dir;
pub use state::{State, StateSection};
pub use store::{Settings, SettingsSection};

use gpui::App;

/// Loads `settings.json` and `state.json` and starts watching `settings.json`.
pub fn init(cx: &mut App) {
    let dir = config_dir();
    store::init(cx, &dir, true);
    state::init(cx, &dir);
}

/// Like [`init`] with an explicit directory, without watching `settings.json`. Used by tests:
/// a file watcher thread would break GPUI's deterministic test scheduler.
pub fn init_with_dir(cx: &mut App, dir: &std::path::Path) {
    store::init(cx, dir, false);
    state::init(cx, dir);
}
