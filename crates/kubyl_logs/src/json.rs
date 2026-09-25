//! Pretty/inline JSON formatting for lines that are (or contain) a JSON object, and extracting
//! `key=value` filters from a clicked field.

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

/// Multi-line, indented rendering of a JSON value (`serde_json::to_string_pretty`).
pub fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

/// Flat `key: value, key: value` rendering used for the inline mode.
pub fn inline(value: &Value) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}={}", inline_scalar(v)))
            .collect::<Vec<_>>()
            .join(" "),
        other => inline_scalar(other),
    }
}

fn inline_scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

/// A `field == value` filter, built from a clicked top-level JSON field.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldFilter {
    pub field: String,
    pub value: String,
}

impl FieldFilter {
    /// Extracts the filter for `field` if `line` is JSON and has it at the top level.
    pub fn from_line(line: &str, field: &str) -> Option<Self> {
        let value = parse_object(line)?;
        let field_value = value.get(field)?;
        Some(FieldFilter {
            field: field.to_string(),
            value: inline_scalar(field_value),
        })
    }

    /// Whether a raw line matches this filter (JSON field equality, or substring fallback for
    /// non-JSON lines so the filter still narrows something sensible). A line that *is* valid
    /// JSON but simply lacks `field` does not match — it does not fall back to substring
    /// matching, since that would make unrelated JSON lines match by coincidence.
    pub fn matches(&self, line: &str) -> bool {
        match parse_object(line) {
            Some(value) => value
                .get(&self.field)
                .is_some_and(|v| inline_scalar(v) == self.value),
            None => line.contains(&self.value),
        }
    }
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
    fn inline_flattens_an_object() {
        let value: Value = serde_json::from_str(r#"{"level":"info","code":200}"#).unwrap();
        assert_eq!(inline(&value), "level=info code=200");
    }

    #[test]
    fn field_filter_extracts_and_matches() {
        let line = r#"{"level":"error","service":"payments"}"#;
        let filter = FieldFilter::from_line(line, "service").unwrap();
        assert_eq!(filter.value, "payments");
        assert!(filter.matches(line));
        assert!(!filter.matches(r#"{"level":"error","service":"auth"}"#));
    }

    #[test]
    fn field_filter_does_not_substring_fallback_for_valid_json_missing_the_field() {
        let filter = FieldFilter {
            field: "service".to_string(),
            value: "payments".to_string(),
        };
        // Valid JSON, but no `service` field: even though the text contains "payments" as a
        // substring, this must not match via the non-JSON fallback path.
        assert!(!filter.matches(r#"{"level":"error","msg":"payments queue backed up"}"#));
        // Genuinely non-JSON lines still use the substring fallback.
        assert!(filter.matches("payments queue backed up"));
    }
}
