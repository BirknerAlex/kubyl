//! `settings.json` and `state.json` without the UI.
//!
//! Two JSON files live in the platform config dir (`dirs::config_dir()/kubyl/`, or
//! `$KUBYL_CONFIG_DIR` when set):
//!
//! - `settings.json`: user-editable, split into typed [`SettingsSection`]s ([`SettingsStore`]).
//! - `state.json`: what the app remembers, split into [`StateSection`]s ([`StateStore`]).
//!
//! `kubyl_settings` keeps both in GPUI globals, debounces saves and notifies observers.
//! Never store tokens, credentials or Secret data in either file.

mod paths;
mod settings;
mod state;

pub use paths::{WriteTicket, config_dir, wait_for_writes, write_atomic};
pub use settings::{SCHEMA_FILE, SETTINGS_FILE, SettingsSection, SettingsStore, Write, watch_dir};
pub use state::{STATE_FILE, StateSection, StateStore};
