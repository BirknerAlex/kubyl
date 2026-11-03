//! Every setting of `settings.json` as pages, groups and items, for the settings view.
//!
//! Built from the combined JSON schema of the registered sections (descriptions, types and
//! defaults come from the section structs) and the file's current contents, so a new field
//! shows up without touching the view. Values the view can't edit in place (maps, lists of
//! objects) get [`Control::Json`]: "Edit in settings.json".

use serde_json::{Map, Value};

/// One section of `settings.json` (`"logs": {…}`), or the top-level fields as "General".
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// The section key; `None` for the top-level fields.
    pub key: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub groups: Vec<Group>,
}

/// Items of a page under one heading: the section's own fields (no title), then one group per
/// nested object (`"alerts": { "notify": {…} }` → "Notify").
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub title: Option<String>,
    pub description: Option<String>,
    pub items: Vec<Item>,
}

/// One setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// JSON keys from the root of `settings.json` (`["logs", "wrap_lines"]`).
    pub path: Vec<String>,
    pub title: String,
    pub description: Option<String>,
    pub control: Control,
    /// What the app uses: the file's value, else the default.
    pub value: Value,
    pub default: Value,
    /// The file sets it (it can be reset).
    pub set: bool,
}

impl Item {
    /// `logs.wrap_lines`.
    pub fn key(&self) -> String {
        self.path.join(".")
    }

    /// The setting matches a search: its key, title or description contains every word.
    pub fn matches(&self, query: &str) -> bool {
        let haystack = format!(
            "{} {} {}",
            self.key(),
            self.title,
            self.description.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        query
            .split_whitespace()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

/// How a setting is edited.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle,
    /// A whole number; `nullable`: empty means "not set".
    Integer {
        min: Option<f64>,
        max: Option<f64>,
        nullable: bool,
    },
    Number {
        nullable: bool,
    },
    Text {
        nullable: bool,
    },
    /// One of fixed strings; `nullable`: "not set" is one more choice.
    Choice {
        options: Vec<Choice>,
        nullable: bool,
    },
    /// A list of strings, edited comma-separated.
    StringList,
    /// Anything else (maps, lists of objects): edited in settings.json.
    Json,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: String,
    pub description: Option<String>,
}

/// Nested objects deeper than this are edited in settings.json.
const MAX_GROUP_DEPTH: usize = 1;

/// The pages of `schema` (from [`crate::SettingsStore::schema`]) with the values of `raw`
/// (the file's contents). "General" comes first, then the sections by title.
pub fn catalog(schema: &Value, raw: &Value) -> Vec<Page> {
    let defs = schema.get("$defs").and_then(Value::as_object);
    let resolver = Resolver { defs };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut general = Vec::new();
    let mut pages = Vec::new();
    for (key, prop) in properties {
        if key == "$schema" {
            continue;
        }
        let prop_resolved = resolver.resolve(prop);
        let is_section = prop_resolved
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| !p.is_empty());
        if is_section {
            let path = vec![key.clone()];
            let default = prop.get("default").cloned().unwrap_or(Value::Null);
            pages.push(Page {
                key: Some(key.clone()),
                title: section_title(key),
                description: description(prop).or_else(|| description(prop_resolved)),
                groups: groups(&resolver, prop_resolved, &path, &default, raw, 0),
            });
        } else {
            general.push(item(&resolver, prop, vec![key.clone()], &Value::Null, raw));
        }
    }
    pages.sort_by(|a, b| a.title.cmp(&b.title));
    if !general.is_empty() {
        pages.insert(
            0,
            Page {
                key: None,
                title: "General".into(),
                description: None,
                groups: vec![Group {
                    title: None,
                    description: None,
                    items: general,
                }],
            },
        );
    }
    pages
}

struct Resolver<'a> {
    defs: Option<&'a Map<String, Value>>,
}

impl<'a> Resolver<'a> {
    /// Follows `$ref` (also through a single-entry `allOf`).
    fn resolve(&self, mut schema: &'a Value) -> &'a Value {
        for _ in 0..8 {
            if let Some(name) = schema
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
                && let Some(target) = self.defs.and_then(|d| d.get(name))
            {
                schema = target;
                continue;
            }
            if let Some([only]) = schema
                .get("allOf")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
            {
                schema = only;
                continue;
            }
            break;
        }
        schema
    }

    /// `Option<T>` as schemars writes it: `anyOf: [T, null]`. Returns `T` and whether null is
    /// allowed.
    fn unwrap_nullable(&self, schema: &'a Value) -> (&'a Value, bool) {
        let schema = self.resolve(schema);
        if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
            let non_null: Vec<&Value> = variants.iter().filter(|v| !is_null_type(v)).collect();
            if let [only] = non_null.as_slice()
                && non_null.len() < variants.len()
            {
                return (self.resolve(only), true);
            }
        }
        (schema, false)
    }
}

fn is_null_type(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

fn groups(
    resolver: &Resolver,
    schema: &Value,
    path: &[String],
    default: &Value,
    raw: &Value,
    depth: usize,
) -> Vec<Group> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut own = Vec::new();
    let mut nested = Vec::new();
    for (key, prop) in properties {
        let mut child_path = path.to_vec();
        child_path.push(key.clone());
        let child_default = prop
            .get("default")
            .cloned()
            .or_else(|| default.get(key).cloned())
            .unwrap_or(Value::Null);
        let (inner, nullable) = resolver.unwrap_nullable(prop);
        let is_object = inner
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| !p.is_empty());
        if is_object && !nullable && depth < MAX_GROUP_DEPTH {
            let mut sub = groups(resolver, inner, &child_path, &child_default, raw, depth + 1);
            // The nested object's own fields lead its group; deeper groups follow.
            if let Some(first) = sub.first_mut()
                && first.title.is_none()
            {
                first.title = Some(title(key));
                first.description = description(prop).or_else(|| description(inner));
            } else {
                sub.insert(
                    0,
                    Group {
                        title: Some(title(key)),
                        description: description(prop).or_else(|| description(inner)),
                        items: Vec::new(),
                    },
                );
            }
            nested.extend(sub);
        } else {
            own.push(item(resolver, prop, child_path, default, raw));
        }
    }
    let mut out = Vec::new();
    if !own.is_empty() {
        out.push(Group {
            title: None,
            description: None,
            items: own,
        });
    }
    out.extend(nested.into_iter().filter(|g| !g.items.is_empty()));
    out
}

/// `parent_default`: the default of the enclosing object, for fields without their own.
fn item(
    resolver: &Resolver,
    prop: &Value,
    path: Vec<String>,
    parent_default: &Value,
    raw: &Value,
) -> Item {
    let key = path.last().cloned().unwrap_or_default();
    let default = prop
        .get("default")
        .cloned()
        .or_else(|| parent_default.get(&key).cloned())
        .unwrap_or(Value::Null);
    let current = lookup(raw, &path);
    let (inner, nullable) = resolver.unwrap_nullable(prop);
    Item {
        title: title(&key),
        description: description(prop).or_else(|| description(inner)),
        control: control(resolver, inner, nullable),
        set: current.is_some(),
        value: current.cloned().unwrap_or_else(|| default.clone()),
        default,
        path,
    }
}

fn control(resolver: &Resolver, schema: &Value, nullable: bool) -> Control {
    if let Some(options) = choices(resolver, schema) {
        return Control::Choice { options, nullable };
    }
    let (ty, type_nullable) = schema_type(schema);
    let nullable = nullable || type_nullable;
    match ty.as_deref() {
        Some("boolean") if !nullable => Control::Toggle,
        Some("integer") => Control::Integer {
            min: schema.get("minimum").and_then(Value::as_f64),
            max: schema.get("maximum").and_then(Value::as_f64),
            nullable,
        },
        Some("number") => Control::Number { nullable },
        Some("string") => Control::Text { nullable },
        Some("array")
            if schema
                .get("items")
                .map(|i| schema_type(resolver.resolve(i)).0.as_deref() == Some("string"))
                .unwrap_or(false)
                && choices(resolver, resolver.resolve(&schema["items"])).is_none() =>
        {
            Control::StringList
        }
        _ => Control::Json,
    }
}

/// `"type": "string"` or `"type": ["string", "null"]`.
fn schema_type(schema: &Value) -> (Option<String>, bool) {
    match schema.get("type") {
        Some(Value::String(ty)) => (Some(ty.clone()), false),
        Some(Value::Array(types)) => {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            let nullable = names.contains(&"null");
            let rest: Vec<&str> = names.into_iter().filter(|t| *t != "null").collect();
            match rest.as_slice() {
                [only] => (Some((*only).to_string()), nullable),
                _ => (None, nullable),
            }
        }
        _ => (None, false),
    }
}

/// The strings of an enum: `enum: [..]`, `const`, or `oneOf`/`anyOf` of those (a Rust enum
/// with documented variants).
fn choices(resolver: &Resolver, schema: &Value) -> Option<Vec<Choice>> {
    let schema = resolver.resolve(schema);
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let options: Option<Vec<Choice>> = values
            .iter()
            .map(|v| {
                v.as_str().map(|s| Choice {
                    value: s.to_string(),
                    description: None,
                })
            })
            .collect();
        return options.filter(|o| !o.is_empty());
    }
    if let Some(value) = schema.get("const").and_then(Value::as_str) {
        return Some(vec![Choice {
            value: value.to_string(),
            description: description(schema),
        }]);
    }
    let variants = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)?;
    let mut out = Vec::new();
    for variant in variants {
        let mut options = choices(resolver, variant)?;
        if options.len() == 1 && options[0].description.is_none() {
            options[0].description = description(variant);
        }
        out.extend(options);
    }
    (!out.is_empty()).then_some(out)
}

fn lookup<'a>(raw: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(raw, |value, key| value.get(key))
}

/// The first paragraph of a doc comment, on one line.
fn description(schema: &Value) -> Option<String> {
    let text = schema.get("description").and_then(Value::as_str)?;
    let paragraph = text.split("\n\n").next().unwrap_or(text);
    let joined = paragraph
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.is_empty()).then_some(joined)
}

const ACRONYMS: &[&str] = &[
    "ui", "api", "url", "urls", "yaml", "json", "mcp", "tls", "oidc", "id", "ids", "ai", "cpu",
    "dns", "http", "https", "ssh", "pdb", "crd", "crds", "ttl", "acp", "cli", "uri",
];

/// `scrollback_lines` → "Scrollback lines", `ui_font_size` → "UI font size".
pub fn title(key: &str) -> String {
    let words: Vec<String> = key
        .split('_')
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, word)| {
            if ACRONYMS.contains(&word) {
                word.to_uppercase()
            } else if i == 0 {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|c| c.to_uppercase().chain(chars).collect())
                    .unwrap_or_default()
            } else {
                word.to_string()
            }
        })
        .collect();
    words.join(" ")
}

/// Page titles of sections whose key doesn't read well on its own.
fn section_title(key: &str) -> String {
    match key {
        "argocd" => "Argo CD".into(),
        "kubernetes" => "Clusters".into(),
        "netflow" => "Network flows".into(),
        "self_update" => "Kubyl updates".into(),
        "updates" => "Cluster updates".into(),
        "agent" => "Agents".into(),
        "mcp" => "MCP server".into(),
        other => title(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "$schema": { "type": "string" },
                "theme": {
                    "description": "`dark`, `light` or `system`.",
                    "$ref": "#/$defs/ThemeChoice",
                    "default": "dark"
                },
                "ui_font_size": { "type": "number", "default": 13.0 },
                "logs": {
                    "title": "LogSettings",
                    "description": "Log viewer settings.\n\nMore text.",
                    "type": "object",
                    "properties": {
                        "wrap_lines": { "description": "Wrap long\nlines.", "type": "boolean", "default": false },
                        "tail": { "type": ["integer", "null"], "minimum": 0, "default": null },
                        "hidden": { "type": "array", "items": { "type": "string" }, "default": ["a"] },
                        "colors": { "type": "object", "additionalProperties": { "type": "string" }, "default": {} },
                        "mode": { "anyOf": [{ "$ref": "#/$defs/Mode" }, { "type": "null" }], "default": null },
                        "notify": { "$ref": "#/$defs/Notify", "default": { "enabled": true } }
                    }
                }
            },
            "$defs": {
                "ThemeChoice": { "oneOf": [
                    { "type": "string", "enum": ["dark", "light"] },
                    { "description": "Follow the OS.", "type": "string", "const": "system" }
                ]},
                "Mode": { "type": "string", "enum": ["fast", "slow"] },
                "Notify": { "type": "object", "properties": {
                    "enabled": { "type": "boolean" }
                }}
            }
        })
    }

    fn find<'a>(pages: &'a [Page], key: &str) -> &'a Item {
        pages
            .iter()
            .flat_map(|p| &p.groups)
            .flat_map(|g| &g.items)
            .find(|i| i.key() == key)
            .unwrap_or_else(|| panic!("no {key}"))
    }

    #[test]
    fn builds_pages_from_the_schema() {
        let pages = catalog(&schema(), &json!({ "logs": { "wrap_lines": true } }));
        assert_eq!(
            pages.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(),
            ["General", "Logs"]
        );
        assert_eq!(
            pages[1].description.as_deref(),
            Some("Log viewer settings.")
        );

        let theme = find(&pages, "theme");
        assert_eq!(theme.value, json!("dark"));
        assert!(!theme.set);
        let Control::Choice { options, nullable } = &theme.control else {
            panic!("theme is a choice");
        };
        assert!(!nullable);
        assert_eq!(
            options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(),
            ["dark", "light", "system"]
        );
        assert_eq!(options[2].description.as_deref(), Some("Follow the OS."));

        assert_eq!(find(&pages, "ui_font_size").title, "UI font size");
        let wrap = find(&pages, "logs.wrap_lines");
        assert_eq!(wrap.control, Control::Toggle);
        assert_eq!(wrap.description.as_deref(), Some("Wrap long lines."));
        assert!(wrap.set);
        assert_eq!(wrap.value, json!(true));
        assert!(matches!(
            find(&pages, "logs.tail").control,
            Control::Integer {
                nullable: true,
                min: Some(_),
                ..
            }
        ));
        assert_eq!(find(&pages, "logs.hidden").control, Control::StringList);
        assert_eq!(find(&pages, "logs.colors").control, Control::Json);
        assert!(matches!(
            find(&pages, "logs.mode").control,
            Control::Choice { nullable: true, .. }
        ));
    }

    #[test]
    fn nested_objects_become_groups_with_inherited_defaults() {
        let pages = catalog(&schema(), &json!({}));
        let logs = &pages[1];
        let notify = logs
            .groups
            .iter()
            .find(|g| g.title.as_deref() == Some("Notify"))
            .unwrap();
        assert_eq!(notify.items[0].key(), "logs.notify.enabled");
        assert_eq!(notify.items[0].value, json!(true));
    }

    #[test]
    fn search_matches_every_word() {
        let pages = catalog(&schema(), &json!({}));
        let wrap = find(&pages, "logs.wrap_lines");
        assert!(wrap.matches("wrap"));
        assert!(wrap.matches("logs long"));
        assert!(!wrap.matches("wrap theme"));
    }

    #[test]
    fn titles() {
        assert_eq!(title("scrollback_lines"), "Scrollback lines");
        assert_eq!(title("yaml_editor"), "YAML editor");
        assert_eq!(section_title("argocd"), "Argo CD");
    }
}
