//! Agent questions (ACP `elicitation/create`): a small form, or a URL to open.
//!
//! [`parse`] turns the request into a [`Request`] the panel renders; [`validate`] checks the
//! user's answers against the requested schema (required fields, lengths, ranges, patterns,
//! options) and builds the `content` of the `accept` answer. Kinds Kubyl doesn't know become
//! plain text fields, so an agent's question is never dropped.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// One field of a form.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: FieldKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Text {
        default: Option<String>,
        min_length: Option<u32>,
        max_length: Option<u32>,
        pattern: Option<String>,
        /// `email`, `uri`, `date`, `date-time`.
        format: Option<String>,
    },
    /// One of these `(value, title)` options.
    Choice {
        options: Vec<(String, String)>,
        default: Option<String>,
    },
    /// Any of these options.
    Multi {
        options: Vec<(String, String)>,
        min: Option<u64>,
        max: Option<u64>,
        default: Vec<String>,
    },
    Number {
        integer: bool,
        minimum: Option<f64>,
        maximum: Option<f64>,
        default: Option<f64>,
    },
    Bool {
        default: bool,
    },
}

/// What the agent asks.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Form {
        message: String,
        title: Option<String>,
        fields: Vec<Field>,
    },
    /// Open `url` in the browser (a sign-in, a consent page). `id` comes back in
    /// `elicitation/complete`.
    Url {
        message: String,
        url: String,
        id: String,
    },
}

/// The session a request belongs to, when it names one.
pub fn session_of(params: &Value) -> Option<String> {
    params["sessionId"].as_str().map(str::to_string)
}

fn options_of(schema: &Value) -> Option<Vec<(String, String)>> {
    if let Some(values) = schema["enum"].as_array() {
        let titles = schema["enumNames"].as_array();
        return Some(
            values
                .iter()
                .enumerate()
                .filter_map(|(i, v)| {
                    let value = v.as_str()?.to_string();
                    let title = titles
                        .and_then(|t| t.get(i))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| value.clone());
                    Some((value, title))
                })
                .collect(),
        );
    }
    let one_of = schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array())?;
    Some(
        one_of
            .iter()
            .filter_map(|o| {
                let value = o["const"].as_str()?.to_string();
                let title = o["title"].as_str().unwrap_or(&value).to_string();
                Some((value, title))
            })
            .collect(),
    )
}

fn field(key: &str, schema: &Value, required: bool) -> Field {
    let text = |key: &str| schema[key].as_str().map(str::to_string);
    let kind = match schema["type"].as_str() {
        Some("boolean") => FieldKind::Bool {
            default: schema["default"].as_bool().unwrap_or(false),
        },
        Some(kind @ ("number" | "integer")) => FieldKind::Number {
            integer: kind == "integer",
            minimum: schema["minimum"].as_f64(),
            maximum: schema["maximum"].as_f64(),
            default: schema["default"].as_f64(),
        },
        Some("array") => FieldKind::Multi {
            options: options_of(&schema["items"]).unwrap_or_default(),
            min: schema["minItems"].as_u64(),
            max: schema["maxItems"].as_u64(),
            default: schema["default"]
                .as_array()
                .map(|d| {
                    d.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        },
        _ => match options_of(schema) {
            Some(options) => FieldKind::Choice {
                options,
                default: text("default"),
            },
            None => FieldKind::Text {
                default: text("default"),
                min_length: schema["minLength"].as_u64().map(|n| n as u32),
                max_length: schema["maxLength"].as_u64().map(|n| n as u32),
                pattern: text("pattern"),
                format: text("format"),
            },
        },
    };
    Field {
        key: key.to_string(),
        label: text("title").unwrap_or_else(|| key.to_string()),
        description: text("description"),
        required,
        kind,
    }
}

/// The request in `params`, or why it can't be shown.
pub fn parse(params: &Value) -> Result<Request, String> {
    let message = params["message"].as_str().unwrap_or_default().to_string();
    match params["mode"].as_str().unwrap_or("form") {
        "url" => {
            let url = params["url"]
                .as_str()
                .ok_or("`url` is missing")?
                .to_string();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err(format!("Kubyl only opens http(s) links, not {url}"));
            }
            Ok(Request::Url {
                message,
                url,
                id: params["elicitationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        "form" => {
            let schema = &params["requestedSchema"];
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let fields = schema["properties"]
                .as_object()
                .map(|properties| {
                    properties
                        .iter()
                        .map(|(key, prop)| field(key, prop, required.contains(&key.as_str())))
                        .collect()
                })
                .unwrap_or_default();
            Ok(Request::Form {
                message,
                title: schema["title"].as_str().map(str::to_string),
                fields,
            })
        }
        other => Err(format!("Kubyl can't show `{other}` questions")),
    }
}

/// What the user entered for one field.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Text(String),
    Bool(bool),
    Selected(Vec<String>),
}

/// The `content` of an `accept` answer, or an error per field key.
pub fn validate(
    fields: &[Field],
    inputs: &BTreeMap<String, Input>,
) -> Result<Value, BTreeMap<String, String>> {
    let mut content = serde_json::Map::new();
    let mut errors = BTreeMap::new();
    for field in fields {
        let input = inputs.get(&field.key);
        let empty = match input {
            None => true,
            Some(Input::Text(t)) => t.trim().is_empty(),
            Some(Input::Selected(s)) => s.is_empty(),
            Some(Input::Bool(_)) => false,
        };
        if empty && !matches!(field.kind, FieldKind::Bool { .. }) {
            if field.required {
                errors.insert(field.key.clone(), "Required.".into());
            }
            continue;
        }
        let value = match (&field.kind, input) {
            (FieldKind::Bool { default }, input) => Ok(json!(
                matches!(input, Some(Input::Bool(true))) || (input.is_none() && *default)
            )),
            (
                FieldKind::Text {
                    min_length,
                    max_length,
                    pattern,
                    format,
                    ..
                },
                Some(Input::Text(text)),
            ) => check_text(
                text.trim(),
                *min_length,
                *max_length,
                pattern.as_deref(),
                format.as_deref(),
            )
            .map(|t| json!(t)),
            (
                FieldKind::Number {
                    integer,
                    minimum,
                    maximum,
                    ..
                },
                Some(Input::Text(text)),
            ) => check_number(text.trim(), *integer, *minimum, *maximum),
            (FieldKind::Choice { options, .. }, Some(Input::Selected(selected))) => {
                match selected.first() {
                    Some(value) if options.iter().any(|(v, _)| v == value) => Ok(json!(value)),
                    _ => Err("Pick one of the options.".to_string()),
                }
            }
            (
                FieldKind::Multi {
                    options, min, max, ..
                },
                Some(Input::Selected(selected)),
            ) => {
                let n = selected.len() as u64;
                if selected
                    .iter()
                    .any(|s| !options.iter().any(|(v, _)| v == s))
                {
                    Err("Pick from the options.".to_string())
                } else if min.is_some_and(|m| n < m) {
                    Err(format!("Pick at least {}.", min.unwrap_or_default()))
                } else if max.is_some_and(|m| n > m) {
                    Err(format!("Pick at most {}.", max.unwrap_or_default()))
                } else {
                    Ok(json!(selected))
                }
            }
            _ => Err("Unexpected value.".to_string()),
        };
        match value {
            Ok(value) => {
                content.insert(field.key.clone(), value);
            }
            Err(message) => {
                errors.insert(field.key.clone(), message);
            }
        }
    }
    if errors.is_empty() {
        Ok(Value::Object(content))
    } else {
        Err(errors)
    }
}

fn check_text(
    text: &str,
    min: Option<u32>,
    max: Option<u32>,
    pattern: Option<&str>,
    format: Option<&str>,
) -> Result<String, String> {
    let len = text.chars().count() as u32;
    if min.is_some_and(|m| len < m) {
        return Err(format!("At least {} characters.", min.unwrap_or_default()));
    }
    if max.is_some_and(|m| len > m) {
        return Err(format!("At most {} characters.", max.unwrap_or_default()));
    }
    if let Some(pattern) = pattern
        && let Ok(re) = regex::Regex::new(pattern)
        && !re.is_match(text)
    {
        return Err(format!("Doesn't match {pattern}."));
    }
    let ok = match format {
        Some("email") => text.contains('@') && !text.contains(char::is_whitespace),
        Some("uri") => text.contains("://"),
        Some("date") => text.parse::<jiff::civil::Date>().is_ok(),
        Some("date-time") => text.parse::<jiff::Timestamp>().is_ok(),
        _ => true,
    };
    if !ok {
        return Err(format!("Not a valid {}.", format.unwrap_or_default()));
    }
    Ok(text.to_string())
}

fn check_number(
    text: &str,
    integer: bool,
    min: Option<f64>,
    max: Option<f64>,
) -> Result<Value, String> {
    let value: f64 = text.parse().map_err(|_| "Not a number.".to_string())?;
    if integer && value.fract() != 0.0 {
        return Err("A whole number.".into());
    }
    if min.is_some_and(|m| value < m) {
        return Err(format!("At least {}.", min.unwrap_or_default()));
    }
    if max.is_some_and(|m| value > m) {
        return Err(format!("At most {}.", max.unwrap_or_default()));
    }
    Ok(if integer {
        json!(value as i64)
    } else {
        json!(value)
    })
}

/// The answers for the agent: `accept` with content, `decline` or `cancel`.
pub fn accept(content: Value) -> Value {
    json!({ "action": "accept", "content": content })
}

pub fn decline() -> Value {
    json!({ "action": "decline" })
}

pub fn cancel() -> Value {
    json!({ "action": "cancel" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Vec<Field> {
        let params = json!({
            "sessionId": "s1", "mode": "form", "message": "Which namespace?",
            "requestedSchema": {
                "type": "object",
                "properties": {
                    "namespace": {"type": "string", "title": "Namespace", "minLength": 1, "pattern": "^[a-z0-9-]+$"},
                    "replicas": {"type": "integer", "minimum": 1, "maximum": 10},
                    "dry_run": {"type": "boolean", "default": true},
                    "env": {"type": "string", "oneOf": [{"const": "dev", "title": "Development"}, {"const": "prod", "title": "Production"}]},
                    "tags": {"type": "array", "items": {"type": "string", "enum": ["a", "b", "c"]}, "maxItems": 2},
                    "weird": {"type": "unknown-kind"}
                },
                "required": ["namespace", "env"]
            }
        });
        assert_eq!(session_of(&params).as_deref(), Some("s1"));
        match parse(&params).unwrap() {
            Request::Form {
                message, fields, ..
            } => {
                assert_eq!(message, "Which namespace?");
                fields
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn forms_parse_every_kind() {
        let fields = form();
        let kinds: BTreeMap<&str, &FieldKind> =
            fields.iter().map(|f| (f.key.as_str(), &f.kind)).collect();
        assert!(matches!(kinds["namespace"], FieldKind::Text { .. }));
        assert!(matches!(
            kinds["replicas"],
            FieldKind::Number { integer: true, .. }
        ));
        assert!(matches!(
            kinds["dry_run"],
            FieldKind::Bool { default: true }
        ));
        assert!(
            matches!(kinds["env"], FieldKind::Choice { options, .. } if options[1].1 == "Production")
        );
        assert!(matches!(kinds["tags"], FieldKind::Multi { options, .. } if options.len() == 3));
        assert!(matches!(kinds["weird"], FieldKind::Text { .. }));
        assert!(fields.iter().find(|f| f.key == "env").unwrap().required);
    }

    #[test]
    fn answers_are_checked_against_the_schema() {
        let fields = form();
        let mut inputs = BTreeMap::new();
        inputs.insert("namespace".to_string(), Input::Text("Shop!".into()));
        inputs.insert("replicas".to_string(), Input::Text("2.5".into()));
        inputs.insert(
            "tags".to_string(),
            Input::Selected(vec!["a".into(), "b".into(), "c".into()]),
        );
        let errors = validate(&fields, &inputs).unwrap_err();
        assert!(errors.contains_key("namespace"));
        assert!(errors.contains_key("replicas"));
        assert!(errors.contains_key("tags"));
        assert_eq!(errors["env"], "Required.");

        inputs.insert("namespace".to_string(), Input::Text("shop".into()));
        inputs.insert("replicas".to_string(), Input::Text("3".into()));
        inputs.insert("env".to_string(), Input::Selected(vec!["prod".into()]));
        inputs.insert("tags".to_string(), Input::Selected(vec!["a".into()]));
        let content = validate(&fields, &inputs).unwrap();
        assert_eq!(
            content,
            json!({"namespace": "shop", "replicas": 3, "dry_run": true, "env": "prod", "tags": ["a"]})
        );
    }

    #[test]
    fn urls_must_be_web_links() {
        let ok = parse(
            &json!({"mode": "url", "url": "https://example.com/login", "elicitationId": "e1", "message": "Sign in"}),
        );
        assert!(matches!(ok, Ok(Request::Url { id, .. }) if id == "e1"));
        assert!(parse(&json!({"mode": "url", "url": "file:///etc/passwd"})).is_err());
        assert!(parse(&json!({"mode": "telepathy"})).is_err());
    }
}
