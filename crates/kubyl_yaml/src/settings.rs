//! The `"yaml_editor"` section of settings.json and the local apply history in state.json.

use gpui::App;
use kubyl_settings::{SettingsSection, State, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// YAML editor settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct YamlSettings {
    /// Validate against the cluster's OpenAPI schema while typing.
    pub validate: bool,
    /// Hide `metadata.managedFields` (a code lens names the managers).
    pub hide_managed_fields: bool,
    /// Ask for confirmation (with the diff summary and the object's name typed) before applying
    /// to clusters marked as production.
    pub confirm_apply_on_prod: bool,
    /// Show the diff side by side instead of unified.
    pub side_by_side_diff: bool,
}

impl Default for YamlSettings {
    fn default() -> Self {
        Self {
            validate: true,
            hide_managed_fields: true,
            confirm_apply_on_prod: true,
            side_by_side_diff: false,
        }
    }
}

impl SettingsSection for YamlSettings {
    const KEY: Option<&'static str> = Some("yaml_editor");
}

/// How many objects and versions per object the local history keeps.
pub const HISTORY_OBJECTS: usize = 50;
pub const HISTORY_VERSIONS: usize = 10;
/// Larger documents aren't kept.
pub const HISTORY_MAX_BYTES: usize = 64 * 1024;

/// Kubyl's own applies of kinds without a server-side history. Secrets are never recorded.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApplyHistory {
    /// Most recently applied first.
    pub objects: Vec<ObjectHistory>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectHistory {
    /// `<cluster id>|<group>/<resource>|<namespace>|<name>`.
    pub key: String,
    /// Newest first.
    pub entries: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryEntry {
    /// RFC 3339.
    pub applied_at: String,
    /// The applied document, as the user wrote it.
    pub yaml: String,
}

impl StateSection for ApplyHistory {
    const KEY: &'static str = "yaml_history";
}

impl ApplyHistory {
    pub fn key(
        cluster: &str,
        group: &str,
        resource: &str,
        namespace: Option<&str>,
        name: &str,
    ) -> String {
        format!(
            "{cluster}|{group}/{resource}|{}|{name}",
            namespace.unwrap_or("")
        )
    }

    pub fn entries(cx: &App, key: &str) -> Vec<HistoryEntry> {
        State::get::<Self>(cx)
            .objects
            .into_iter()
            .find(|o| o.key == key)
            .map(|o| o.entries)
            .unwrap_or_default()
    }

    /// The entries of several keys (an entry id and the ids its contexts had before they were
    /// grouped), newest first, each apply once.
    pub fn entries_of(cx: &App, keys: &[String]) -> Vec<HistoryEntry> {
        let history = State::get::<Self>(cx);
        history.combined(keys)
    }

    fn combined(&self, keys: &[String]) -> Vec<HistoryEntry> {
        let mut entries: Vec<HistoryEntry> = keys
            .iter()
            .filter_map(|key| self.objects.iter().find(|o| &o.key == key))
            .flat_map(|o| o.entries.iter().cloned())
            .collect();
        // RFC 3339 in UTC sorts as text; newest first.
        entries.sort_by(|a, b| b.applied_at.cmp(&a.applied_at));
        let mut seen = std::collections::HashSet::new();
        entries.retain(|e| seen.insert((e.applied_at.clone(), e.yaml.clone())));
        entries.truncate(HISTORY_VERSIONS);
        entries
    }

    /// Records an apply. `kind` guards against ever storing Secret data.
    pub fn record(cx: &mut App, key: String, kind: &str, yaml: String) {
        if kind == "Secret" || yaml.len() > HISTORY_MAX_BYTES {
            return;
        }
        let entry = HistoryEntry {
            applied_at: jiff::Timestamp::now().to_string(),
            yaml,
        };
        State::update::<Self>(cx, |history| history.push(key, entry));
    }

    fn push(&mut self, key: String, entry: HistoryEntry) {
        let mut object = match self.objects.iter().position(|o| o.key == key) {
            Some(ix) => self.objects.remove(ix),
            None => ObjectHistory {
                key,
                entries: Vec::new(),
            },
        };
        if object.entries.first().is_some_and(|e| e.yaml == entry.yaml) {
            object.entries[0].applied_at = entry.applied_at;
        } else {
            object.entries.insert(0, entry);
            object.entries.truncate(HISTORY_VERSIONS);
        }
        self.objects.insert(0, object);
        self.objects.truncate(HISTORY_OBJECTS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histories_of_an_entry_and_its_former_contexts_combine() {
        let entry = |at: &str, yaml: &str| HistoryEntry {
            applied_at: at.into(),
            yaml: yaml.into(),
        };
        let history = ApplyHistory {
            objects: vec![
                ObjectHistory {
                    key: "group:c,u@/k/|/configmaps|ns|x".into(),
                    entries: vec![entry("2026-09-26T12:00:00Z", "v3")],
                },
                ObjectHistory {
                    key: "a/c/u@/k|/configmaps|ns|x".into(),
                    entries: vec![
                        entry("2026-09-25T12:00:00Z", "v2"),
                        entry("2026-09-24T12:00:00Z", "v1"),
                    ],
                },
                ObjectHistory {
                    key: "b/c/u@/k|/configmaps|ns|x".into(),
                    entries: vec![
                        entry("2026-09-25T12:00:00Z", "v2"),
                        entry("2026-09-23T12:00:00Z", "v0"),
                    ],
                },
            ],
        };
        let keys = [
            "group:c,u@/k/|/configmaps|ns|x".to_string(),
            "a/c/u@/k|/configmaps|ns|x".to_string(),
            "b/c/u@/k|/configmaps|ns|x".to_string(),
        ];
        let yamls: Vec<String> = history
            .combined(&keys)
            .into_iter()
            .map(|e| e.yaml)
            .collect();
        assert_eq!(
            yamls,
            ["v3", "v2", "v1", "v0"],
            "all of them, the duplicate once"
        );
    }

    #[test]
    fn history_keeps_the_newest_versions() {
        let mut history = ApplyHistory::default();
        for i in 0..(HISTORY_VERSIONS + 3) {
            history.push(
                "c|/configmaps|ns|x".into(),
                HistoryEntry {
                    applied_at: i.to_string(),
                    yaml: format!("v{i}"),
                },
            );
        }
        // Re-applying the same text only refreshes the time.
        history.push(
            "c|/configmaps|ns|x".into(),
            HistoryEntry {
                applied_at: "now".into(),
                yaml: format!("v{}", HISTORY_VERSIONS + 2),
            },
        );
        assert_eq!(history.objects.len(), 1);
        let entries = &history.objects[0].entries;
        assert_eq!(entries.len(), HISTORY_VERSIONS);
        assert_eq!(entries[0].applied_at, "now");
    }
}
