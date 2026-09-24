//! Schema validation of a parsed document, with byte ranges for inline diagnostics.
//!
//! Messages follow the API server's wording where it has one (`Unsupported value`,
//! `Required value`, `unknown field`), so what the editor shows before a dry run reads like
//! what the server would answer.

use std::collections::HashSet;
use std::ops::Range;

use serde_json::Value;

use crate::parse::{Node, NodeValue, Parsed, Path};
use crate::schema::Schema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Where a problem was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Syntax,
    Schema,
    /// Dry run or apply (validation, admission webhooks).
    Server,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Problem {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
    /// `spec.privateKey.rotationPolicy`, when known.
    pub path: Option<String>,
    pub source: Source,
}

impl Problem {
    pub fn error(range: Range<usize>, message: impl Into<String>) -> Self {
        Self {
            range,
            severity: Severity::Error,
            message: message.into(),
            path: None,
            source: Source::Schema,
        }
    }

    fn at(mut self, path: &Path) -> Self {
        if !path.0.is_empty() {
            self.path = Some(path.to_string());
        }
        self
    }

    fn warning(mut self) -> Self {
        self.severity = Severity::Warning;
        self
    }
}

/// The syntax error of a parse, if any.
pub fn syntax_problems(parsed: &Parsed) -> Vec<Problem> {
    parsed
        .error
        .iter()
        .map(|e| Problem {
            range: e.range.clone(),
            severity: Severity::Error,
            message: e.message.clone(),
            path: None,
            source: Source::Syntax,
        })
        .collect()
}

/// Validates one document against its kind's schema. `status` is read-only and skipped.
pub fn validate(root: &Node, schema: &Schema) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut v = Validator { out: &mut out };
    let anchor = root_anchor(root);
    v.walk(root, schema, &Path::default(), anchor);
    out.sort_by_key(|p| (p.range.start, p.severity));
    out
}

/// Where document-level problems go: its first key.
fn root_anchor(root: &Node) -> Range<usize> {
    root.as_map()
        .and_then(|entries| entries.first())
        .map_or(root.span.start..root.span.start, |e| e.key.span.clone())
}

/// Checks that every document says what it is.
pub fn validate_header(root: &Node) -> Vec<Problem> {
    let mut out = Vec::new();
    let anchor = root_anchor(root);
    if root.as_map().is_none() {
        out.push(Problem::error(
            root.span.clone(),
            "a Kubernetes object must be a mapping",
        ));
        return out;
    }
    for key in ["apiVersion", "kind"] {
        match root.get(key).and_then(Node::as_str) {
            Some(v) if !v.is_empty() => {}
            _ => out.push(Problem::error(
                anchor.clone(),
                format!("Required value: {key}"),
            )),
        }
    }
    out
}

struct Validator<'o> {
    out: &'o mut Vec<Problem>,
}

fn quote_list(values: &[Value]) -> String {
    values
        .iter()
        .map(|v| match v {
            Value::String(s) => format!("“{s}”"),
            other => format!("“{other}”"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn node_type(node: &Node) -> &'static str {
    match &node.value {
        NodeValue::Map(_) => "object",
        NodeValue::Seq(_) => "array",
        NodeValue::Scalar(s) => s.type_name(),
    }
}

fn type_matches(actual: &str, allowed: &[&str]) -> bool {
    allowed.is_empty()
        || allowed.contains(&actual)
        || (actual == "integer" && allowed.contains(&"number"))
}

impl Validator<'_> {
    fn push(&mut self, problem: Problem) {
        self.out.push(problem);
    }

    /// `anchor`: where to report problems of this node as a whole (its key).
    fn walk(&mut self, node: &Node, schema: &Schema, path: &Path, anchor: Range<usize>) {
        if path.0.len() == 1 && path.starts_with(&["status"]) {
            return;
        }
        if path.starts_with(&["metadata", "managedFields"]) {
            return;
        }
        let actual = node_type(node);
        if actual == "null" {
            // `key:` with no value, or `~`: the server treats it as unset.
            return;
        }
        let allowed = schema.allowed_types();
        if !type_matches(actual, &allowed) {
            let expected = allowed.join(" or ");
            let mut message = format!("expected {expected}, got {actual}");
            if allowed.contains(&"string") && node.as_scalar().is_some() {
                let text = node.as_str().unwrap_or_default();
                message.push_str(&format!(" (quote it: \"{text}\")"));
            }
            self.push(Problem::error(node.span.clone(), message).at(path));
            return;
        }
        match &node.value {
            NodeValue::Map(entries) => {
                let properties = schema.properties();
                let unknown_ok = schema.allows_unknown();
                let additional = schema.additional();
                let mut seen = HashSet::new();
                for entry in entries {
                    let key = entry.key_str();
                    let child_path = path.push_key(key);
                    if !seen.insert(key) {
                        self.push(
                            Problem::error(
                                entry.key.span.clone(),
                                format!("duplicate key “{key}”"),
                            )
                            .at(&child_path),
                        );
                        continue;
                    }
                    let child = properties
                        .iter()
                        .find(|(n, _)| *n == key)
                        .map(|(_, s)| s.clone())
                        .or_else(|| additional.clone());
                    match child {
                        Some(child) => {
                            self.walk(&entry.value, &child, &child_path, entry.key.span.clone())
                        }
                        None if unknown_ok => {}
                        None => self.push(
                            Problem::error(
                                entry.key.span.clone(),
                                format!("unknown field “{child_path}”"),
                            )
                            .at(&child_path),
                        ),
                    }
                }
                for required in schema.required() {
                    if !seen.contains(required) {
                        let full = path.push_key(required);
                        self.push(
                            Problem::error(anchor.clone(), format!("Required value: {full}"))
                                .at(&full),
                        );
                    }
                }
            }
            NodeValue::Seq(items) => {
                self.check_count(
                    node,
                    schema,
                    path,
                    items.len(),
                    "minItems",
                    "maxItems",
                    "items",
                );
                if let Some(item_schema) = schema.items() {
                    for (ix, item) in items.iter().enumerate() {
                        self.walk(item, &item_schema, &path.push_index(ix), item.span.clone());
                    }
                }
                self.check_list_uniqueness(items, schema, path);
            }
            NodeValue::Scalar(scalar) => {
                let value = scalar.to_json();
                if let Some(values) = schema.enum_values()
                    && !values.contains(&value)
                {
                    let shown = match &value {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    self.push(
                        Problem::error(
                            node.span.clone(),
                            format!(
                                "Unsupported value “{shown}”: supported values are {}",
                                quote_list(values)
                            ),
                        )
                        .at(path),
                    );
                    return;
                }
                if let Value::String(s) = &value {
                    if let Some(pattern) = schema.pattern()
                        && let Ok(re) = regex::Regex::new(pattern)
                        && !re.is_match(s)
                    {
                        self.push(
                            Problem::error(
                                node.span.clone(),
                                format!("Invalid value “{s}”: should match '{pattern}'"),
                            )
                            .at(path),
                        );
                    }
                    self.check_count(
                        node,
                        schema,
                        path,
                        s.chars().count(),
                        "minLength",
                        "maxLength",
                        "characters",
                    );
                }
                if let Some(n) = value.as_f64() {
                    if let Some(min) = schema.number("minimum")
                        && n < min
                    {
                        self.push(
                            Problem::error(
                                node.span.clone(),
                                format!(
                                    "Invalid value: {value}: must be greater than or equal to {min}"
                                ),
                            )
                            .at(path),
                        );
                    }
                    if let Some(max) = schema.number("maximum")
                        && n > max
                    {
                        self.push(
                            Problem::error(
                                node.span.clone(),
                                format!(
                                    "Invalid value: {value}: must be less than or equal to {max}"
                                ),
                            )
                            .at(path),
                        );
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_count(
        &mut self,
        node: &Node,
        schema: &Schema,
        path: &Path,
        count: usize,
        min_key: &str,
        max_key: &str,
        unit: &str,
    ) {
        let count_f = count as f64;
        if let Some(min) = schema.number(min_key)
            && count_f < min
        {
            self.push(
                Problem::error(
                    node.span.clone(),
                    format!("must have at least {min} {unit}"),
                )
                .at(path),
            );
        }
        if let Some(max) = schema.number(max_key)
            && count_f > max
        {
            self.push(
                Problem::error(node.span.clone(), format!("must have at most {max} {unit}"))
                    .at(path),
            );
        }
    }

    /// `x-kubernetes-list-type: set` (unique items) and `map` (unique `list-map-keys`).
    fn check_list_uniqueness(&mut self, items: &[Node], schema: &Schema, path: &Path) {
        match schema.list_type() {
            Some("set") => {
                let mut seen = HashSet::new();
                for (ix, item) in items.iter().enumerate() {
                    if !seen.insert(item.to_json().to_string()) {
                        self.push(
                            Problem::error(
                                item.span.clone(),
                                format!("Duplicate value: {}", item.to_json()),
                            )
                            .at(&path.push_index(ix)),
                        );
                    }
                }
            }
            Some("map") => {
                let keys = schema.list_map_keys();
                if keys.is_empty() {
                    return;
                }
                let mut seen = HashSet::new();
                for (ix, item) in items.iter().enumerate() {
                    let id: Vec<String> = keys
                        .iter()
                        .map(|k| {
                            item.get(k)
                                .map(|v| v.to_json().to_string())
                                .unwrap_or_default()
                        })
                        .collect();
                    if !seen.insert(id.clone()) {
                        let shown = keys
                            .iter()
                            .zip(&id)
                            .map(|(k, v)| format!("\"{k}\":{v}"))
                            .collect::<Vec<_>>()
                            .join(",");
                        self.push(
                            Problem::error(
                                item.span.clone(),
                                format!("Duplicate value: {{{shown}}}"),
                            )
                            .at(&path.push_index(ix))
                            .warning(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::schema::tests::cert_doc;
    use kubyl_core::Gvk;

    fn problems(text: &str) -> Vec<(String, String)> {
        let doc = cert_doc();
        let schema =
            Schema::for_gvk(&doc, &Gvk::new("cert-manager.io", "v1", "Certificate")).unwrap();
        let parsed = parse(text);
        let root = parsed.roots().next().unwrap();
        validate(root, &schema)
            .into_iter()
            .map(|p| (text[p.range].to_string(), p.message))
            .collect()
    }

    const CERT: &str = "apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: api-tls
  labels:
    app.kubernetes.io/managed-by: Helm
spec:
  secretName: api-tls
  renewBefore: 720h # 30d
  issuerRef:
    name: letsencrypt-prod
  privateKey:
    size: 256
    rotationPolicy: Allways
status:
  bogus: 1
";

    #[test]
    fn reports_the_enum_error_from_the_mockup() {
        assert_eq!(
            problems(CERT),
            [(
                "Allways".to_string(),
                "Unsupported value “Allways”: supported values are “Never”, “Always”".to_string()
            )]
        );
    }

    #[test]
    fn unknown_fields_types_and_required_values() {
        let text = "apiVersion: cert-manager.io/v1
kind: Certificate
spec:
  secretNam: x
  dnsNames: api.example.com
  privateKey:
    size: big
  keystores:
    anything: goes
";
        let found = problems(text);
        assert!(found.contains(&("secretNam".into(), "unknown field “spec.secretNam”".into())));
        assert!(found.contains(&(
            "api.example.com".into(),
            "expected array, got string".into()
        )));
        assert!(found.contains(&("big".into(), "expected integer, got string".into())));
        assert!(found.contains(&("spec".into(), "Required value: spec.issuerRef".into())));
        assert!(found.contains(&("spec".into(), "Required value: spec.secretName".into())));
        assert_eq!(found.len(), 5, "{found:?}");
    }

    #[test]
    fn quoting_hint_for_numbers_in_strings() {
        let text = "apiVersion: v1\nkind: Certificate\nmetadata:\n  labels:\n    port: 80\n";
        let found = problems(text);
        assert_eq!(
            found,
            [
                ("apiVersion".into(), "Required value: spec".into()),
                (
                    "80".into(),
                    "expected string, got integer (quote it: \"80\")".into()
                )
            ]
        );
    }

    #[test]
    fn header_checks() {
        let parsed = parse("metadata:\n  name: x\n");
        let found = validate_header(parsed.roots().next().unwrap());
        assert_eq!(found.len(), 2);
    }
}
