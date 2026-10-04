use std::path::Path;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{App, AsyncApp, BorrowAppContext as _, Global, Subscription};
use kubyl_core::{Notification, NotificationCenter};
pub use kubyl_settings_core::SettingsSection;
use kubyl_settings_core::{SettingsStore, Write, WriteTicket, watch_dir, write_atomic};

/// The loaded `settings.json`. See the crate docs.
pub struct Settings {
    store: SettingsStore,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Global for Settings {}

impl Settings {
    /// Parses `T` from the loaded file and makes it available to [`Settings::get`].
    /// Registering twice is a no-op.
    pub fn register<T: SettingsSection>(cx: &mut App) {
        cx.global_mut::<Self>().store.register::<T>();
    }

    /// The current value of `T`.
    ///
    /// # Panics
    /// If `T` was not registered with [`Settings::register`].
    pub fn get<T: SettingsSection>(cx: &App) -> &T {
        cx.global::<Self>().store.get::<T>()
    }

    /// Calls `on_change` whenever `T` changes (file edited, or [`Settings::update`]).
    pub fn observe<T: SettingsSection>(
        cx: &mut App,
        on_change: impl Fn(&T, &mut App) + 'static,
    ) -> Subscription {
        let mut last = Self::get::<T>(cx).clone();
        cx.observe_global::<Self>(move |cx| {
            let current = Self::get::<T>(cx).clone();
            if current != last {
                last = current.clone();
                on_change(&current, cx);
            }
        })
    }

    /// Changes `T` and writes the fields that differ from their defaults back to settings.json.
    pub fn update<T: SettingsSection>(cx: &mut App, f: impl FnOnce(&mut T)) {
        let write = cx.update_global::<Self, _>(|settings, _| settings.store.update::<T>(f));
        let (path, contents) = match write {
            Write::File { path, contents } => (path, contents),
            Write::Unreadable(path) => {
                tracing::warn!(
                    "not saving to {}: it has errors; fix or delete it first",
                    path.display()
                );
                return;
            }
        };
        let ticket = WriteTicket::new();
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = ticket.write(&path, &contents) {
                    tracing::error!("failed to write {}: {err}", path.display());
                }
            })
            .detach();
    }

    /// Path of `settings.json`.
    pub fn path(cx: &App) -> &Path {
        cx.global::<Self>().store.path()
    }

    /// Writes `settings.schema.json` for all registered sections. Call once after every
    /// crate's `init`.
    pub fn write_schema(cx: &App) {
        let (path, schema) = cx.global::<Self>().store.schema();
        cx.background_executor()
            .spawn(async move {
                let json = serde_json::to_vec_pretty(&schema).expect("schema serialize");
                if let Err(err) = write_atomic(&path, &json) {
                    tracing::warn!("failed to write {}: {err}", path.display());
                }
            })
            .detach();
    }

    /// Replaces the file contents and re-parses every section. Invalid JSON keeps the old values
    /// (and stops [`Settings::update`] from writing until the file is valid again).
    pub fn reload_from_str(cx: &mut App, contents: &str) {
        let errors =
            cx.update_global::<Self, _>(|settings, _| settings.store.reload_from_str(contents));
        for err in errors {
            report(cx, err);
        }
    }
}

fn report(cx: &mut App, message: String) {
    tracing::warn!("{message}");
    NotificationCenter::push(cx, Notification::error(message));
}

/// Loads settings.json from `dir`; with `watch`, reloads it when it changes on disk.
pub(crate) fn init(cx: &mut App, dir: &Path, watch: bool) {
    let (store, error) = SettingsStore::load(dir);
    let path = store.path().to_path_buf();
    let (tx, rx) = futures::channel::mpsc::unbounded();
    let watcher = if watch {
        watch_dir(dir, move || {
            tx.unbounded_send(()).ok();
        })
    } else {
        None
    };
    cx.set_global(Settings {
        store,
        _watcher: watcher,
    });
    if let Some(error) = error {
        report(cx, error);
    }

    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut rx = rx;
        while rx.next().await.is_some() {
            // Editors write in several steps; wait for the burst to settle.
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            while rx.try_recv().is_ok() {}
            let path = path.clone();
            let contents = cx
                .background_executor()
                .spawn(async move { std::fs::read_to_string(path) })
                .await;
            if let Ok(contents) = contents {
                cx.update(|cx| Settings::reload_from_str(cx, &contents));
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use kubyl_settings_core::SETTINGS_FILE;
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
    #[serde(default)]
    struct Flat {
        theme: String,
        font_size: f32,
    }

    impl SettingsSection for Flat {
        const KEY: Option<&'static str> = None;
    }

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
    #[serde(default)]
    struct Logs {
        wrap_lines: bool,
    }

    impl SettingsSection for Logs {
        const KEY: Option<&'static str> = Some("logs");
    }

    fn setup(cx: &mut gpui::TestAppContext, contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SETTINGS_FILE), contents).unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            init(cx, dir.path(), false);
            Settings::register::<Flat>(cx);
            Settings::register::<Logs>(cx);
        });
        dir
    }

    #[gpui::test]
    fn reads_flat_and_keyed_sections(cx: &mut gpui::TestAppContext) {
        let _dir = setup(
            cx,
            r#"{ "theme": "light", "font_size": 14, "logs": { "wrap_lines": true } }"#,
        );
        cx.update(|cx| {
            assert_eq!(Settings::get::<Flat>(cx).theme, "light");
            assert_eq!(Settings::get::<Flat>(cx).font_size, 14.0);
            assert!(Settings::get::<Logs>(cx).wrap_lines);
        });
    }

    #[gpui::test]
    fn missing_file_gets_created_with_defaults(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            init(cx, dir.path(), false);
            Settings::register::<Logs>(cx);
            assert!(!Settings::get::<Logs>(cx).wrap_lines);
        });
        let written = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(written.contains("settings.schema.json"));
    }

    #[gpui::test]
    fn observers_fire_only_for_their_section(cx: &mut gpui::TestAppContext) {
        let _dir = setup(cx, r#"{ "theme": "dark" }"#);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let _sub = cx.update(|cx| {
            let seen = seen.clone();
            Settings::observe::<Flat>(cx, move |flat, _| {
                seen.borrow_mut().push(flat.theme.clone())
            })
        });
        cx.update(|cx| {
            Settings::reload_from_str(cx, r#"{ "theme": "dark", "logs": { "wrap_lines": true } }"#);
        });
        cx.run_until_parked();
        assert!(seen.borrow().is_empty());
        cx.update(|cx| Settings::reload_from_str(cx, r#"{ "theme": "light" }"#));
        cx.run_until_parked();
        assert_eq!(*seen.borrow(), ["light"]);
    }

    #[gpui::test]
    fn invalid_json_keeps_old_values_and_notifies(cx: &mut gpui::TestAppContext) {
        let _dir = setup(cx, r#"{ "theme": "light" }"#);
        cx.update(|cx| {
            Settings::reload_from_str(cx, r#"{ "theme": "#);
            assert_eq!(Settings::get::<Flat>(cx).theme, "light");
            assert_eq!(NotificationCenter::global(cx).latest_id(), 1);
        });
    }

    #[gpui::test]
    fn broken_file_is_backed_up_and_never_overwritten(cx: &mut gpui::TestAppContext) {
        let broken = r#"{ "theme": "light", "logs": { "wrap_lines": tru"#;
        let dir = setup(cx, broken);
        cx.update(|cx| Settings::update::<Logs>(cx, |logs| logs.wrap_lines = true));
        cx.run_until_parked();
        let file = dir.path().join(SETTINGS_FILE);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), broken);
        let backup = dir.path().join("settings.json.bak");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), broken);

        // Once the user fixes the file, saving works again.
        cx.update(|cx| Settings::reload_from_str(cx, r#"{ "theme": "light" }"#));
        cx.update(|cx| Settings::update::<Logs>(cx, |logs| logs.wrap_lines = true));
        cx.run_until_parked();
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("wrap_lines")
        );
    }

    #[gpui::test]
    fn non_object_section_is_replaced_on_update(cx: &mut gpui::TestAppContext) {
        for bad in ["null", "[]", "3", "\"x\""] {
            let dir = setup(cx, &format!(r#"{{ "logs": {bad} }}"#));
            cx.update(|cx| Settings::update::<Logs>(cx, |logs| logs.wrap_lines = true));
            cx.run_until_parked();
            let written: Value = serde_json::from_str(
                &std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap(),
            )
            .unwrap();
            assert_eq!(
                written,
                serde_json::json!({ "logs": { "wrap_lines": true } })
            );
        }
    }

    #[gpui::test]
    fn invalid_reload_stops_writes(cx: &mut gpui::TestAppContext) {
        let dir = setup(cx, r#"{ "theme": "light" }"#);
        cx.update(|cx| Settings::reload_from_str(cx, r#"{ "theme": "#));
        cx.update(|cx| Settings::update::<Logs>(cx, |logs| logs.wrap_lines = true));
        cx.run_until_parked();
        let written = std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert_eq!(written, r#"{ "theme": "light" }"#);
    }

    #[gpui::test]
    fn update_writes_only_changed_fields(cx: &mut gpui::TestAppContext) {
        let dir = setup(cx, r#"{ "$schema": "./settings.schema.json" }"#);
        cx.update(|cx| Settings::update::<Logs>(cx, |logs| logs.wrap_lines = true));
        cx.update(|cx| Settings::update::<Flat>(cx, |flat| flat.theme = "light".into()));
        cx.run_until_parked();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap())
                .unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "$schema": "./settings.schema.json",
                "logs": { "wrap_lines": true },
                "theme": "light",
            })
        );
    }

    #[gpui::test]
    fn schema_combines_sections(cx: &mut gpui::TestAppContext) {
        let _dir = setup(cx, "{}");
        cx.update(|cx| {
            let (_, schema) = cx.global::<Settings>().store.schema();
            let props = schema["properties"].as_object().unwrap();
            assert!(props.contains_key("theme"));
            assert!(props.contains_key("font_size"));
            assert!(props["logs"]["properties"]["wrap_lines"].is_object());
        });
    }
}
