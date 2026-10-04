//! `settings.json`: typed sections, parsing, writing back and the JSON schema.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::paths::write_atomic;

pub const SETTINGS_FILE: &str = "settings.json";
pub const SCHEMA_FILE: &str = "settings.schema.json";
const DEFAULT_SETTINGS: &str = "{\n  \"$schema\": \"./settings.schema.json\"\n}\n";

/// A typed part of `settings.json`.
///
/// Use `#[serde(default)]` on the struct so missing fields fall back to their defaults.
pub trait SettingsSection:
    Serialize + DeserializeOwned + Default + JsonSchema + Clone + PartialEq + Send + Sync + 'static
{
    /// `None`: the fields live at the top level (`"theme": "dark"`), like Zed.
    /// `Some(key)`: they live in an object under `key` (`"logs": { "wrap_lines": true }`).
    const KEY: Option<&'static str>;
}

struct Entry {
    value: Box<dyn Any + Send + Sync>,
    key: Option<&'static str>,
    parse: fn(&Value) -> Result<Box<dyn Any + Send + Sync>, String>,
    schema: fn() -> Value,
}

/// The loaded `settings.json`, without any UI: the app keeps one in a global
/// (`kubyl_settings::Settings`) and tells views when sections change.
pub struct SettingsStore {
    path: PathBuf,
    raw: Value,
    /// The file on disk could not be read or parsed. `raw` does not reflect it, so writing it
    /// back would destroy what the user has there; changes stay in memory until it is fixed.
    unreadable: bool,
    entries: HashMap<TypeId, Entry>,
}

/// What [`SettingsStore::update`] wants written, or why it can't be.
pub enum Write {
    /// Write `contents` to `path` (off the UI thread, with a `WriteTicket`).
    File { path: PathBuf, contents: Vec<u8> },
    /// The file on disk has errors; the change stays in memory.
    Unreadable(PathBuf),
}

impl SettingsStore {
    /// Loads `settings.json` from `dir`, creating it when missing. The second value is an error
    /// to show the user (the file has invalid JSON; a backup was made).
    pub fn load(dir: &Path) -> (Self, Option<String>) {
        let path = dir.join(SETTINGS_FILE);
        let mut unreadable = false;
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
                unreadable = true;
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

        if error.is_some() {
            unreadable = true;
            // Keep a copy in case the user's next edit replaces it.
            let backup = path.with_file_name(format!("{SETTINGS_FILE}.bak"));
            if let Err(err) = std::fs::copy(&path, &backup) {
                tracing::warn!("failed to back up {}: {err}", path.display());
            }
        }
        let store = Self {
            path,
            raw,
            unreadable,
            entries: HashMap::new(),
        };
        (store, error)
    }

    /// Parses `T` from the loaded file and makes it available to [`SettingsStore::get`].
    /// Registering twice is a no-op.
    pub fn register<T: SettingsSection>(&mut self) {
        if self.entries.contains_key(&TypeId::of::<T>()) {
            return;
        }
        let value = match parse::<T>(&self.raw) {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!("settings.json: {err}");
                Box::new(T::default())
            }
        };
        self.entries.insert(
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
    /// If `T` was not registered with [`SettingsStore::register`].
    pub fn get<T: SettingsSection>(&self) -> &T {
        self.entries
            .get(&TypeId::of::<T>())
            .and_then(|entry| entry.value.downcast_ref())
            .unwrap_or_else(|| {
                panic!(
                    "Settings::register::<{}>() was not called",
                    std::any::type_name::<T>()
                )
            })
    }

    /// Changes `T` and returns what to write back: the fields that differ from their defaults.
    pub fn update<T: SettingsSection>(&mut self, f: impl FnOnce(&mut T)) -> Write {
        let mut value = self.get::<T>().clone();
        f(&mut value);
        merge_section(&mut self.raw, T::KEY, &value);
        if let Some(entry) = self.entries.get_mut(&TypeId::of::<T>()) {
            entry.value = Box::new(value);
        }
        if self.unreadable {
            return Write::Unreadable(self.path.clone());
        }
        Write::File {
            path: self.path.clone(),
            contents: serde_json::to_vec_pretty(&self.raw).expect("settings serialize"),
        }
    }

    /// Path of `settings.json`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The combined schema of all registered sections and where to write it.
    pub fn schema(&self) -> (PathBuf, Value) {
        (
            self.path.with_file_name(SCHEMA_FILE),
            combined_schema(self.entries.values()),
        )
    }

    /// Replaces the file contents and re-parses every section. Invalid JSON keeps the old values
    /// (and makes [`SettingsStore::update`] stop writing until the file is valid again).
    /// Returns the errors to show the user.
    pub fn reload_from_str(&mut self, contents: &str) -> Vec<String> {
        let raw: Value = match serde_json::from_str(contents) {
            Ok(raw @ Value::Object(_)) => raw,
            Ok(_) => {
                self.unreadable = true;
                return vec!["settings.json must contain a JSON object".into()];
            }
            Err(err) => {
                self.unreadable = true;
                return vec![format!("settings.json is not valid JSON: {err}")];
            }
        };
        self.unreadable = false;
        if raw == self.raw {
            return Vec::new();
        }
        let mut errors = Vec::new();
        for entry in self.entries.values_mut() {
            match (entry.parse)(&raw) {
                Ok(value) => entry.value = value,
                Err(err) => errors.push(format!("settings.json: {err}")),
            }
        }
        self.raw = raw;
        errors
    }
}

fn parse<T: SettingsSection>(raw: &Value) -> Result<Box<dyn Any + Send + Sync>, String> {
    let section = match T::KEY {
        None => raw.clone(),
        Some(key) => raw.get(key).cloned().unwrap_or(Value::Null),
    };
    if section.is_null() {
        return Ok(Box::new(T::default()));
    }
    T::deserialize(section)
        .map(|value| Box::new(value) as Box<dyn Any + Send + Sync>)
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
        Some(key) => {
            let section = root.entry(key).or_insert_with(|| Value::Object(Map::new()));
            // `null`, `[]` etc. typed by hand: replace with an object.
            if !section.is_object() {
                *section = Value::Object(Map::new());
            }
            section.as_object_mut().expect("just made an object")
        }
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

/// Watches `dir` (not recursively) and calls `on_change` for every event that isn't a plain
/// read. `None` when the platform watcher can't be started (logged).
pub fn watch_dir(
    dir: &Path,
    on_change: impl Fn() + Send + 'static,
) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher as _;
    notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok_and(|e| !e.kind.is_access()) {
            on_change();
        }
    })
    .and_then(|mut watcher| {
        watcher.watch(dir, notify::RecursiveMode::NonRecursive)?;
        Ok(watcher)
    })
    .inspect_err(|err| tracing::warn!("not watching settings.json: {err}"))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

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

    fn store(contents: &str) -> (tempfile::TempDir, SettingsStore) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SETTINGS_FILE), contents).unwrap();
        let (mut store, _) = SettingsStore::load(dir.path());
        store.register::<Flat>();
        store.register::<Logs>();
        (dir, store)
    }

    #[test]
    fn reads_flat_and_keyed_sections() {
        let (_dir, store) =
            store(r#"{ "theme": "light", "font_size": 14, "logs": { "wrap_lines": true } }"#);
        assert_eq!(store.get::<Flat>().theme, "light");
        assert_eq!(store.get::<Flat>().font_size, 14.0);
        assert!(store.get::<Logs>().wrap_lines);
    }

    #[test]
    fn update_writes_only_changed_fields() {
        let (_dir, mut store) = store(r#"{ "$schema": "./settings.schema.json" }"#);
        store.update::<Logs>(|logs| logs.wrap_lines = true);
        let Write::File { contents, .. } = store.update::<Flat>(|flat| flat.theme = "light".into())
        else {
            panic!("file is readable");
        };
        let written: Value = serde_json::from_slice(&contents).unwrap();
        assert_eq!(
            written,
            serde_json::json!({
                "$schema": "./settings.schema.json",
                "logs": { "wrap_lines": true },
                "theme": "light",
            })
        );
    }

    #[test]
    fn invalid_reload_keeps_values_and_stops_writes() {
        let (_dir, mut store) = store(r#"{ "theme": "light" }"#);
        let errors = store.reload_from_str(r#"{ "theme": "#);
        assert_eq!(errors.len(), 1);
        assert_eq!(store.get::<Flat>().theme, "light");
        assert!(matches!(
            store.update::<Logs>(|logs| logs.wrap_lines = true),
            Write::Unreadable(_)
        ));
    }

    #[test]
    fn schema_combines_sections() {
        let (_dir, store) = store("{}");
        let (path, schema) = store.schema();
        assert!(path.ends_with(SCHEMA_FILE));
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("theme"));
        assert!(props.contains_key("font_size"));
        assert!(props["logs"]["properties"]["wrap_lines"].is_object());
    }
}
