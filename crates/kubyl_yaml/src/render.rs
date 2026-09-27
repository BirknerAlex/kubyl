//! Turns a live object into editor text, and editor documents back into objects to send.
//!
//! - `metadata.managedFields` is hidden by default (a code lens names the managers).
//! - Secrets: `data` values are shown masked (`••••••••`) or decoded, never base64. The
//!   original values stay in memory ([`SecretValues`]) so masked values are sent back unchanged
//!   and edited or decoded ones are re-encoded. Nothing decoded is written anywhere.
//! - OpenShift Routes: an inline private key (`spec.tls.key`) is masked the same way, revealed
//!   by the same toggle and sent back unchanged while it's still masked.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kubyl_resources::route;
use serde_json::{Map, Value};

use crate::parse::{self, Node, NodeValue, Path};

/// Shown instead of a Secret value until the user reveals it.
pub const MASK: &str = "••••••••";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderOptions {
    pub hide_managed_fields: bool,
    pub reveal_secrets: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            hide_managed_fields: true,
            reveal_secrets: false,
        }
    }
}

/// A Secret's original `data` (or a Route's inline key), kept in memory for re-encoding.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretValues {
    /// Key → base64 as stored on the server.
    pub original: BTreeMap<String, String>,
    /// Keys whose value isn't UTF-8 text; they are shown (and sent) as base64.
    pub binary: BTreeSet<String>,
    /// kubectl's last-applied annotation, which holds the whole Secret (or the Route with its
    /// key); always masked.
    pub last_applied: Option<String>,
    /// An OpenShift Route's `spec.tls.key` as stored on the server.
    pub route_key: Option<String>,
}

/// Never prints the values (they end up in logs and panics otherwise).
impl std::fmt::Debug for SecretValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretValues")
            .field("keys", &self.original.keys().collect::<Vec<_>>())
            .field("binary", &self.binary)
            .field("last_applied", &self.last_applied.as_ref().map(|_| MASK))
            .field("route_key", &self.route_key.as_ref().map(|_| MASK))
            .finish()
    }
}

/// kubectl's copy of the last applied object (for Secrets: including `data`).
pub const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

impl SecretValues {
    fn decoded(&self, key: &str) -> Option<String> {
        let raw = self.original.get(key)?;
        STANDARD
            .decode(raw)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    }
}

/// Whether `object` is a core `Secret`.
pub fn is_secret(object: &Value) -> bool {
    object["kind"].as_str() == Some("Secret") && object["apiVersion"].as_str() == Some("v1")
}

/// The field managers of an object, in order (`helm`, `kubyl`…).
pub fn managers(object: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in object
        .pointer("/metadata/managedFields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(manager) = entry["manager"].as_str()
            && !out.iter().any(|m| m == manager)
        {
            out.push(manager.to_string());
        }
    }
    out
}

/// The text of `object` for the editor, and the Secret values it hides.
pub fn render(object: &Value, options: RenderOptions) -> (String, Option<SecretValues>) {
    let mut object = object.clone();
    if options.hide_managed_fields
        && let Some(meta) = object.get_mut("metadata").and_then(Value::as_object_mut)
    {
        meta.remove("managedFields");
    }
    let mut secret = None;
    if is_secret(&object)
        && let Some(data) = object.get_mut("data").and_then(Value::as_object_mut)
    {
        let mut values = SecretValues::default();
        for (key, value) in data.iter_mut() {
            let raw = value.as_str().unwrap_or_default().to_string();
            values.original.insert(key.clone(), raw.clone());
            let decoded = STANDARD
                .decode(&raw)
                .ok()
                .and_then(|b| String::from_utf8(b).ok());
            match decoded {
                None => {
                    values.binary.insert(key.clone());
                }
                Some(_) if !options.reveal_secrets => *value = Value::String(MASK.into()),
                Some(text) => *value = Value::String(text),
            }
        }
        if let Some(annotation) = object
            .pointer_mut("/metadata/annotations")
            .and_then(Value::as_object_mut)
            .and_then(|a| a.get_mut(LAST_APPLIED))
        {
            values.last_applied = annotation.as_str().map(String::from);
            *annotation = Value::String(MASK.into());
        }
        secret = Some(values);
    }
    // The key in `spec`, or only in kubectl's last-applied copy (taken out of `spec` since).
    if route::has_key_material(&object) {
        let mut values = SecretValues {
            route_key: route::inline_key(&object).map(String::from),
            ..Default::default()
        };
        values.last_applied = object
            .pointer(&format!(
                "/metadata/annotations/{}",
                LAST_APPLIED.replace('/', "~1")
            ))
            .and_then(Value::as_str)
            .map(String::from);
        // Masks the key and the last-applied copy; a revealed key is put back in plain text.
        route::mask_inline_key(&mut object);
        if options.reveal_secrets
            && let (Some(slot), Some(key)) =
                (object.pointer_mut("/spec/tls/key"), &values.route_key)
        {
            *slot = Value::String(key.clone());
        }
        secret = Some(values);
    }
    (to_yaml(&sorted(object)), secret)
}

/// The Route documents in `text`.
fn route_roots(parsed: &parse::Parsed) -> impl Iterator<Item = &Node> {
    parsed.roots().filter(|root| {
        root.get("kind").and_then(Node::as_str) == Some(route::KIND)
            && root
                .get("apiVersion")
                .and_then(Node::as_str)
                .is_some_and(|api| api.starts_with("route.openshift.io/"))
    })
}

/// The `spec.tls.key` entries of the Route documents in `text`.
fn route_key_entries(parsed: &parse::Parsed) -> Vec<&parse::Entry> {
    route_roots(parsed)
        .filter_map(|root| root.find_entry(&Path::keys(&["spec", "tls", "key"])))
        .collect()
}

/// Edits that mask every inline Route key in `text` (rendered manifests: Helm, Argo CD), and
/// kubectl's last-applied copy of those Routes. Values already masked stay as they are.
pub fn mask_route_key_edits(text: &str) -> Vec<Edit> {
    let parsed = parse::parse(text);
    let unmasked = |entry: &&parse::Entry| {
        entry
            .value
            .as_str()
            .is_some_and(|v| v != MASK && !v.is_empty())
    };
    route_roots(&parsed)
        .filter(|root| {
            root.find_entry(&Path::keys(&["spec", "tls", "key"]))
                .as_ref()
                .is_some_and(unmasked)
                // A key taken out of `spec` may still be in the last-applied copy.
                || root
                    .find_entry(&Path::keys(&["metadata", "annotations", LAST_APPLIED]))
                    .and_then(|entry| entry.value.as_str())
                    .is_some_and(route::applied_text_has_key)
        })
        .flat_map(|root| {
            [
                root.find_entry(&Path::keys(&["spec", "tls", "key"])),
                root.find_entry(&Path::keys(&["metadata", "annotations", LAST_APPLIED])),
            ]
        })
        .flatten()
        .filter(unmasked)
        .map(|entry| Edit {
            range: value_range(text, &entry.value),
            text: MASK.to_string(),
        })
        .collect()
}

/// Keys in alphabetical order at every level, like `kubectl get -o yaml` (stable diffs no
/// matter how the server or kube serialized the object).
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// YAML for a JSON value (the same formatting as the explorer's Copy YAML).
pub fn to_yaml(value: &Value) -> String {
    kubyl_resources::format::to_yaml(value)
}

/// Replaces the Secret values in `object` (from the buffer) with what the server must get:
/// masked values → the original base64, binary values as they are, everything else encoded.
/// A Route's masked inline key → the original key.
pub fn restore_secret(object: &mut Value, values: Option<&SecretValues>) {
    let route = route::is_route_object(object) && object.get("kind").is_some();
    if !is_secret(object) && !route {
        return;
    }
    let empty = SecretValues::default();
    let values = values.unwrap_or(&empty);
    if route
        && let (Some(original), Some(key)) =
            (&values.route_key, object.pointer_mut("/spec/tls/key"))
        && key.as_str() == Some(MASK)
    {
        *key = Value::String(original.clone());
    }
    if let (Some(original), Some(annotation)) = (
        &values.last_applied,
        object
            .pointer_mut("/metadata/annotations")
            .and_then(Value::as_object_mut)
            .and_then(|a| a.get_mut(LAST_APPLIED)),
    ) && annotation.as_str() == Some(MASK)
    {
        *annotation = Value::String(original.clone());
    }
    if route {
        return;
    }
    let Some(data) = object.get_mut("data").and_then(Value::as_object_mut) else {
        return;
    };
    let mut encoded = Map::new();
    for (key, value) in std::mem::take(data) {
        let text = match &value {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let out = if text == MASK && values.original.contains_key(&key) {
            values.original[&key].clone()
        } else if values.binary.contains(&key) {
            text
        } else {
            STANDARD.encode(text.as_bytes())
        };
        encoded.insert(key, Value::String(out));
    }
    *data = encoded;
}

/// A text edit: replace `range` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// Applies non-overlapping edits (any order) to `text`.
pub fn apply_edits(text: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    let mut out = text.to_string();
    for edit in edits {
        if edit.range.end <= out.len() && out.is_char_boundary(edit.range.start) {
            out.replace_range(edit.range, &edit.text);
        }
    }
    out
}

/// A YAML scalar for `value` as the value of a key at column `key_col`: a block literal for
/// multi-line text, a quoted string when plain would change its meaning.
pub fn yaml_scalar(value: &str, key_col: usize) -> String {
    if value.contains('\n') {
        let indent = " ".repeat(key_col + 2);
        let chomp = if value.ends_with('\n') { "" } else { "-" };
        let body: Vec<String> = value
            .trim_end_matches('\n')
            .split('\n')
            .map(|l| {
                if l.is_empty() {
                    String::new()
                } else {
                    format!("{indent}{l}")
                }
            })
            .collect();
        return format!("|{chomp}\n{}", body.join("\n"));
    }
    let plain_ok = !value.is_empty()
        && parse::resolve_plain(value) == Value::String(value.to_string())
        && !value.starts_with([
            ' ', '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"',
            '%', '@', '`',
        ])
        && !value.ends_with(' ')
        && !value.contains(": ")
        && !value.contains(" #")
        && !matches!(
            value,
            "y" | "Y"
                | "n"
                | "N"
                | "yes"
                | "Yes"
                | "YES"
                | "no"
                | "No"
                | "NO"
                | "on"
                | "On"
                | "ON"
                | "off"
                | "Off"
                | "OFF"
        );
    if plain_ok {
        value.to_string()
    } else {
        serde_json::to_string(value).unwrap_or_default()
    }
}

/// The `data` entries of the Secret documents in `text`.
fn secret_data_entries(parsed: &parse::Parsed) -> Vec<&parse::Entry> {
    parsed
        .roots()
        .filter(|root| {
            root.get("kind").and_then(Node::as_str) == Some("Secret")
                && root.get("apiVersion").and_then(Node::as_str) == Some("v1")
        })
        .filter_map(|root| root.get("data")?.as_map())
        .flatten()
        .collect()
}

fn column(text: &str, offset: usize) -> usize {
    offset - parse::line_range(text, offset).start
}

/// Edits that reveal (decode) or mask the Secret values (and a Route's inline key) in `text`.
/// Values the user changed stay as they are.
pub fn toggle_secret_edits(text: &str, values: &SecretValues, reveal: bool) -> Vec<Edit> {
    let parsed = parse::parse(text);
    let mut edits = Vec::new();
    for entry in secret_data_entries(&parsed) {
        let key = entry.key_str();
        if values.binary.contains(key) {
            continue;
        }
        let Some(current) = entry.value.as_str() else {
            continue;
        };
        let Some(decoded) = values.decoded(key) else {
            continue;
        };
        let replacement = if reveal && current == MASK {
            yaml_scalar(&decoded, column(text, entry.key.span.start))
        } else if !reveal && current == decoded {
            MASK.to_string()
        } else {
            continue;
        };
        edits.push(Edit {
            range: value_range(text, &entry.value),
            text: replacement,
        });
    }
    if let Some(original) = &values.route_key {
        for entry in route_key_entries(&parsed) {
            let Some(current) = entry.value.as_str() else {
                continue;
            };
            let replacement = if reveal && current == MASK {
                yaml_scalar(original, column(text, entry.key.span.start))
            } else if !reveal && current == original {
                MASK.to_string()
            } else {
                continue;
            };
            edits.push(Edit {
                range: value_range(text, &entry.value),
                text: replacement,
            });
        }
    }
    edits
}

/// The bytes of a value node including a block scalar's indicator line.
fn value_range(text: &str, node: &Node) -> Range<usize> {
    let mut start = node.span.start;
    let end = node.span.end;
    if let NodeValue::Scalar(s) = &node.value
        && s.style == parse::Style::Block
    {
        // The span starts at the content; include `|`/`>` on the key's line.
        let before = &text[..start];
        if let Some(bar) = before.rfind(['|', '>']) {
            start = bar;
        }
        // The span runs up to the next token (past the line break and the next indentation).
        let trimmed_end = text[..end].trim_end_matches(['\n', '\r', ' ']).len();
        return start..trimmed_end;
    }
    if let NodeValue::Scalar(s) = &node.value
        && s.style == parse::Style::Quoted
    {
        // Spans of quoted scalars exclude the quotes on some inputs; widen to them.
        if start > 0
            && matches!(text.as_bytes()[start - 1], b'"' | b'\'')
            && !text[start..].starts_with(['"', '\''])
        {
            start -= 1;
            return start..(end + 1).min(text.len());
        }
    }
    start..end
}

/// Edits that show or hide `metadata.managedFields` in `text` (the first document).
/// `managed` is the live object's list.
pub fn toggle_managed_fields_edits(text: &str, managed: Option<&Value>, show: bool) -> Vec<Edit> {
    let parsed = parse::parse(text);
    let Some(root) = parsed.roots().next() else {
        return Vec::new();
    };
    let Some(meta) = root.get("metadata") else {
        return Vec::new();
    };
    let existing = root.find_entry(&Path::keys(&["metadata", "managedFields"]));
    match (show, existing) {
        (false, Some(entry)) => {
            let start = parse::line_range(text, entry.key.span.start).start;
            let end = text[entry.value.span.end..]
                .find('\n')
                .map_or(text.len(), |i| entry.value.span.end + i + 1);
            vec![Edit {
                range: start..end,
                text: String::new(),
            }]
        }
        (true, None) => {
            let Some(managed) = managed else {
                return Vec::new();
            };
            let Some(entries) = meta.as_map() else {
                return Vec::new();
            };
            let Some(first) = entries.first() else {
                return Vec::new();
            };
            let indent = column(text, first.key.span.start);
            let block = to_yaml(&serde_json::json!({ "managedFields": managed }));
            let pad = " ".repeat(indent);
            let block: String = block.lines().map(|l| format!("{pad}{l}\n")).collect();
            // Insert after the last metadata entry (keeps kubectl's order roughly).
            let last = entries.last().expect("non-empty");
            let at = text[last.value.span.end..]
                .find('\n')
                .map_or(text.len(), |i| last.value.span.end + i + 1);
            vec![Edit {
                range: at..at,
                text: block,
            }]
        }
        _ => Vec::new(),
    }
}

/// Fields the API server sets; they are dropped before applying.
pub fn strip_server_fields(object: &mut Value) {
    if let Some(meta) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        for key in [
            "managedFields",
            "resourceVersion",
            "uid",
            "generation",
            "creationTimestamp",
            "selfLink",
        ] {
            meta.remove(key);
        }
    }
    if let Some(map) = object.as_object_mut() {
        map.remove("status");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn secret() -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Secret",
            "metadata": {"name": "db", "annotations": {LAST_APPLIED: "{\"data\":{\"password\":\"aHVudGVyMg==\"}}"}, "managedFields": [{"manager": "kubectl"}, {"manager": "helm"}, {"manager": "kubectl"}]},
            "data": {
                "password": STANDARD.encode("hunter2"),
                "tls.crt": STANDARD.encode("-----BEGIN\nabc\n-----END\n"),
                "blob": STANDARD.encode([0xff_u8, 0xfe, 0x00]),
            },
            "type": "Opaque"
        })
    }

    #[test]
    fn secrets_are_masked_and_never_base64() {
        let (text, values) = render(&secret(), RenderOptions::default());
        let values = values.unwrap();
        assert!(text.contains(&format!("password: {MASK}")), "{text}");
        assert!(!text.contains(&STANDARD.encode("hunter2")));
        assert!(text.contains(&format!("{LAST_APPLIED}: {MASK}")), "{text}");
        assert!(!text.contains("managedFields"));
        assert!(text.contains(&format!("blob: {}", STANDARD.encode([0xff_u8, 0xfe, 0x00]))));
        assert_eq!(values.binary.len(), 1);

        // Reveal in the buffer, edit one value, mask again: only the untouched one masks.
        let revealed = apply_edits(&text, toggle_secret_edits(&text, &values, true));
        assert!(revealed.contains("password: hunter2"), "{revealed}");
        assert!(
            revealed.contains("tls.crt: |\n    -----BEGIN\n    abc\n    -----END\n"),
            "{revealed}"
        );
        let edited = revealed.replace("hunter2", "correct horse");
        let masked = apply_edits(&edited, toggle_secret_edits(&edited, &values, false));
        assert!(masked.contains("password: correct horse"), "{masked}");
        assert!(masked.contains(&format!("tls.crt: {MASK}")), "{masked}");

        // Back to the wire format.
        let parsed = parse::parse(&masked);
        let mut object = parsed.roots().next().unwrap().to_json();
        restore_secret(&mut object, Some(&values));
        assert_eq!(object["data"]["password"], STANDARD.encode("correct horse"));
        assert_eq!(object["data"]["tls.crt"], secret()["data"]["tls.crt"]);
        assert_eq!(object["data"]["blob"], secret()["data"]["blob"]);
        assert_eq!(
            object["metadata"]["annotations"][LAST_APPLIED],
            secret()["metadata"]["annotations"][LAST_APPLIED]
        );
    }

    const ROUTE_KEY: &str =
        "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBg\n-----END PRIVATE KEY-----\n";

    fn route() -> Value {
        json!({
            "apiVersion": "route.openshift.io/v1",
            "kind": "Route",
            "metadata": {"name": "shop-secure", "namespace": "shop",
                "annotations": {LAST_APPLIED: format!("{{\"spec\":{{\"tls\":{{\"key\":{ROUTE_KEY:?}}}}}}}")}},
            "spec": {
                "host": "secure.apps.example.com",
                "to": {"kind": "Service", "name": "shop-api"},
                "tls": {"termination": "reencrypt", "certificate": "CERT", "key": ROUTE_KEY}
            }
        })
    }

    #[test]
    fn route_keys_are_masked_revealed_and_restored() {
        let (text, values) = render(&route(), RenderOptions::default());
        let values = values.expect("a Route with a key keeps it in memory");
        assert!(!text.contains("MIIEvQIBADANBg"), "{text}");
        assert!(text.contains(&format!("key: {MASK}")), "{text}");
        assert!(text.contains(&format!("{LAST_APPLIED}: {MASK}")), "{text}");
        assert!(text.contains("certificate: CERT"), "{text}");
        assert!(!format!("{values:?}").contains("MIIEvQIBADANBg"));

        // The toggle reveals it as a block, masks it again, and an edit stays.
        let revealed = apply_edits(&text, toggle_secret_edits(&text, &values, true));
        assert!(
            revealed.contains("key: |\n      -----BEGIN PRIVATE KEY-----\n      MIIEvQIBADANBg"),
            "{revealed}"
        );
        let masked = apply_edits(&revealed, toggle_secret_edits(&revealed, &values, false));
        assert_eq!(masked, text);
        let (shown, _) = render(
            &route(),
            RenderOptions {
                reveal_secrets: true,
                ..Default::default()
            },
        );
        assert!(shown.contains("MIIEvQIBADANBg"), "{shown}");
        assert!(
            shown.contains(&format!("{LAST_APPLIED}: {MASK}")),
            "{shown}"
        );

        // Applying a masked key sends the original; a new key is sent as typed.
        let mut object = parse::parse(&text).roots().next().unwrap().to_json();
        restore_secret(&mut object, Some(&values));
        assert_eq!(object["spec"]["tls"]["key"], ROUTE_KEY);
        assert_eq!(
            object["metadata"]["annotations"][LAST_APPLIED],
            route()["metadata"]["annotations"][LAST_APPLIED]
        );
        let edited = text.replace(&format!("key: {MASK}"), "key: new-key");
        let mut object = parse::parse(&edited).roots().next().unwrap().to_json();
        restore_secret(&mut object, Some(&values));
        assert_eq!(object["spec"]["tls"]["key"], "new-key");

        // Rendered manifests (Helm, Argo CD) mask keys without a live object.
        let manifest = to_yaml(&route());
        let masked = apply_edits(&manifest, mask_route_key_edits(&manifest));
        assert!(!masked.contains("MIIEvQIBADANBg"), "{masked}");
        assert!(masked.contains(&format!("key: {MASK}")), "{masked}");
        assert!(mask_route_key_edits(&masked).is_empty());
        // Routes without a key (in `spec` or the last-applied copy) render as they are.
        let mut plain = route();
        plain["spec"]["tls"] = json!({"termination": "edge"});
        plain["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("annotations");
        assert!(render(&plain, RenderOptions::default()).1.is_none());
    }

    #[test]
    fn managers_and_managed_fields_toggle() {
        let object = secret();
        assert_eq!(managers(&object), ["kubectl", "helm"]);
        let (text, _) = render(&object, RenderOptions::default());
        let shown = apply_edits(
            &text,
            toggle_managed_fields_edits(&text, object.pointer("/metadata/managedFields"), true),
        );
        let parsed = parse::parse(&shown);
        let root = parsed.roots().next().unwrap();
        assert_eq!(
            root.to_json()["metadata"]["managedFields"],
            object["metadata"]["managedFields"]
        );
        let hidden = apply_edits(&shown, toggle_managed_fields_edits(&shown, None, false));
        assert_eq!(hidden, text);
    }

    #[test]
    fn scalars_are_quoted_when_needed() {
        assert_eq!(yaml_scalar("hunter2", 2), "hunter2");
        assert_eq!(yaml_scalar("123", 2), "\"123\"");
        assert_eq!(yaml_scalar("yes", 2), "\"yes\"");
        assert_eq!(yaml_scalar("a: b", 2), "\"a: b\"");
        assert_eq!(yaml_scalar("", 2), "\"\"");
        assert_eq!(yaml_scalar("a\nb", 0), "|-\n  a\n  b");
    }

    #[test]
    fn strips_server_fields() {
        let mut object =
            json!({"metadata": {"name": "x", "resourceVersion": "1", "uid": "u"}, "status": {}});
        strip_server_fields(&mut object);
        assert_eq!(object, json!({"metadata": {"name": "x"}}));
    }

    /// The key was taken out of `spec`; kubectl's last-applied copy still holds it.
    #[test]
    fn stale_route_keys_in_last_applied_are_masked_and_restored() {
        let mut stale = route();
        stale["spec"]["tls"].as_object_mut().unwrap().remove("key");
        let (text, values) = render(&stale, RenderOptions::default());
        let values = values.expect("the last-applied copy is kept in memory");
        assert!(values.route_key.is_none());
        assert!(!text.contains("MIIEvQIBADANBg"), "{text}");
        assert!(text.contains(&format!("{LAST_APPLIED}: {MASK}")), "{text}");
        // Applying sends the original annotation back.
        let mut object = parse::parse(&text).roots().next().unwrap().to_json();
        restore_secret(&mut object, Some(&values));
        assert_eq!(
            object["metadata"]["annotations"][LAST_APPLIED],
            stale["metadata"]["annotations"][LAST_APPLIED]
        );
        // Rendered manifests mask it too.
        let manifest = to_yaml(&stale);
        let masked = apply_edits(&manifest, mask_route_key_edits(&manifest));
        assert!(!masked.contains("MIIEvQIBADANBg"), "{masked}");
        assert!(mask_route_key_edits(&masked).is_empty());
    }
}
