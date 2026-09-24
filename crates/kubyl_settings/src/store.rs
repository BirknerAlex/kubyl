use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{App, AsyncApp, BorrowAppContext as _, Global, Subscription};
use kubyl_core::{Notification, NotificationCenter};
use notify::Watcher as _;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::paths::write_atomic;

pub(crate) const SETTINGS_FILE: &str = "settings.json";
pub(crate) const SCHEMA_FILE: &str = "settings.schema.json";
const DEFAULT_SETTINGS: &str = "{\n  \"$schema\": \"./settings.schema.json\"\n}\n";

/// A typed part of `settings.json`.
///
/// Use `#[serde(default)]` on the struct so missing fields fall back to their defaults.
pub trait SettingsSection:
    Serialize + DeserializeOwned + Default + JsonSchema + Clone + PartialEq + 'static
{
    /// `None`: the fields live at the top level (`"theme": "dark"`), like Zed.
    /// `Some(key)`: they live in an object under `key` (`"logs": { "wrap_lines": true }`).
    const KEY: Option<&'static str>;
}

struct Entry {
    value: Box<dyn Any>,
    key: Option<&'static str>,
    parse: fn(&Value) -> Result<Box<dyn Any>, String>,
    schema: fn() -> Value,
}

/// The loaded `settings.json`. See the crate docs.
pub struct Settings {
    path: PathBuf,
    raw: Value,
    entries: HashMap<TypeId, Entry>,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl Global for Settings {}

impl Settings {
    /// Parses `T` from the loaded file and makes it available to [`Settings::get`].
    /// Registering twice is a no-op.
    pub fn register<T: SettingsSection>(cx: &mut App) {
        let settings = cx.global_mut::<Self>();
        if settings.entries.contains_key(&TypeId::of::<T>()) {
            return;
        }
        let value = match parse::<T>(&settings.raw) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("settings.json: {err}");
                Box::new(T::default())
            }
        };
        settings.entries.insert(
            TypeId::of::<T>(),
            Entry {
                value,
                key: T::KEY,
                parse: parse::<T>,
                schema: schema::<T>,
            },
        );
    }

    /// The current value of `T`.
    ///
    /// # Panics
    /// If `T` was not registered with [`Settings::register`].
    pub fn get<T: SettingsSection>(cx: &App) -> &T {
        cx.global::<Self>()
            .entries
            .get(&TypeId::of::<T>())
            .and_then(|entry| entry.value.downcast_ref())
            .unwrap_or_else(|| {
                panic!(
                    "Settings::register::<{}>() was not called",
                    std::any::type_name::<T>()
                )
            })
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
        let mut value = Self::get::<T>(cx).clone();
        f(&mut value);
        let contents = cx.update_global::<Self, _>(|settings, _| {
            merge_section(&mut settings.raw, T::KEY, &value);
            if let Some(entry) = settings.entries.get_mut(&TypeId::of::<T>()) {
                entry.value = Box::new(value);
            }
            serde_json::to_vec_pretty(&settings.raw).expect("settings serialize")
        });
        let path = cx.global::<Self>().path.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = write_atomic(&path, &contents) {
                    tracing::error!("failed to write {}: {err}", path.display());
                }
            })
            .detach();
    }

    /// Path of `settings.json`.
    pub fn path(cx: &App) -> &Path {
        &cx.global::<Self>().path
    }

    /// Writes `settings.schema.json` for all registered sections. Call once after every
    /// crate's `init`.
    pub fn write_schema(cx: &App) {
        let settings = cx.global::<Self>();
        let schema = combined_schema(settings.entries.values());
        let path = settings.path.with_file_name(SCHEMA_FILE);
        cx.background_executor()
            .spawn(async move {
                let json = serde_json::to_vec_pretty(&schema).expect("schema serialize");
                if let Err(err) = write_atomic(&path, &json) {
                    tracing::warn!("failed to write {}: {err}", path.display());
                }
            })
            .detach();
    }

    /// Replaces the file contents and re-parses every section. Invalid JSON keeps the old values.
    pub fn reload_from_str(cx: &mut App, contents: &str) {
        let raw: Value = match serde_json::from_str(contents) {
            Ok(raw @ Value::Object(_)) => raw,
            Ok(_) => return report(cx, "settings.json must contain a JSON object".into()),
            Err(err) => return report(cx, format!("settings.json is not valid JSON: {err}")),
        };
        if raw == cx.global::<Self>().raw {
            return;
        }
        let errors = cx.update_global::<Self, _>(|settings, _| {
            let mut errors = Vec::new();
            for entry in settings.entries.values_mut() {
                match (entry.parse)(&raw) {
                    Ok(value) => entry.value = value,
                    Err(err) => errors.push(err),
                }
            }
            settings.raw = raw;
            errors
        });
        for err in errors {
            report(cx, format!("settings.json: {err}"));
        }
    }
}

fn report(cx: &mut App, message: String) {
    tracing::warn!("{message}");
    NotificationCenter::push(cx, Notification::error(message));
}

fn parse<T: SettingsSection>(raw: &Value) -> Result<Box<dyn Any>, String> {
    let section = match T::KEY {
        None => raw.clone(),
        Some(key) => raw.get(key).cloned().unwrap_or(Value::Null),
    };
    if section.is_null() {
        return Ok(Box::new(T::default()));
    }
    T::deserialize(section)
        .map(|value| Box::new(value) as Box<dyn Any>)
        .map_err(|err| match T::KEY {
            Some(key) => format!("\"{key}\": {err}"),
            None => err.to_string(),
        })
}

fn schema<T: SettingsSection>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serialize")
}

/// Writes the fields of `value` into `raw`, skipping default-valued fields that the user never set.
fn merge_section<T: SettingsSection>(raw: &mut Value, key: Option<&str>, value: &T) {
    let Value::Object(new) = serde_json::to_value(value).expect("settings serialize") else {
        return;
    };
    let Value::Object(defaults) = serde_json::to_value(T::default()).expect("settings serialize")
    else {
        return;
    };
    let root = raw.as_object_mut().expect("settings root is an object");
    let target = match key {
        None => root,
        Some(key) => root
            .entry(key)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .expect("settings section is an object"),
    };
    for (field, value) in new {
        if defaults.get(&field) == Some(&value) && !target.contains_key(&field) {
            continue;
        }
        target.insert(field, value);
    }
}

fn combined_schema<'a>(entries: impl Iterator<Item = &'a Entry>) -> Value {
    let mut properties = Map::new();
    let mut defs = Map::new();
    properties.insert("$schema".into(), serde_json::json!({ "type": "string" }));
    for entry in entries {
        let mut schema = (entry.schema)();
        let Some(obj) = schema.as_object_mut() else {
            continue;
        };
        obj.remove("$schema");
        if let Some(Value::Object(d)) = obj.remove("$defs") {
            defs.extend(d);
        }
        match entry.key {
            None => {
                if let Some(Value::Object(props)) = obj.remove("properties") {
                    properties.extend(props);
                }
            }
            Some(key) => {
                properties.insert(key.into(), schema);
            }
        }
    }
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Kubyl settings",
        "type": "object",
        "properties": properties,
        "$defs": defs,
    })
}

/// Loads settings.json from `dir`; with `watch`, reloads it when it changes on disk.
pub(crate) fn init(cx: &mut App, dir: &Path, watch: bool) {
    let path = dir.join(SETTINGS_FILE);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Err(err) = write_atomic(&path, DEFAULT_SETTINGS.as_bytes()) {
                tracing::warn!("failed to create {}: {err}", path.display());
            }
            DEFAULT_SETTINGS.to_string()
        }
        Err(err) => {
            tracing::warn!("failed to read {}: {err}", path.display());
            DEFAULT_SETTINGS.to_string()
        }
    };
    let (raw, error) = match serde_json::from_str::<Value>(&contents) {
        Ok(raw @ Value::Object(_)) => (raw, None),
        Ok(_) => (
            Value::Object(Map::new()),
            Some("settings.json must contain a JSON object".to_string()),
        ),
        Err(err) => (
            Value::Object(Map::new()),
            Some(format!("settings.json is not valid JSON: {err}")),
        ),
    };

    let (tx, rx) = futures::channel::mpsc::unbounded();
    let watcher = if !watch {
        None
    } else {
        notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|e| !e.kind.is_access()) {
                tx.unbounded_send(()).ok();
            }
        })
        .and_then(|mut watcher| {
            watcher.watch(dir, notify::RecursiveMode::NonRecursive)?;
            Ok(watcher)
        })
        .inspect_err(|err| tracing::warn!("not watching settings.json: {err}"))
        .ok()
    };

    cx.set_global(Settings {
        path: path.clone(),
        raw,
        entries: HashMap::new(),
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
    use serde::Deserialize;
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
            let schema = combined_schema(cx.global::<Settings>().entries.values());
            let props = schema["properties"].as_object().unwrap();
            assert!(props.contains_key("theme"));
            assert!(props.contains_key("font_size"));
            assert!(props["logs"]["properties"]["wrap_lines"].is_object());
        });
    }
}
