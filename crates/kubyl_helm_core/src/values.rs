//! Values in the editors: parsing the buffer, the overrides an edited copy of the chart's
//! defaults amounts to, and the schema check (`values.schema.json` through phase 04's
//! validator). Values can hold passwords: nothing here logs them.

use kubyl_yaml_core::diff::LineDiff;
use kubyl_yaml_core::schema::Schema;
use kubyl_yaml_core::validate::{self, Problem};
use serde_json::{Map, Value};

/// Parses an editor's YAML into values (`{}` when empty). A syntax error says where.
pub fn parse(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let parsed = kubyl_yaml_core::parse::parse(text);
    if let Some(error) = &parsed.error {
        return Err(format!("The values aren't valid YAML: {}", error.message));
    }
    let mut roots = parsed.roots();
    let Some(root) = roots.next() else {
        return Ok(Value::Object(Map::new()));
    };
    if roots.next().is_some() {
        return Err("The values hold more than one YAML document.".into());
    }
    match root.to_json() {
        Value::Null => Ok(Value::Object(Map::new())),
        value @ Value::Object(_) => Ok(value),
        _ => Err("The values must be a map (key: value).".into()),
    }
}

/// What `edited` changes compared with `defaults`: only those keys, so the release's
/// user-supplied values stay small and later chart versions bring their own defaults. Maps
/// recurse, anything else (lists included) is replaced; a key removed from the defaults becomes
/// `null`, which Helm reads as "remove".
pub fn overrides(defaults: &Value, edited: &Value) -> Value {
    match (defaults, edited) {
        (Value::Object(defaults), Value::Object(edited)) => {
            let mut out = Map::new();
            for (key, value) in edited {
                match defaults.get(key) {
                    Some(default) if default == value => {}
                    Some(default @ Value::Object(_)) if value.is_object() => {
                        let nested = overrides(default, value);
                        if nested.as_object().is_some_and(|m| !m.is_empty()) {
                            out.insert(key.clone(), nested);
                        }
                    }
                    _ => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            for key in defaults.keys() {
                if !edited.contains_key(key) {
                    out.insert(key.clone(), Value::Null);
                }
            }
            Value::Object(out)
        }
        (_, edited) => edited.clone(),
    }
}

/// JSON Schema allows unknown keys unless `additionalProperties: false`; Kubernetes' OpenAPI
/// schemas (what phase 04's validator reads) reject them unless allowed. Marks every object
/// schema without `additionalProperties` as open.
pub fn open_schema(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            let is_object = map.get("type").and_then(Value::as_str) == Some("object")
                || map.contains_key("properties");
            if is_object && !map.contains_key("additionalProperties") {
                map.insert("additionalProperties".into(), Value::Bool(true));
            }
            for value in map.values_mut() {
                open_schema(value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(open_schema),
        _ => {}
    }
}

/// Problems of the editor's text against the chart's schema (already [`open_schema`]d), plus
/// YAML syntax errors. `partial`: the text only overrides the defaults, so required keys may be
/// missing from it.
pub fn problems(text: &str, schema: Option<&Value>, partial: bool) -> Vec<Problem> {
    let parsed = kubyl_yaml_core::parse::parse(text);
    let mut out = validate::syntax_problems(&parsed);
    if let Some(schema) = schema
        && let Some(root) = parsed.roots().next()
    {
        out.extend(
            validate::validate(root, &Schema::new(schema, schema))
                .into_iter()
                .filter(|p| !(partial && p.message.starts_with("Required value"))),
        );
    }
    out
}

/// A diff with every line's value masked ([`crate::preview::mask_value_line`]), for the values
/// diff before "Reveal".
pub fn masked(diff: &LineDiff) -> LineDiff {
    let mut out = diff.clone();
    for hunk in &mut out.hunks {
        for line in &mut hunk.lines {
            line.text = crate::preview::mask_value_line(&line.text);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edited_defaults_become_overrides() {
        let defaults = json!({"replicaCount": 1, "image": {"repository": "pause", "tag": "3.10"}, "greeting": "hello", "list": [1, 2]});
        let edited = parse(
            "replicaCount: 2\nimage:\n  repository: pause\n  tag: \"3.10\"\nlist: [1, 2, 3]\nextra: true\n",
        )
        .unwrap();
        assert_eq!(
            overrides(&defaults, &edited),
            json!({"replicaCount": 2, "list": [1, 2, 3], "extra": true, "greeting": null})
        );
        assert_eq!(overrides(&defaults, &defaults), json!({}));
        assert_eq!(parse("").unwrap(), json!({}));
        assert!(parse("- a\n").is_err());
        assert!(parse("a: [\n").is_err());
    }

    #[test]
    fn values_are_checked_against_the_chart_schema() {
        let mut schema = json!({
            "type": "object",
            "required": ["replicaCount"],
            "properties": {
                "replicaCount": {"type": "integer"},
                "storage": {"type": "string", "enum": ["1Mi", "2Mi"]},
                "image": {"type": "object", "properties": {"tag": {"type": "string"}}}
            }
        });
        open_schema(&mut schema);
        let problems = problems(
            "replicaCount: two\nstorage: 5Mi\nimage:\n  pullPolicy: Always\nother: 1\n",
            Some(&schema),
            false,
        );
        let messages: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.starts_with("expected integer")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("Unsupported value “5Mi”")),
            "{messages:?}"
        );
        // Unknown keys are fine in JSON Schema.
        assert!(
            !messages.iter().any(|m| m.contains("unknown field")),
            "{messages:?}"
        );
        // Override-only text doesn't need the required keys.
        assert!(problems_required("storage: 1Mi\n", &schema, false));
        assert!(!problems_required("storage: 1Mi\n", &schema, true));
    }

    fn problems_required(text: &str, schema: &Value, partial: bool) -> bool {
        problems(text, Some(schema), partial)
            .iter()
            .any(|p| p.message.starts_with("Required value"))
    }

    #[test]
    fn masked_diffs_hide_values() {
        let diff = crate::preview::values_diff(&json!({"a": "x"}), &json!({"a": "y"}));
        let masked = masked(&diff);
        let text: String = masked
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter().map(|l| l.text.clone()))
            .collect();
        assert!(!text.contains('x') || !text.contains(": x"));
        assert!(!text.contains(": y"));
    }
}
