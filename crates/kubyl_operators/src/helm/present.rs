//! Turning a decoded release into what the release tab shows: values as YAML lines with the
//! secrets masked, the manifest with Secret data masked, the objects in the manifest, and the
//! `helm` commands to copy.

use serde_json::Value;

/// Shown instead of a masked value.
pub const MASK: &str = "••••••••";

/// How a piece of a line is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Key,
    Punct,
    Str,
    Number,
    Bool,
    Null,
    Masked,
    Comment,
    Plain,
}

/// A styled piece of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
}

/// One line of highlighted YAML.
pub type Line = Vec<Span>;

fn span(text: impl Into<String>, tone: Tone) -> Span {
    Span {
        text: text.into(),
        tone,
    }
}

/// Values as YAML lines, keys sorted. With `mask`, strings and numbers show as [`MASK`]:
/// values commonly hold passwords and tokens; keys, booleans and `null` stay readable.
pub fn value_lines(value: &Value, mask: bool) -> Vec<Line> {
    let value = &kubyl_yaml::render::sorted(value.clone());
    let mut out = Vec::new();
    match value {
        Value::Object(map) if map.is_empty() => out.push(vec![span("{}", Tone::Punct)]),
        Value::Object(_) | Value::Array(_) => emit_block(value, 0, mask, &mut out),
        scalar => out.push(scalar_spans(scalar, 0, mask)),
    }
    out
}

fn emit_block(value: &Value, indent: usize, mask: bool, out: &mut Vec<Line>) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let mut line = vec![
                    span(pad.clone(), Tone::Plain),
                    span(key_text(key), Tone::Key),
                    span(":", Tone::Punct),
                ];
                if is_nested(item) {
                    out.push(line);
                    // Sequences under a key sit at the key's indent, like `helm get values`.
                    let child = if item.is_array() { indent } else { indent + 2 };
                    emit_block(item, child, mask, out);
                } else {
                    line.push(span(" ", Tone::Plain));
                    push_scalar(&mut line, item, indent, mask, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                let mut line = vec![span(pad.clone(), Tone::Plain), span("- ", Tone::Punct)];
                match item {
                    Value::Object(map) if !map.is_empty() => {
                        // The first key on the dash's line, the rest below it.
                        let mut nested = Vec::new();
                        emit_block(item, indent + 2, mask, &mut nested);
                        let mut first = nested.remove(0);
                        first.remove(0); // its indent
                        line.extend(first);
                        out.push(line);
                        out.extend(nested);
                    }
                    Value::Array(inner) if !inner.is_empty() => {
                        out.push(line);
                        emit_block(item, indent + 2, mask, out);
                    }
                    _ => push_scalar(&mut line, item, indent, mask, out),
                }
            }
        }
        scalar => out.push(scalar_spans(scalar, indent, mask)),
    }
}

fn is_nested(value: &Value) -> bool {
    match value {
        Value::Object(map) => !map.is_empty(),
        Value::Array(items) => !items.is_empty(),
        _ => false,
    }
}

fn key_text(key: &str) -> String {
    kubyl_yaml::render::yaml_scalar(key, 0)
}

/// Appends a scalar to `line` (multi-line strings continue on the next lines).
fn push_scalar(line: &mut Line, value: &Value, indent: usize, mask: bool, out: &mut Vec<Line>) {
    let spans = scalar_spans(value, indent, mask);
    match value {
        Value::String(s) if !mask && s.contains('\n') => {
            let text = kubyl_yaml::render::yaml_scalar(s, indent);
            let mut lines = text.split('\n');
            line.push(span(lines.next().unwrap_or_default(), Tone::Punct));
            out.push(std::mem::take(line));
            for rest in lines {
                out.push(vec![span(rest, Tone::Str)]);
            }
        }
        _ => {
            line.extend(spans);
            out.push(std::mem::take(line));
        }
    }
}

fn scalar_spans(value: &Value, indent: usize, mask: bool) -> Line {
    match value {
        Value::Null => vec![span("null", Tone::Null)],
        Value::Bool(b) => vec![span(b.to_string(), Tone::Bool)],
        Value::Object(_) => vec![span("{}", Tone::Punct)],
        Value::Array(_) => vec![span("[]", Tone::Punct)],
        _ if mask => vec![span(MASK, Tone::Masked)],
        Value::Number(n) => vec![span(n.to_string(), Tone::Number)],
        Value::String(s) => vec![span(kubyl_yaml::render::yaml_scalar(s, indent), Tone::Str)],
    }
}

/// Highlights YAML text line by line (the manifest): comments, `---`, keys and values.
pub fn text_lines(text: &str) -> Vec<Line> {
    text.lines().map(highlight_line).collect()
}

fn highlight_line(line: &str) -> Line {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    if trimmed.starts_with('#') || trimmed == "---" {
        return vec![span(line, Tone::Comment)];
    }
    let mut out = vec![span(indent, Tone::Plain)];
    let mut rest = trimmed;
    if let Some(after) = rest.strip_prefix("- ") {
        out.push(span("- ", Tone::Punct));
        rest = after;
    }
    // `key: value` (keys may be quoted); anything else is a value (block text, list items).
    let key_end = if rest.starts_with('"') || rest.starts_with('\'') {
        let quote = rest.chars().next().unwrap_or('"');
        rest[1..].find(quote).map(|i| i + 2)
    } else {
        rest.find(": ")
            .or_else(|| rest.strip_suffix(':').map(|k| k.len()))
    };
    match key_end {
        Some(end) if rest[end..].starts_with(':') => {
            out.push(span(&rest[..end], Tone::Key));
            out.push(span(":", Tone::Punct));
            let value = &rest[end + 1..];
            if !value.is_empty() {
                out.push(span(value, value_tone(value.trim())));
            }
        }
        _ => out.push(span(rest, value_tone(rest))),
    }
    out
}

fn value_tone(value: &str) -> Tone {
    if value == MASK || value.trim_matches('"') == MASK {
        Tone::Masked
    } else if matches!(value, "true" | "false") {
        Tone::Bool
    } else if matches!(value, "null" | "~") {
        Tone::Null
    } else if value.parse::<f64>().is_ok() {
        Tone::Number
    } else if value.starts_with('#') {
        Tone::Comment
    } else {
        Tone::Str
    }
}

/// The manifest with every value under `data` and `stringData` of `Secret` documents replaced
/// by [`MASK`]. Everything else (comments, order) stays as rendered. Documents are handled one
/// by one, so a syntax error in one can't leave a later Secret unmasked; a document that doesn't
/// parse and mentions a Secret is hidden.
pub fn mask_secrets(manifest: &str) -> String {
    let mut out = String::with_capacity(manifest.len());
    let mut doc = String::new();
    for line in manifest.split_inclusive('\n') {
        if line.trim_end() == "---" || line.starts_with("--- ") {
            out.push_str(&mask_document(&doc));
            doc.clear();
            out.push_str(line);
        } else {
            doc.push_str(line);
        }
    }
    out.push_str(&mask_document(&doc));
    out
}

fn mask_document(doc: &str) -> String {
    if !doc.contains("Secret") {
        return doc.to_string();
    }
    let parsed = kubyl_yaml::parse::parse(doc);
    if parsed.error.is_some() {
        return "# A Secret Kubyl couldn't parse: hidden.\n".to_string();
    }
    let mut edits = Vec::new();
    for root in parsed.roots() {
        if root.get("kind").and_then(|n| n.as_str()) != Some("Secret") {
            continue;
        }
        for field in ["data", "stringData"] {
            let Some(entries) = root.get(field).and_then(|n| n.as_map()) else {
                continue;
            };
            for entry in entries {
                edits.push(kubyl_yaml::render::Edit {
                    range: entry.value.span.clone(),
                    text: MASK.to_string(),
                });
            }
        }
    }
    kubyl_yaml::render::apply_edits(doc, edits)
}

/// An object the release manages (from its manifest).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ManifestObject {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    /// As written in the manifest; namespaced kinds without one live in the release's namespace.
    pub namespace: Option<String>,
    /// From a hook (`helm.sh/hook`), not the release's own objects.
    pub hook: bool,
}

/// The objects in a manifest, in order.
pub fn manifest_objects(manifest: &str) -> Vec<ManifestObject> {
    let parsed = kubyl_yaml::parse::parse(manifest);
    parsed
        .roots()
        .filter_map(|root| {
            let text = |path: &[&str]| {
                root.find(&kubyl_yaml::parse::Path::keys(path))
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            };
            Some(ManifestObject {
                api_version: text(&["apiVersion"])?,
                kind: text(&["kind"])?,
                name: text(&["metadata", "name"])?,
                namespace: text(&["metadata", "namespace"]),
                hook: text(&["metadata", "annotations", "helm.sh/hook"]).is_some(),
            })
        })
        .collect()
}

/// A `helm` command for the clipboard: what it does and the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub label: String,
    pub command: String,
}

fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=+,".contains(c))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// The commands the release tab offers: Kubyl shows releases read-only, so rollback,
/// uninstall and friends go through the user's `helm`. `context` is the kubeconfig context.
pub fn commands(
    name: &str,
    namespace: &str,
    revision: u32,
    latest: u32,
    context: Option<&str>,
) -> Vec<Command> {
    let tail = {
        let mut tail = format!(" -n {}", quote(namespace));
        if let Some(context) = context {
            tail.push_str(&format!(" --kube-context {}", quote(context)));
        }
        tail
    };
    let name = quote(name);
    let mut out = Vec::new();
    if revision < latest {
        out.push(Command {
            label: format!("Roll back to revision {revision}"),
            command: format!("helm rollback {name} {revision}{tail}"),
        });
    } else if latest > 1 {
        out.push(Command {
            label: format!("Roll back to revision {}", latest - 1),
            command: format!("helm rollback {name} {}{tail}", latest - 1),
        });
    }
    out.push(Command {
        label: "Uninstall".into(),
        command: format!("helm uninstall {name}{tail}"),
    });
    out.push(Command {
        label: "Get values".into(),
        command: format!("helm get values {name} --revision {revision}{tail}"),
    });
    out.push(Command {
        label: "History".into(),
        command: format!("helm history {name}{tail}"),
    });
    out.push(Command {
        label: "Status".into(),
        command: format!("helm status {name}{tail}"),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn values_render_as_yaml_and_mask_strings_and_numbers() {
        let values = json!({
            "grafana": {"adminPassword": "hunter2", "enabled": true, "replicas": 2},
            "receivers": [{"name": "pager", "routing_key": "abc"}],
            "tags": ["a", "b"],
            "empty": {},
            "note": "line one\nline two\n"
        });
        let shown = plain(&value_lines(&values, false));
        assert_eq!(
            shown,
            [
                "empty: {}",
                "grafana:",
                "  adminPassword: hunter2",
                "  enabled: true",
                "  replicas: 2",
                "note: |",
                "  line one",
                "  line two",
                "receivers:",
                "- name: pager",
                "  routing_key: abc",
                "tags:",
                "- a",
                "- b",
            ]
        );
        // Parses back to the same values.
        let text = shown.join("\n");
        let back: Value = serde_saphyr::from_str(&text).unwrap();
        assert_eq!(back, values);

        let masked = plain(&value_lines(&values, true));
        assert!(masked.contains(&format!("  adminPassword: {MASK}")));
        assert!(masked.contains(&format!("  replicas: {MASK}")));
        assert!(masked.contains(&"  enabled: true".to_string()));
        assert!(!masked.join("\n").contains("hunter2"));
        assert!(!masked.join("\n").contains("line one"));
    }

    #[test]
    fn manifests_mask_secret_data_and_list_objects() {
        let manifest = "---\n# Source: db/templates/secret.yaml\napiVersion: v1\nkind: Secret\nmetadata:\n  name: db\n  annotations:\n    note: keep\ndata:\n  password: aHVudGVyMg==\nstringData:\n  token: \"s3cret\"\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: db\n  namespace: shop\nspec:\n  replicas: 1\n---\napiVersion: batch/v1\nkind: Job\nmetadata:\n  name: migrate\n  annotations:\n    helm.sh/hook: post-upgrade\n";
        let masked = mask_secrets(manifest);
        assert!(!masked.contains("aHVudGVyMg=="), "{masked}");
        assert!(!masked.contains("s3cret"), "{masked}");
        assert!(masked.contains(&format!("password: {MASK}")));
        assert!(masked.contains("# Source: db/templates/secret.yaml"));
        assert!(masked.contains("note: keep"));
        let objects = manifest_objects(manifest);
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[1].kind, "Deployment");
        assert_eq!(objects[1].namespace.as_deref(), Some("shop"));
        assert!(objects[2].hook && !objects[0].hook);
        // A broken document before a Secret doesn't unmask it; a broken Secret is hidden.
        let broken = format!(
            "---\nkind: ConfigMap\ndata: [unclosed\n{manifest}---\nkind: Secret\ndata:\n  a: {{x: [\n"
        );
        let masked_broken = mask_secrets(&broken);
        assert!(!masked_broken.contains("aHVudGVyMg=="), "{masked_broken}");
        assert!(masked_broken.contains("couldn't parse"));
        let lines = text_lines(&masked);
        assert!(lines.iter().flatten().any(|s| s.tone == Tone::Masked));
        assert_eq!(lines[1][0].tone, Tone::Comment);
    }

    #[test]
    fn helm_commands_quote_their_arguments() {
        let commands = commands("shop db", "shop", 3, 5, Some("kind-kubyl-dev"));
        assert_eq!(
            commands[0].command,
            "helm rollback 'shop db' 3 -n shop --kube-context kind-kubyl-dev"
        );
        assert!(
            commands
                .iter()
                .any(|c| c.command.starts_with("helm uninstall 'shop db'"))
        );
        let latest = super::commands("db", "shop", 5, 5, None);
        assert_eq!(latest[0].command, "helm rollback db 4 -n shop");
        let first = super::commands("db", "shop", 1, 1, None);
        assert_eq!(first[0].label, "Uninstall");
    }
}
