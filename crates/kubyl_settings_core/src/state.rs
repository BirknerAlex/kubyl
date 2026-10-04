//! `state.json`: what the app remembers between runs, in typed sections.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub const STATE_FILE: &str = "state.json";

/// A typed part of `state.json`, stored under [`StateSection::KEY`].
pub trait StateSection: Serialize + DeserializeOwned + Default + 'static {
    const KEY: &'static str;
}

/// The loaded `state.json`, without any UI. The app keeps one in a global
/// (`kubyl_settings::State`) and debounces saves.
pub struct StateStore {
    path: PathBuf,
    raw: Map<String, Value>,
    /// Changes not yet written.
    dirty: bool,
}

impl StateStore {
    /// Loads `state.json` from `dir`; a missing or invalid file starts empty.
    pub fn load(dir: &Path) -> Self {
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
        Self {
            path,
            raw,
            dirty: false,
        }
    }

    /// The stored value of `T`, or its default when missing or unreadable.
    pub fn get<T: StateSection>(&self) -> T {
        self.raw
            .get(T::KEY)
            .and_then(|value| {
                serde_json::from_value(value.clone())
                    .inspect_err(|err| tracing::warn!("state.json \"{}\": {err}", T::KEY))
                    .ok()
            })
            .unwrap_or_default()
    }

    /// Stores `value`. Returns `true` when a save has to be scheduled (the store wasn't dirty
    /// yet); a store that is already dirty has one pending.
    pub fn set<T: StateSection>(&mut self, value: &T) -> bool {
        let value = serde_json::to_value(value).expect("state serialize");
        if self.raw.get(T::KEY) == Some(&value) {
            return false;
        }
        self.raw.insert(T::KEY.to_string(), value);
        !std::mem::replace(&mut self.dirty, true)
    }

    /// The path and contents to write when there are unsaved changes, and marks them saved.
    pub fn take_unsaved(&mut self) -> Option<(PathBuf, Vec<u8>)> {
        std::mem::take(&mut self.dirty).then(|| {
            (
                self.path.clone(),
                serde_json::to_vec_pretty(&self.raw).expect("state serialize"),
            )
        })
    }
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

    #[test]
    fn set_asks_for_one_save_until_taken() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = StateStore::load(dir.path());
        assert!(state.set(&Layout { sidebar_width: 1.0 }));
        assert!(!state.set(&Layout { sidebar_width: 2.0 }));
        assert!(!state.set(&Layout { sidebar_width: 2.0 }));
        let (path, contents) = state.take_unsaved().unwrap();
        assert!(path.ends_with(STATE_FILE));
        assert!(String::from_utf8(contents).unwrap().contains('2'));
        assert!(state.take_unsaved().is_none());
        assert_eq!(state.get::<Layout>().sidebar_width, 2.0);
    }
}
