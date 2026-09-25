//! JSON log lines: inline (`key=value`) and pretty (indented, multi-line) renderings with token
//! spans for key highlighting, the clickable fields of each rendering, and field filters.

use std::ops::Range;

use serde_json::Value;

/// If `line` is a JSON object (after trimming leading whitespace), returns it parsed.
pub fn parse_object(line: &str) -> Option<Value> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    value.is_object().then_some(value)
}

/// What a span of rendered JSON is, for coloring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Key,
    String,
    Number,
    Bool,
    Null,
    Punctuation,
}

/// A rendered JSON value: the text, token spans (for colors) and the clickable fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rendered {
    pub text: String,
    pub tokens: Vec<(Range<usize>, Token)>,
    /// Clickable fields: the byte range of the key in `text`, and the filter it adds.
    pub fields: Vec<(Range<usize>, FieldFilter)>,
}

impl Rendered {
    fn push(&mut self, text: &str, token: Option<Token>) -> Range<usize> {
        let start = self.text.len();
        self.text.push_str(text);
        let range = start..self.text.len();
        if let Some(token) = token {
            self.tokens.push((range.clone(), token));
        }
        range
    }
}

fn scalar_token(value: &Value) -> Token {
    match value {
        Value::String(_) => Token::String,
        Value::Number(_) => Token::Number,
        Value::Bool(_) => Token::Bool,
        Value::Null => Token::Null,
        _ => Token::Punctuation,
    }
}

/// The text of a scalar as a filter compares it (strings without quotes).
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

/// Flat `key=value` rendering; nested objects are flattened to dotted paths
/// (`http.status=200`), arrays stay JSON.
pub fn inline(value: &Value) -> Rendered {
    let mut out = Rendered::default();
    let Value::Object(map) = value else {
        out.push(&scalar_text(value), Some(scalar_token(value)));
        return out;
    };
    let mut first = true;
    // Depth-first so nested fields stay next to their parent, in document order.
    fn walk(
        out: &mut Rendered,
        first: &mut bool,
        prefix: &str,
        map: &serde_json::Map<String, Value>,
    ) {
        for (key, value) in map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if let Value::Object(inner) = value
                && !inner.is_empty()
            {
                walk(out, first, &path, inner);
                continue;
            }
            if !*first {
                out.push(" ", None);
            }
            *first = false;
            let key_range = out.push(&path, Some(Token::Key));
            out.push("=", Some(Token::Punctuation));
            let text = match value {
                Value::Array(_) | Value::Object(_) => value.to_string(),
                other => scalar_text(other),
            };
            out.push(&text, Some(scalar_token(value)));
            if !matches!(value, Value::Array(_) | Value::Object(_)) {
                out.fields.push((
                    key_range,
                    FieldFilter {
                        field: path,
                        value: scalar_text(value),
                    },
                ));
            }
        }
    }
    walk(&mut out, &mut first, "", map);
    out
}

/// Indented multi-line rendering (two spaces, like `jq`).
pub fn pretty(value: &Value) -> Rendered {
    let mut out = Rendered::default();
    fn write(out: &mut Rendered, value: &Value, indent: usize, path: &str) {
        match value {
            Value::Object(map) if !map.is_empty() => {
                out.push("{", Some(Token::Punctuation));
                let len = map.len();
                for (i, (key, child)) in map.iter().enumerate() {
                    out.push("\n", None);
                    out.push(&"  ".repeat(indent + 1), None);
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    let key_range = out.push(&format!("\"{key}\""), Some(Token::Key));
                    out.push(": ", Some(Token::Punctuation));
                    if !matches!(child, Value::Object(_) | Value::Array(_)) {
                        out.fields.push((
                            key_range,
                            FieldFilter {
                                field: child_path.clone(),
                                value: scalar_text(child),
                            },
                        ));
                    }
                    write(out, child, indent + 1, &child_path);
                    if i + 1 < len {
                        out.push(",", Some(Token::Punctuation));
                    }
                }
                out.push("\n", None);
                out.push(&"  ".repeat(indent), None);
                out.push("}", Some(Token::Punctuation));
            }
            Value::Array(items) if !items.is_empty() => {
                out.push("[", Some(Token::Punctuation));
                let len = items.len();
                for (i, item) in items.iter().enumerate() {
                    out.push("\n", None);
                    out.push(&"  ".repeat(indent + 1), None);
                    write(out, item, indent + 1, path);
                    if i + 1 < len {
                        out.push(",", Some(Token::Punctuation));
                    }
                }
                out.push("\n", None);
                out.push(&"  ".repeat(indent), None);
                out.push("]", Some(Token::Punctuation));
            }
            other => {
                out.push(&other.to_string(), Some(scalar_token(other)));
            }
        }
    }
    write(&mut out, value, 0, "");
    out
}

/// A `field == value` filter, built from a clicked JSON field. `field` is a dotted path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FieldFilter {
    pub field: String,
    pub value: String,
}

impl FieldFilter {
    /// Extracts the filter for `field` if `line` is JSON and has it.
    pub fn from_line(line: &str, field: &str) -> Option<Self> {
        let value = parse_object(line)?;
        let field_value = lookup(&value, field)?;
        Some(FieldFilter {
            field: field.to_string(),
            value: scalar_text(field_value),
        })
    }

    /// Whether a raw line matches this filter (JSON field equality, or substring fallback for
    /// non-JSON lines so the filter still narrows something sensible). A line that *is* valid
    /// JSON but lacks `field` does not match.
    pub fn matches(&self, line: &str) -> bool {
        match parse_object(line) {
            Some(value) => {
                lookup(&value, &self.field).is_some_and(|v| scalar_text(v) == self.value)
            }
            None => line.contains(&self.value),
        }
    }

    /// `field=value`, for the filter chip.
    pub fn label(&self) -> String {
        format!("{}={}", self.field, self.value)
    }
}

/// Looks up a dotted path (`http.status`), trying the whole key first for keys that contain
/// dots themselves (`k8s.pod`).
fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let map = value.as_object()?;
    if let Some(found) = map.get(path) {
        return Some(found);
    }
    let (head, rest) = path.split_once('.')?;
    lookup(map.get(head)?, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_json_objects_only() {
        assert!(parse_object(r#"{"a":1}"#).is_some());
        assert!(parse_object("[1,2,3]").is_none());
        assert!(parse_object("not json").is_none());
    }

    #[test]
    fn inline_flattens_an_object_with_spans_and_fields() {
        let value: Value =
            serde_json::from_str(r#"{"level":"info","http":{"status":200},"tags":["a"]}"#).unwrap();
        let rendered = inline(&value);
        assert_eq!(rendered.text, r#"level=info http.status=200 tags=["a"]"#);
        assert_eq!(&rendered.text[rendered.tokens[0].0.clone()], "level");
        assert_eq!(rendered.tokens[0].1, Token::Key);
        let fields: Vec<String> = rendered.fields.iter().map(|(_, f)| f.label()).collect();
        assert_eq!(fields, ["level=info", "http.status=200"]);
        let (range, _) = &rendered.fields[1];
        assert_eq!(&rendered.text[range.clone()], "http.status");
    }

    #[test]
    fn pretty_indents_and_marks_keys() {
        let value: Value = serde_json::from_str(r#"{"a":1,"b":{"c":"x"}}"#).unwrap();
        let rendered = pretty(&value);
        assert_eq!(
            rendered.text,
            "{\n  \"a\": 1,\n  \"b\": {\n    \"c\": \"x\"\n  }\n}"
        );
        let fields: Vec<String> = rendered.fields.iter().map(|(_, f)| f.label()).collect();
        assert_eq!(fields, ["a=1", "b.c=x"]);
    }

    #[test]
    fn field_filter_extracts_and_matches_nested_paths() {
        let line = r#"{"level":"error","svc":{"name":"payments"}}"#;
        let filter = FieldFilter::from_line(line, "svc.name").unwrap();
        assert_eq!(filter.value, "payments");
        assert!(filter.matches(line));
        assert!(!filter.matches(r#"{"level":"error","svc":{"name":"auth"}}"#));
    }

    #[test]
    fn field_filter_does_not_substring_fallback_for_valid_json_missing_the_field() {
        let filter = FieldFilter {
            field: "service".to_string(),
            value: "payments".to_string(),
        };
        assert!(!filter.matches(r#"{"level":"error","msg":"payments queue backed up"}"#));
        assert!(filter.matches("payments queue backed up"));
    }
}
