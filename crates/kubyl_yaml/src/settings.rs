//! The `"yaml"` settings.json section and the apply history (from `kubyl_yaml_core`), read
//! from and written to the app's state.json.

use gpui::App;
use kubyl_settings::State;
pub use kubyl_yaml_core::settings::*;

/// The recorded applies of several keys (an entry id and the ids its contexts had before they
/// were grouped), newest first, each apply once.
pub fn history_of(cx: &App, keys: &[String]) -> Vec<HistoryEntry> {
    State::get::<ApplyHistory>(cx).combined(keys)
}

/// Records an apply in state.json (never Secrets or Route keys, see [`recordable`]).
pub fn record_apply(cx: &mut App, key: String, kind: &str, yaml: String) {
    let mut history = State::get::<ApplyHistory>(cx);
    if history.record(key, kind, yaml) {
        State::set(cx, &history);
    }
}
