use std::path::Path;
use std::time::Duration;

use gpui::{App, Global};
pub use kubyl_settings_core::StateSection;
use kubyl_settings_core::{StateStore, WriteTicket, wait_for_writes};

const SAVE_DELAY: Duration = Duration::from_millis(500);
/// How long quitting waits for writes still running in the background.
const QUIT_WAIT: Duration = Duration::from_secs(5);

/// What the app remembers between runs (`state.json`). Saves are debounced.
pub struct State {
    store: StateStore,
}

impl Global for State {}

impl State {
    /// The stored value of `T`, or its default when missing or unreadable.
    pub fn get<T: StateSection>(cx: &App) -> T {
        cx.global::<Self>().store.get::<T>()
    }

    /// Stores `value` and schedules a save.
    pub fn set<T: StateSection>(cx: &mut App, value: &T) {
        if cx.global_mut::<Self>().store.set(value) {
            Self::schedule_save(cx);
        }
    }

    /// Reads, changes and stores `T`.
    pub fn update<T: StateSection>(cx: &mut App, f: impl FnOnce(&mut T)) {
        let mut value = Self::get::<T>(cx);
        f(&mut value);
        Self::set(cx, &value);
    }

    /// Writes pending changes now, on the calling thread. Use on quit.
    pub fn flush(cx: &mut App) {
        if let Some((path, contents)) = cx.global_mut::<Self>().store.take_unsaved()
            && let Err(err) = WriteTicket::new().write(&path, &contents)
        {
            tracing::error!("failed to write {}: {err}", path.display());
        }
    }

    fn schedule_save(cx: &mut App) {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            // `None`: already written by `flush`.
            let Some((path, contents, ticket)) = cx.update(|cx| {
                let (path, contents) = cx.global_mut::<Self>().store.take_unsaved()?;
                Some((path, contents, WriteTicket::new()))
            }) else {
                return;
            };
            cx.background_executor()
                .spawn(async move {
                    if let Err(err) = ticket.write(&path, &contents) {
                        tracing::error!("failed to write {}: {err}", path.display());
                    }
                })
                .await;
        })
        .detach();
    }
}

pub(crate) fn init(cx: &mut App, dir: &Path) {
    cx.set_global(State {
        store: StateStore::load(dir),
    });
    cx.on_app_quit(|cx| {
        State::flush(cx);
        // A debounced save or a Settings::update may still be in flight on the executor.
        cx.background_executor()
            .spawn(async { wait_for_writes(QUIT_WAIT) })
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_settings_core::STATE_FILE;
    use serde::{Deserialize, Serialize};

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
