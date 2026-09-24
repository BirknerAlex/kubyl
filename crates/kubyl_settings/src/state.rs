use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{App, Global};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::paths::write_atomic;

pub(crate) const STATE_FILE: &str = "state.json";
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// A typed part of `state.json`, stored under [`StateSection::KEY`].
pub trait StateSection: Serialize + DeserializeOwned + Default + 'static {
    const KEY: &'static str;
}

/// What the app remembers between runs (`state.json`). Saves are debounced.
pub struct State {
    path: PathBuf,
    raw: Map<String, Value>,
    /// Changes not yet written. A detached save task is pending while this is set.
    dirty: bool,
}

impl Global for State {}

impl State {
    /// The stored value of `T`, or its default when missing or unreadable.
    pub fn get<T: StateSection>(cx: &App) -> T {
        cx.global::<Self>()
            .raw
            .get(T::KEY)
            .and_then(|value| {
                serde_json::from_value(value.clone())
                    .inspect_err(|err| tracing::warn!("state.json \"{}\": {err}", T::KEY))
                    .ok()
            })
            .unwrap_or_default()
    }

    /// Stores `value` and schedules a save.
    pub fn set<T: StateSection>(cx: &mut App, value: &T) {
        let value = serde_json::to_value(value).expect("state serialize");
        let state = cx.global_mut::<Self>();
        if state.raw.get(T::KEY) == Some(&value) {
            return;
        }
        state.raw.insert(T::KEY.to_string(), value);
        Self::schedule_save(cx);
    }

    /// Reads, changes and stores `T`.
    pub fn update<T: StateSection>(cx: &mut App, f: impl FnOnce(&mut T)) {
        let mut value = Self::get::<T>(cx);
        f(&mut value);
        Self::set(cx, &value);
    }

    /// Writes pending changes now, on the calling thread. Use on quit.
    pub fn flush(cx: &mut App) {
        let state = cx.global_mut::<Self>();
        if std::mem::take(&mut state.dirty) {
            let contents = serde_json::to_vec_pretty(&state.raw).expect("state serialize");
            if let Err(err) = write_atomic(&state.path, &contents) {
                tracing::error!("failed to write {}: {err}", state.path.display());
            }
        }
    }

    fn schedule_save(cx: &mut App) {
        let state = cx.global_mut::<Self>();
        if std::mem::replace(&mut state.dirty, true) {
            return;
        }
        cx.spawn(async move |cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Some((path, contents)) = cx.update(|cx| {
                let state = cx.global_mut::<Self>();
                // Already written by `flush`.
                if !std::mem::take(&mut state.dirty) {
                    return None;
                }
                Some((
                    state.path.clone(),
                    serde_json::to_vec_pretty(&state.raw).expect("state serialize"),
                ))
            }) else {
                return;
            };
            cx.background_executor()
                .spawn(async move {
                    if let Err(err) = write_atomic(&path, &contents) {
                        tracing::error!("failed to write {}: {err}", path.display());
                    }
                })
                .await;
        })
        .detach();
    }
}

pub(crate) fn init(cx: &mut App, dir: &Path) {
    let path = dir.join(STATE_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<Value>(&contents) {
            Ok(Value::Object(map)) => map,
            _ => {
                tracing::warn!("{} is invalid; starting fresh", path.display());
                Map::new()
            }
        },
        Err(_) => Map::new(),
    };
    cx.set_global(State {
        path,
        raw,
        dirty: false,
    });
    cx.on_app_quit(|cx| {
        State::flush(cx);
        async {}
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Layout {
        sidebar_width: f32,
    }

    impl StateSection for Layout {
        const KEY: &'static str = "layout";
    }

    #[gpui::test]
    fn saves_debounced_and_reloads(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            init(cx, dir.path());
            assert_eq!(State::get::<Layout>(cx), Layout::default());
            State::update::<Layout>(cx, |l| l.sidebar_width = 250.0);
            State::update::<Layout>(cx, |l| l.sidebar_width = 268.0);
        });
        cx.run_until_parked();
        assert!(!dir.path().join(STATE_FILE).exists());
        cx.executor().advance_clock(SAVE_DELAY * 2);
        cx.run_until_parked();

        let written = std::fs::read_to_string(dir.path().join(STATE_FILE)).unwrap();
        assert!(written.contains("268"));

        cx.update(|cx| {
            init(cx, dir.path());
            assert_eq!(State::get::<Layout>(cx).sidebar_width, 268.0);
        });
    }
}
