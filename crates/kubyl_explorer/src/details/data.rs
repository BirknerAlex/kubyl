//! The "Data" of ConfigMaps and Secrets in the details, and "Used by" for both.
//!
//! One block per key, sorted by name: the key, its size and line count, a format chip (YAML,
//! JSON, properties, shell, XML or text, from the extension and a content sniff) and a copy
//! button. A block shows its first [`PREVIEW_LINES`] lines ("Show all" for the rest, up to
//! [`MAX_LINES`]), YAML with light highlighting. `binaryData` shows its decoded size with copy
//! as base64 and "Save to file…". Secrets stay masked until a key is revealed, then use the
//! same block.
//!
//! Large ConfigMaps (up to 1 MiB) stay cheap: the model (decoding, line counts, formats) is
//! built once per object version, and only the lines a block shows are laid out.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use gpui::{
    AnyElement, App, ClipboardItem, Context, Entity, FontStyle, FontWeight, HighlightStyle,
    IntoElement, SharedString, StyledText, Subscription, Window, div, prelude::*,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{Notification, NotificationCenter, ResourceRef, ViewKind, ViewRequest};
use kubyl_resources::format::{array_at, format_bytes, str_at};
use kubyl_ui::{Colors, Icon, IconButton, IconName, fonts, h_flex, u, v_flex};
use serde_json::Value;

use super::{DetailsContent, SECRET_MASK, Target, section};

/// Lines a block shows until "Show all".
pub const PREVIEW_LINES: usize = 20;
/// An expanded block stops here and offers the YAML editor.
pub const MAX_LINES: usize = 2000;
/// Longer lines are cut (minified JSON can be one 1 MiB line).
const PREVIEW_LINE_CHARS: usize = 400;
const MAX_LINE_CHARS: usize = 2000;
/// More keys than this get a filter and "Expand all"/"Collapse all".
pub const FILTER_ABOVE: usize = 8;

/// What a value looks like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Json,
    Properties,
    Shell,
    Xml,
    Text,
    Binary,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Yaml => "YAML",
            Format::Json => "JSON",
            Format::Properties => "properties",
            Format::Shell => "shell",
            Format::Xml => "XML",
            Format::Text => "text",
            Format::Binary => "binary",
        }
    }
}

/// One key of a ConfigMap or Secret. `Debug` never shows the value (Secrets).
#[derive(Clone)]
pub struct DataEntry {
    pub key: String,
    /// The value as text; `None` for binary values.
    pub text: Option<Arc<str>>,
    /// The decoded bytes of a binary value.
    pub bytes: Option<Arc<Vec<u8>>>,
    /// Decoded size in bytes.
    pub size: usize,
    pub lines: usize,
    pub format: Format,
}

impl std::fmt::Debug for DataEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataEntry")
            .field("key", &self.key)
            .field("size", &self.size)
            .field("lines", &self.lines)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl DataEntry {
    fn text(key: &str, text: &str) -> Self {
        Self {
            key: key.to_string(),
            size: text.len(),
            lines: line_count(text),
            format: detect_format(key, text),
            text: Some(Arc::from(text)),
            bytes: None,
        }
    }

    fn binary(key: &str, bytes: Vec<u8>) -> Self {
        Self {
            key: key.to_string(),
            text: None,
            size: bytes.len(),
            lines: 0,
            format: Format::Binary,
            bytes: Some(Arc::new(bytes)),
        }
    }

    pub fn is_binary(&self) -> bool {
        self.text.is_none()
    }

    /// A single-line text value (what `.env` can hold).
    pub fn single_line(&self) -> Option<&str> {
        self.text
            .as_deref()
            .filter(|t| !t.trim_end_matches('\n').contains('\n'))
    }
}

/// The keys of an object, decoded once per version.
#[derive(Clone, Debug, Default)]
pub struct DataModel {
    pub entries: Vec<DataEntry>,
    pub total: usize,
}

impl DataModel {
    fn new(mut entries: Vec<DataEntry>) -> Self {
        // `data` first, then `binaryData`, each sorted by name (like kubectl).
        entries.sort_by(|a, b| a.is_binary().cmp(&b.is_binary()).then(a.key.cmp(&b.key)));
        let total = entries.iter().map(|e| e.size).sum();
        Self { entries, total }
    }

    pub fn binary_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_binary()).count()
    }
}

/// `data` (text) and `binaryData` (base64) of a ConfigMap.
pub fn configmap_model(object: &Value) -> DataModel {
    let mut entries = Vec::new();
    if let Some(map) = object.get("data").and_then(Value::as_object) {
        for (key, value) in map {
            entries.push(DataEntry::text(key, value.as_str().unwrap_or_default()));
        }
    }
    if let Some(map) = object.get("binaryData").and_then(Value::as_object) {
        for (key, value) in map {
            let raw = value.as_str().unwrap_or_default();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw)
                .unwrap_or_default();
            entries.push(DataEntry::binary(key, bytes));
        }
    }
    DataModel::new(entries)
}

/// `data` (base64) and `stringData` of a Secret, decoded; values that aren't UTF-8 are binary.
/// Never keeps raw base64.
pub fn secret_model(object: &Value) -> DataModel {
    let mut entries = Vec::new();
    if let Some(map) = object.get("data").and_then(Value::as_object) {
        for (key, value) in map {
            let raw = value.as_str().unwrap_or_default();
            match base64::engine::general_purpose::STANDARD.decode(raw) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => entries.push(DataEntry::text(key, &text)),
                    Err(err) => entries.push(DataEntry::binary(key, err.into_bytes())),
                },
                Err(_) => entries.push(DataEntry::text(key, "<invalid base64>")),
            }
        }
    }
    if let Some(map) = object.get("stringData").and_then(Value::as_object) {
        for (key, value) in map {
            entries.push(DataEntry::text(key, value.as_str().unwrap_or_default()));
        }
    }
    DataModel::new(entries)
}

/// Lines of a value: `""` has none, a trailing newline doesn't start another.
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.trim_end_matches('\n').split('\n').count()
    }
}

/// The format from the key's extension, else a sniff of the first meaningful line.
pub fn detect_format(key: &str, text: &str) -> Format {
    let lower = key.to_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, ext)| ext);
    match extension {
        Some("yaml" | "yml") => return Format::Yaml,
        Some("json") => return Format::Json,
        Some("properties" | "env" | "ini") => return Format::Properties,
        Some("sh" | "bash" | "zsh") => return Format::Shell,
        Some("xml" | "xsd" | "xsl" | "html" | "svg") => return Format::Xml,
        _ => {}
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Format::Text;
    }
    if trimmed.starts_with("#!") {
        return Format::Shell;
    }
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.contains('"'))
    {
        return Format::Json;
    }
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        return Format::Xml;
    }
    // A single line is text (`info`, a URL); structure needs a few lines.
    let lines: Vec<&str> = trimmed
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .take(10)
        .collect();
    if lines.len() < 2 {
        return Format::Text;
    }
    let is_key = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '"'))
    };
    let yaml = lines.iter().all(|line| {
        let l = line.trim_start();
        l.starts_with("- ")
            || l == "-"
            || l.split_once(':')
                .is_some_and(|(k, rest)| is_key(k) && (rest.is_empty() || rest.starts_with(' ')))
            || line.starts_with(' ')
    });
    if yaml {
        return Format::Yaml;
    }
    let properties = lines.iter().all(|line| {
        line.split_once('=')
            .is_some_and(|(k, _)| is_key(k.trim()) && !k.trim().is_empty())
    });
    if properties {
        return Format::Properties;
    }
    Format::Text
}

/// The first `max_lines` lines, each cut at `max_chars`. Returns the text and whether lines
/// were cut.
pub fn head(text: &str, max_lines: usize, max_chars: usize) -> (String, bool) {
    let mut out = String::new();
    let mut cut = false;
    for (ix, line) in text.trim_end_matches('\n').split('\n').enumerate() {
        if ix == max_lines {
            break;
        }
        if ix > 0 {
            out.push('\n');
        }
        if line.chars().count() > max_chars {
            out.extend(line.chars().take(max_chars));
            out.push('…');
            cut = true;
        } else {
            out.push_str(line);
        }
    }
    (out, cut)
}

/// `KEY=value` lines of the single-line values (quoted when needed), and how many keys were
/// skipped (multi-line or binary).
pub fn env_export(entries: &[DataEntry]) -> (String, usize) {
    let mut out = String::new();
    let mut skipped = 0;
    for entry in entries {
        let Some(value) = entry.single_line() else {
            skipped += 1;
            continue;
        };
        let value = value.trim_end_matches('\n');
        let plain = !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_alphanumeric() || "-_./:@,+%=".contains(c));
        if plain || value.is_empty() {
            out.push_str(&format!("{}={value}\n", entry.key));
        } else {
            let escaped = value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('$', "\\$");
            out.push_str(&format!("{}=\"{escaped}\"\n", entry.key));
        }
    }
    (out, skipped)
}

/// The `data` map as YAML (what `kubectl create configmap --from-file` would hold).
pub fn yaml_export(object: &Value) -> String {
    match object.get("data") {
        Some(data) if data.is_object() => kubyl_resources::format::to_yaml(data),
        _ => String::new(),
    }
}

/// What a span of a YAML line is, for highlighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YamlSpan {
    Key,
    String,
    Scalar,
    Comment,
    Punctuation,
}

/// Highlight spans (byte ranges into `text`) of YAML, line by line: keys, quoted and plain
/// strings, numbers/booleans/null, comments. Good enough for reading; not a parser.
pub fn yaml_spans(text: &str) -> Vec<(Range<usize>, YamlSpan)> {
    let mut spans = Vec::new();
    let mut offset = 0;
    for line in text.split('\n') {
        yaml_line(line, offset, &mut spans);
        offset += line.len() + 1;
    }
    spans
}

fn yaml_line(line: &str, base: usize, spans: &mut Vec<(Range<usize>, YamlSpan)>) {
    let indent = line.len() - line.trim_start().len();
    let mut rest = &line[indent..];
    let mut at = base + indent;
    if rest.starts_with('#') {
        spans.push((at..base + line.len(), YamlSpan::Comment));
        return;
    }
    if let Some(stripped) = rest.strip_prefix("- ") {
        spans.push((at..at + 1, YamlSpan::Punctuation));
        at += 2;
        rest = stripped;
    }
    // `key:` (not inside a quoted scalar).
    let quoted = rest.starts_with('"') || rest.starts_with('\'');
    let key_end = (!quoted)
        .then(|| {
            rest.char_indices().find_map(|(ix, c)| {
                (c == ':' && rest[ix + 1..].chars().next().is_none_or(|n| n == ' ')).then_some(ix)
            })
        })
        .flatten();
    let value = match key_end {
        Some(ix) => {
            spans.push((at..at + ix, YamlSpan::Key));
            spans.push((at + ix..at + ix + 1, YamlSpan::Punctuation));
            let value = &rest[ix + 1..];
            let skip = value.len() - value.trim_start().len();
            at += ix + 1 + skip;
            value.trim_start()
        }
        None => rest,
    };
    if value.is_empty() {
        return;
    }
    // A trailing comment (` #` outside quotes).
    let (scalar, comment) = match value
        .char_indices()
        .find(|(ix, c)| *c == '#' && *ix > 0 && value.as_bytes()[ix - 1] == b' ')
    {
        Some((ix, _)) if !value.starts_with('"') && !value.starts_with('\'') => {
            (value[..ix].trim_end(), Some(ix))
        }
        _ => (value, None),
    };
    if !scalar.is_empty() {
        let kind = if matches!(scalar, "|" | ">" | "|-" | ">-" | "{}" | "[]") {
            YamlSpan::Punctuation
        } else if matches!(
            scalar,
            "true" | "false" | "null" | "~" | "yes" | "no" | "on" | "off"
        ) || scalar.parse::<f64>().is_ok()
        {
            YamlSpan::Scalar
        } else {
            YamlSpan::String
        };
        spans.push((at..at + scalar.len(), kind));
    }
    if let Some(ix) = comment {
        spans.push((at + ix..at + value.len(), YamlSpan::Comment));
    }
}

/// ConfigMaps or Secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    ConfigMap,
    Secret,
}

/// How one pod uses the object: `volume config → a.yaml, b.json`, `envFrom`, `env LOG_LEVEL`.
pub fn pod_uses(pod: &Value, source: Source, name: &str) -> Vec<String> {
    let (volume_field, env_ref, env_from_ref, name_field) = match source {
        Source::ConfigMap => ("configMap", "configMapKeyRef", "configMapRef", "name"),
        Source::Secret => ("secret", "secretKeyRef", "secretRef", "secretName"),
    };
    let mut uses = Vec::new();
    let keys_of = |items: &[Value]| -> String {
        let keys: Vec<&str> = items.iter().map(|i| str_at(i, "/key")).collect();
        if keys.is_empty() {
            "all keys".into()
        } else {
            keys.join(", ")
        }
    };
    for volume in array_at(pod, "/spec/volumes") {
        let volume_name = str_at(volume, "/name");
        if let Some(reference) = volume.get(volume_field)
            && reference.get(name_field).and_then(Value::as_str) == Some(name)
        {
            uses.push(format!(
                "volume {volume_name} → {}",
                keys_of(array_at(reference, "/items"))
            ));
        }
        for projected in array_at(volume, "/projected/sources") {
            if let Some(reference) = projected.get(volume_field)
                && reference.get("name").and_then(Value::as_str) == Some(name)
            {
                uses.push(format!(
                    "projected volume {volume_name} → {}",
                    keys_of(array_at(reference, "/items"))
                ));
            }
        }
    }
    let containers = ["/spec/initContainers", "/spec/containers"]
        .into_iter()
        .flat_map(|pointer| array_at(pod, pointer));
    for container in containers {
        for env in array_at(container, "/env") {
            if let Some(reference) = env.pointer(&format!("/valueFrom/{env_ref}"))
                && str_at(reference, "/name") == name
            {
                let variable = str_at(env, "/name");
                let key = str_at(reference, "/key");
                uses.push(if variable == key {
                    format!("env {variable}")
                } else {
                    format!("env {variable} ← {key}")
                });
            }
        }
        for from in array_at(container, "/envFrom") {
            if from.get(env_from_ref).map(|r| str_at(r, "/name")) == Some(name) {
                let prefix = str_at(from, "/prefix");
                uses.push(if prefix.is_empty() {
                    "envFrom".into()
                } else {
                    format!("envFrom (prefix {prefix})")
                });
            }
        }
    }
    uses.sort();
    uses.dedup();
    uses
}

/// Pods using the object, grouped by what runs them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsedBy {
    /// `Deployment`, `StatefulSet`, `Job`… or `Pod` for pods without an owner.
    pub kind: String,
    pub name: String,
    /// The group of the owner's API (`apps`, `batch`), for the link.
    pub group: String,
    pub pods: usize,
    pub uses: Vec<String>,
}

/// The workload that runs a pod: its controller, with ReplicaSets of Deployments mapped to the
/// Deployment (the ReplicaSet's name is `<deployment>-<pod-template-hash>`).
pub fn owner_of(pod: &Value) -> (String, String, String) {
    let owners = array_at(pod, "/metadata/ownerReferences");
    let Some(owner) = owners
        .iter()
        .find(|o| o["controller"].as_bool() == Some(true))
        .or_else(|| owners.first())
    else {
        return (
            "Pod".into(),
            str_at(pod, "/metadata/name").into(),
            String::new(),
        );
    };
    let kind = str_at(owner, "/kind");
    let name = str_at(owner, "/name");
    let group = str_at(owner, "/apiVersion")
        .split_once('/')
        .map(|(g, _)| g.to_string())
        .unwrap_or_default();
    let hash = str_at(pod, "/metadata/labels/pod-template-hash");
    if kind == "ReplicaSet"
        && !hash.is_empty()
        && let Some(deployment) = name.strip_suffix(&format!("-{hash}"))
    {
        return ("Deployment".into(), deployment.into(), "apps".into());
    }
    (kind.into(), name.into(), group)
}

/// Who uses the object, from the pods of its namespace. No extra API calls.
pub fn used_by<'a>(
    pods: impl IntoIterator<Item = &'a Value>,
    source: Source,
    name: &str,
) -> Vec<UsedBy> {
    let mut groups: BTreeMap<(String, String), UsedBy> = BTreeMap::new();
    for pod in pods {
        let uses = pod_uses(pod, source, name);
        if uses.is_empty() {
            continue;
        }
        let (kind, owner, group) = owner_of(pod);
        let entry = groups
            .entry((kind.clone(), owner.clone()))
            .or_insert_with(|| UsedBy {
                kind,
                name: owner,
                group,
                pods: 0,
                uses: Vec::new(),
            });
        entry.pods += 1;
        for use_ in uses {
            if !entry.uses.contains(&use_) {
                entry.uses.push(use_);
            }
        }
    }
    groups.into_values().collect()
}

/// UI state of the Data section (per shown object).
#[derive(Default)]
pub struct DataUi {
    /// Keys whose block shows only its header.
    collapsed: HashSet<String>,
    /// Keys showing all lines instead of the preview.
    full: HashSet<String>,
    /// Keys with soft wrap off.
    nowrap: HashSet<String>,
    filter: Option<Entity<InputState>>,
    _filter_subscription: Option<Subscription>,
    /// The model of the shown object version: `(uid, resourceVersion, secret)`.
    model: Option<((String, String, bool), Arc<DataModel>)>,
}

impl DataUi {
    fn model(&mut self, object: &Value, secret: bool) -> Arc<DataModel> {
        let key = (
            str_at(object, "/metadata/uid").to_string(),
            str_at(object, "/metadata/resourceVersion").to_string(),
            secret,
        );
        if let Some((cached, model)) = &self.model
            && *cached == key
        {
            return model.clone();
        }
        let model = Arc::new(if secret {
            secret_model(object)
        } else {
            configmap_model(object)
        });
        self.model = Some((key, model.clone()));
        model
    }

    fn query(&self, cx: &App) -> String {
        self.filter
            .as_ref()
            .map(|f| f.read(cx).value().trim().to_lowercase())
            .unwrap_or_default()
    }
}

impl DetailsContent {
    /// Creates the key filter once the shown object has more than [`FILTER_ABOVE`] keys.
    pub(super) fn ensure_data_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(target), Some(object)) = (&self.target, &self.object) else {
            return;
        };
        let secret = match target.kind.as_str() {
            "ConfigMap" => false,
            "Secret" => true,
            _ => return,
        };
        let object = object.clone();
        let count = self.data.model(&object, secret).entries.len();
        if count <= FILTER_ABOVE || self.data.filter.is_some() {
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(format!("Filter {count} keys")));
        self.data._filter_subscription =
            Some(cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        self.data.filter = Some(input);
    }

    /// Chips for the header: `12 keys · 1 binary`, the total size, `immutable`.
    pub(super) fn data_chips(&mut self, object: &Value, secret: bool) -> Vec<String> {
        let model = self.data.model(object, secret);
        let binary = model.binary_count();
        let mut chips = vec![if binary > 0 {
            format!("{} keys · {binary} binary", model.entries.len() - binary)
        } else {
            format!("{} keys", model.entries.len())
        }];
        if !secret {
            chips.push(format_size(model.total));
        }
        chips
    }

    /// The Data section of a ConfigMap (`secret: false`) or Secret.
    pub(super) fn render_data(
        &mut self,
        object: &Value,
        target: &Target,
        secret: bool,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let model = self.data.model(object, secret);
        if model.entries.is_empty() {
            return Vec::new();
        }
        let query = self.data.query(cx);
        let many = model.entries.len() > FILTER_ABOVE;
        let mut header = h_flex().gap(u(4.0)).child(
            div()
                .flex_1()
                .text_size(u(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_dim)
                .child("DATA"),
        );
        if many {
            let keys: Vec<String> = model.entries.iter().map(|e| e.key.clone()).collect();
            let all = keys.clone();
            header = header
                .child(
                    small_button("data-expand-all", None, "Expand all", colors).on_click(
                        cx.listener(move |this, _, _, cx| {
                            for key in &all {
                                this.data.collapsed.remove(key);
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    small_button("data-collapse-all", None, "Collapse all", colors).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.data.collapsed.extend(keys.iter().cloned());
                            cx.notify();
                        }),
                    ),
                );
        }
        if !secret {
            header = header.child(copy_all_menu(object, &model, colors));
        }
        let mut body = v_flex().gap(u(6.0));
        if let Some(filter) = &self.data.filter {
            body = body.child(
                div()
                    .h(u(26.0))
                    .px(u(6.0))
                    .flex()
                    .items_center()
                    .rounded(u(4.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(colors.border)
                    .text_size(u(12.0))
                    .child(
                        Input::new(filter)
                            .appearance(false)
                            .prefix(Icon::new(IconName::Funnel).size(12.0)),
                    ),
            );
        }
        let mut shown = 0;
        for (ix, entry) in model.entries.iter().enumerate() {
            if !query.is_empty() && !entry.key.to_lowercase().contains(&query) {
                continue;
            }
            shown += 1;
            let block = if secret && !self.revealed.contains(&entry.key) {
                self.masked_row(ix, entry, colors, cx)
            } else {
                self.block(ix, entry, target, secret, colors, cx)
            };
            body = body.child(block);
        }
        if shown == 0 {
            body = body.child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("No key matches the filter."),
            );
        }
        vec![
            v_flex()
                .px(u(14.0))
                .py(u(12.0))
                .gap(u(8.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(header)
                .child(body)
                .into_any_element(),
        ]
    }

    /// A Secret key that isn't revealed: the mask, reveal and copy.
    fn masked_row(
        &self,
        ix: usize,
        entry: &DataEntry,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = entry.key.clone();
        h_flex()
            .h(u(28.0))
            .px(u(8.0))
            .gap(u(8.0))
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.subheader_background)
            .child(Icon::new(IconName::Key).size(12.0).color(colors.text_dim))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .child(entry.key.clone()),
            )
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child(SECRET_MASK),
            )
            .child(self.reveal_button(ix, &key, false, cx))
            .child(copy_button(ix, entry))
            .into_any_element()
    }

    fn reveal_button(
        &self,
        ix: usize,
        key: &str,
        revealed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = key.to_string();
        IconButton::new(
            SharedString::from(format!("data-reveal-{ix}")),
            if revealed {
                IconName::EyeOff
            } else {
                IconName::Eye
            },
        )
        .icon_size(12.0)
        .toggled(revealed)
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.revealed.remove(&key) {
                this.revealed.insert(key.clone());
            }
            cx.notify();
        }))
    }

    /// One key's block: header (key, format, size, wrap, copy) and its lines.
    fn block(
        &self,
        ix: usize,
        entry: &DataEntry,
        target: &Target,
        secret: bool,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.data.collapsed.contains(&entry.key);
        let full = self.data.full.contains(&entry.key);
        let wrap = !self.data.nowrap.contains(&entry.key);
        let toggle_key = entry.key.clone();
        let meta = match (entry.is_binary(), entry.lines) {
            (true, _) | (false, 0) => format_size(entry.size),
            (false, 1) => format!("{} · 1 line", format_size(entry.size)),
            (false, n) => format!("{} · {} lines", format_size(entry.size), thousands(n)),
        };
        let mut header = h_flex()
            .id(SharedString::from(format!("data-head-{ix}")))
            .h(u(28.0))
            .pl(u(6.0))
            .pr(u(4.0))
            .gap(u(6.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.data.collapsed.remove(&toggle_key) {
                    this.data.collapsed.insert(toggle_key.clone());
                }
                cx.notify();
            }))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(11.0)
                .color(colors.text_dim),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(fonts::MONO)
                    .text_size(u(12.0))
                    .text_color(colors.text)
                    .child(entry.key.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .px(u(5.0))
                    .rounded(u(3.0))
                    .bg(colors.chip_background)
                    .text_size(u(10.5))
                    .text_color(if entry.is_binary() {
                        colors.purple
                    } else {
                        colors.text_muted
                    })
                    .child(entry.format.label()),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(meta),
            );
        if entry.is_binary() {
            header = header
                .child(copy_base64_button(ix, entry, colors))
                .child(save_button(ix, entry, target, colors));
        } else {
            let wrap_key = entry.key.clone();
            if secret {
                header = header.child(self.reveal_button(ix, &entry.key, true, cx));
            }
            header = header
                .child(
                    IconButton::new(
                        SharedString::from(format!("data-wrap-{ix}")),
                        IconName::WrapText,
                    )
                    .icon_size(12.0)
                    .toggled(wrap)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.data.nowrap.remove(&wrap_key) {
                            this.data.nowrap.insert(wrap_key.clone());
                        }
                        cx.notify();
                    })),
                )
                .child(copy_button(ix, entry));
        }
        let mut block = v_flex()
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.subheader_background)
            .child(header);
        if collapsed {
            return block.into_any_element();
        }
        let content = match (&entry.text, &entry.bytes) {
            (_, Some(bytes)) => div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(format!(
                    "Binary data, {}. Copy it as base64 or save it to a file.",
                    format_size(bytes.len())
                ))
                .into_any_element(),
            (Some(text), _) if text.is_empty() => div()
                .text_size(u(12.0))
                .italic()
                .text_color(colors.text_faint)
                .child("(empty)")
                .into_any_element(),
            (Some(text), _) => {
                let (limit, chars) = if full {
                    (MAX_LINES, MAX_LINE_CHARS)
                } else {
                    (PREVIEW_LINES, PREVIEW_LINE_CHARS)
                };
                let (shown, cut) = head(text, limit, chars);
                self.text_body(ix, entry, shown, cut, full, wrap, target, colors, cx)
            }
            (None, None) => div().into_any_element(),
        };
        block = block.child(div().pl(u(24.0)).pr(u(10.0)).pb(u(8.0)).child(content));
        block.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn text_body(
        &self,
        ix: usize,
        entry: &DataEntry,
        shown: String,
        cut: bool,
        full: bool,
        wrap: bool,
        target: &Target,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text = if entry.format == Format::Yaml {
            let highlights: Vec<(Range<usize>, HighlightStyle)> = yaml_spans(&shown)
                .into_iter()
                .filter(|(range, _)| !range.is_empty())
                .map(|(range, span)| (range, yaml_style(span, colors)))
                .collect();
            StyledText::new(shown).with_highlights(highlights)
        } else {
            StyledText::new(shown)
        };
        let lines = div()
            .id(SharedString::from(format!("data-text-{ix}")))
            .font_family(fonts::MONO)
            .text_size(u(11.5))
            .line_height(u(17.0))
            .text_color(colors.text)
            .map(|this| {
                if wrap {
                    this.w_full()
                } else {
                    this.whitespace_nowrap().overflow_x_scroll()
                }
            })
            .child(text);
        let more = entry.lines > PREVIEW_LINES;
        let key = entry.key.clone();
        let footer = if more || cut {
            let mut footer = h_flex().gap(u(10.0)).pt(u(4.0)).text_size(u(11.5));
            if more {
                footer = footer.child(
                    div()
                        .id(SharedString::from(format!("data-more-{ix}")))
                        .cursor_pointer()
                        .text_color(colors.accent)
                        .hover(|s| s.underline())
                        .child(if full {
                            "Show fewer".to_string()
                        } else {
                            format!("Show all {} lines", thousands(entry.lines))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.data.full.remove(&key) {
                                this.data.full.insert(key.clone());
                            }
                            cx.notify();
                        })),
                );
            }
            if (full && entry.lines > MAX_LINES) || cut {
                let reference = ResourceRef::object(
                    target.cluster.clone(),
                    target.gvr.clone(),
                    target.namespace.clone(),
                    target.name.clone(),
                );
                let note = if full && entry.lines > MAX_LINES {
                    format!(
                        "Showing {} of {} lines.",
                        thousands(MAX_LINES),
                        thousands(entry.lines)
                    )
                } else {
                    "Long lines are cut.".to_string()
                };
                footer = footer
                    .child(div().text_color(colors.text_dim).child(note))
                    .child(
                        div()
                            .id(SharedString::from(format!("data-yaml-{ix}")))
                            .cursor_pointer()
                            .text_color(colors.accent)
                            .hover(|s| s.underline())
                            .child("Open in YAML editor")
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(OpenView(ViewRequest::for_resource(
                                        ViewKind::Yaml,
                                        reference.clone(),
                                    ))),
                                    cx,
                                );
                            }),
                    );
            }
            Some(footer)
        } else {
            None
        };
        v_flex().child(lines).children(footer).into_any_element()
    }

    /// "Used by": pods referencing the object, grouped by their workload.
    pub(super) fn render_used_by(
        &self,
        target: &Target,
        source: Source,
        colors: &Colors,
        cx: &App,
    ) -> Option<AnyElement> {
        let pods = self.related.pods.as_ref()?;
        let store = pods.read(cx);
        if !store.status().is_ready() {
            return None;
        }
        let groups = used_by(
            store.objects().values().map(|p| p.as_ref()),
            source,
            &target.name,
        );
        let total: usize = groups.iter().map(|g| g.pods).sum();
        let mut list = v_flex().gap(u(6.0)).text_size(u(12.0));
        for (ix, group) in groups.iter().enumerate() {
            let resource = match group.kind.as_str() {
                "Deployment" => Some("deployments"),
                "StatefulSet" => Some("statefulsets"),
                "DaemonSet" => Some("daemonsets"),
                "ReplicaSet" => Some("replicasets"),
                "Job" => Some("jobs"),
                "Pod" => Some("pods"),
                _ => None,
            };
            let name: AnyElement = match resource {
                Some(resource) => super::link(
                    SharedString::from(format!("used-by-{ix}")),
                    group.name.clone(),
                    ResourceRef::object(
                        target.cluster.clone(),
                        kubyl_core::Gvr::new(group.group.clone(), "v1", resource),
                        target.namespace.clone(),
                        group.name.clone(),
                    ),
                    colors,
                )
                .into_any_element(),
                None => div().child(group.name.clone()).into_any_element(),
            };
            list = list.child(
                h_flex()
                    .gap(u(8.0))
                    .items_start()
                    .child(
                        Icon::new(super::catalog::icon_for(
                            &group.group,
                            resource.unwrap_or_default(),
                        ))
                        .size(13.0)
                        .color(colors.text_dim),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap(u(5.0))
                                    .flex_wrap()
                                    .child(
                                        div().text_color(colors.text_dim).child(group.kind.clone()),
                                    )
                                    .child(
                                        div()
                                            .font_family(fonts::MONO)
                                            .text_size(u(11.5))
                                            .child(name),
                                    )
                                    .when(group.kind != "Pod", |this| {
                                        this.child(div().text_color(colors.text_dim).child(
                                            format!(
                                                "· {} pod{}",
                                                group.pods,
                                                if group.pods == 1 { "" } else { "s" }
                                            ),
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.0))
                                    .text_color(colors.text_muted)
                                    .child(group.uses.join(" · ")),
                            ),
                    ),
            );
        }
        if groups.is_empty() {
            list = list.child(
                div()
                    .text_color(colors.text_dim)
                    .child("No pod in this namespace uses it."),
            );
        }
        Some(
            section(
                format!("Used by · {total} pod{}", if total == 1 { "" } else { "s" }),
                colors,
            )
            .child(list)
            .into_any_element(),
        )
    }

    /// Forgets the Data section's state (another object is shown).
    pub(super) fn reset_data(&mut self) {
        self.data = DataUi::default();
    }
}

fn yaml_style(span: YamlSpan, colors: &Colors) -> HighlightStyle {
    let color = match span {
        YamlSpan::Key => colors.red,
        YamlSpan::String => colors.green,
        YamlSpan::Scalar => colors.orange,
        YamlSpan::Comment => colors.text_dim,
        YamlSpan::Punctuation => colors.text_muted,
    };
    HighlightStyle {
        color: Some(color),
        font_style: (span == YamlSpan::Comment).then_some(FontStyle::Italic),
        ..Default::default()
    }
}

/// `96 B`, `2.1 KiB`, `18 KiB`, `1 MiB`.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let short = format_bytes(bytes as f64);
    let split = short
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(short.len());
    format!("{} {}B", &short[..split], &short[split..])
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (ix, c) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn small_button(
    id: &'static str,
    icon: Option<IconName>,
    label: &'static str,
    colors: &Colors,
) -> gpui::Stateful<gpui::Div> {
    let hover = colors.hover;
    h_flex()
        .id(id)
        .flex_none()
        .h(u(22.0))
        .px(u(6.0))
        .gap(u(4.0))
        .rounded(u(4.0))
        .text_size(u(12.0))
        .text_color(colors.text_muted)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .children(icon.map(|icon| Icon::new(icon).size(12.0)))
        .child(label)
}

fn copy_button(ix: usize, entry: &DataEntry) -> impl IntoElement {
    let key = entry.key.clone();
    let text = entry.text.clone();
    IconButton::new(
        SharedString::from(format!("data-copy-{ix}")),
        IconName::Copy,
    )
    .icon_size(12.0)
    .on_click(move |_, _, cx| {
        cx.stop_propagation();
        if let Some(text) = &text {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            NotificationCenter::push(cx, Notification::info(format!("Copied {key}")));
        }
    })
}

fn copy_base64_button(ix: usize, entry: &DataEntry, colors: &Colors) -> impl IntoElement {
    let key = entry.key.clone();
    let bytes = entry.bytes.clone();
    super::row_button(
        SharedString::from(format!("data-b64-{ix}")),
        IconName::Copy,
        "base64",
        colors,
    )
    .on_click(move |_, _, cx| {
        cx.stop_propagation();
        if let Some(bytes) = &bytes {
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.as_slice());
            cx.write_to_clipboard(ClipboardItem::new_string(encoded));
            NotificationCenter::push(cx, Notification::info(format!("Copied {key} as base64")));
        }
    })
}

fn save_button(ix: usize, entry: &DataEntry, target: &Target, colors: &Colors) -> impl IntoElement {
    let key = entry.key.clone();
    let bytes = entry.bytes.clone();
    let object = target.name.clone();
    super::row_button(
        SharedString::from(format!("data-save-{ix}")),
        IconName::Download,
        "Save…",
        colors,
    )
    .on_click(move |_, _, cx| {
        cx.stop_propagation();
        let Some(bytes) = bytes.clone() else {
            return;
        };
        save_to_file(key.clone(), object.clone(), bytes, cx);
    })
}

/// Asks for a path and writes the bytes there, off the UI thread.
fn save_to_file(key: String, object: String, bytes: Arc<Vec<u8>>, cx: &mut App) {
    let directory = dirs_download().unwrap_or_else(|| PathBuf::from("."));
    let name: String = key
        .chars()
        .map(|c| if c == '/' || c == '\\' { '_' } else { c })
        .collect();
    let path = cx.prompt_for_new_path(&directory, Some(&name));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = path.await else {
            return;
        };
        let target = path.clone();
        let result = cx
            .background_executor()
            .spawn(async move { std::fs::write(&target, bytes.as_slice()) })
            .await;
        cx.update(|cx| {
            NotificationCenter::push(
                cx,
                match result {
                    Ok(()) => Notification::success(format!(
                        "Saved {key} of {object} to {}",
                        path.display()
                    )),
                    Err(err) => Notification::error(format!("Couldn't save {key}: {err}")),
                },
            )
        });
    })
    .detach();
}

fn dirs_download() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Downloads"))
        .filter(|p| p.is_dir())
}

/// "Copy all": the `data` map as YAML, or the single-line values as `.env`.
fn copy_all_menu(object: &Value, model: &DataModel, colors: &Colors) -> impl IntoElement {
    let yaml = yaml_export(object);
    let (env, skipped) = env_export(&model.entries);
    let env_label = if skipped == 0 {
        "Copy as .env".to_string()
    } else {
        format!(
            "Copy as .env · skips {skipped} multi-line or binary key{}",
            if skipped == 1 { "" } else { "s" }
        )
    };
    Button::new("data-copy-all")
        .ghost()
        .compact()
        .child(
            h_flex()
                .gap(u(4.0))
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child(Icon::new(IconName::Copy).size(12.0))
                .child("Copy all")
                .child(Icon::new(IconName::ChevronDown).size(11.0)),
        )
        .dropdown_menu(move |menu, _, _| {
            let yaml = yaml.clone();
            let env = env.clone();
            menu.item(PopupMenuItem::new("Copy as YAML (the data map)").on_click(
                move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(yaml.clone()));
                    NotificationCenter::push(cx, Notification::info("Copied the data as YAML"));
                },
            ))
            .item(
                PopupMenuItem::new(env_label.clone()).on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(env.clone()));
                    NotificationCenter::push(cx, Notification::info("Copied the data as .env"));
                }),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_from_extension_and_content() {
        assert_eq!(detect_format("app.yaml", "x"), Format::Yaml);
        assert_eq!(detect_format("flags.JSON", ""), Format::Json);
        assert_eq!(detect_format("log4j2.properties", ""), Format::Properties);
        assert_eq!(detect_format("entrypoint.sh", ""), Format::Shell);
        assert_eq!(detect_format("pom.xml", ""), Format::Xml);
        assert_eq!(detect_format("run", "#!/bin/sh\necho hi\n"), Format::Shell);
        assert_eq!(detect_format("flags", "{\"a\": true}"), Format::Json);
        assert_eq!(detect_format("page", "<html></html>"), Format::Xml);
        assert_eq!(
            detect_format("config", "server:\n  port: 8080\nname: x\n"),
            Format::Yaml
        );
        assert_eq!(detect_format("list", "- a\n- b\n"), Format::Yaml);
        assert_eq!(
            detect_format("settings", "a.b=1\nc = two\n# comment\n"),
            Format::Properties
        );
        assert_eq!(detect_format("LOG_LEVEL", "info"), Format::Text);
        assert_eq!(detect_format("url", "http://x:8080/a"), Format::Text);
        assert_eq!(
            detect_format("notes", "Hello there.\nSecond line."),
            Format::Text
        );
    }

    #[test]
    fn line_counts_and_heads() {
        assert_eq!(line_count(""), 0);
        assert_eq!(line_count("a"), 1);
        assert_eq!(line_count("a\nb\n"), 2);
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let (head_, cut) = head(&text, PREVIEW_LINES, 400);
        assert_eq!(head_.lines().count(), 20);
        assert!(!cut);
        let long = "x".repeat(1000);
        let (head_, cut) = head(&long, 20, 400);
        assert!(cut);
        assert_eq!(head_.chars().count(), 401);
    }

    #[test]
    fn configmap_models_sort_and_decode() {
        let cm = json!({
            "metadata": {"uid": "u", "resourceVersion": "1"},
            "data": {"b.yaml": "a: 1\n", "A": "x", "empty": ""},
            "binaryData": {"trust.jks": "AAEC"}
        });
        let model = configmap_model(&cm);
        let keys: Vec<&str> = model.entries.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["A", "b.yaml", "empty", "trust.jks"]);
        let binary = &model.entries[3];
        assert!(binary.is_binary());
        assert_eq!(binary.size, 3);
        assert_eq!(model.binary_count(), 1);
        assert_eq!(model.total, 1 + 5 + 3);
        assert_eq!(model.entries[1].format, Format::Yaml);
    }

    #[test]
    fn secret_models_never_keep_base64() {
        let secret = json!({
            "data": {"password": "aHVudGVyMg==", "blob": "/wA=", "bad": "!!"},
            "stringData": {"user": "admin"}
        });
        let model = secret_model(&secret);
        let get = |k: &str| model.entries.iter().find(|e| e.key == k).unwrap();
        assert_eq!(get("password").text.as_deref(), Some("hunter2"));
        assert!(get("blob").is_binary());
        assert_eq!(get("bad").text.as_deref(), Some("<invalid base64>"));
        assert_eq!(get("user").text.as_deref(), Some("admin"));
        let debug = format!("{model:?}");
        assert!(
            !debug.contains("aHVudGVyMg") && !debug.contains("hunter2"),
            "{debug}"
        );
        assert_eq!(format_size(96), "96 B");
        assert_eq!(format_size(2150), "2.1 KiB");
        assert_eq!(format_size(18 * 1024 + 400), "18 KiB");
    }

    #[test]
    fn env_export_skips_multiline_and_binary() {
        let entries = vec![
            DataEntry::text("LOG_LEVEL", "info"),
            DataEntry::text("GREETING", "hello \"world\" $HOME"),
            DataEntry::text("config.yaml", "a: 1\nb: 2\n"),
            DataEntry::text("EMPTY", ""),
            DataEntry::binary("blob", vec![0, 1]),
        ];
        let (env, skipped) = env_export(&entries);
        assert_eq!(
            env,
            "LOG_LEVEL=info\nGREETING=\"hello \\\"world\\\" \\$HOME\"\nEMPTY=\n"
        );
        assert_eq!(skipped, 2);
        let yaml = yaml_export(&json!({"data": {"a": "1", "b": "x\ny"}}));
        assert!(
            yaml.contains("a: '1'") || yaml.contains("a: \"1\""),
            "{yaml}"
        );
    }

    #[test]
    fn yaml_highlighting() {
        let text = "server:\n  port: 8080 # the port\n  name: \"x: y\"\n- item\n# note";
        let spans = yaml_spans(text);
        let of = |kind: YamlSpan| -> Vec<&str> {
            spans
                .iter()
                .filter(|(_, k)| *k == kind)
                .map(|(r, _)| &text[r.clone()])
                .collect()
        };
        assert_eq!(of(YamlSpan::Key), ["server", "port", "name"]);
        assert_eq!(of(YamlSpan::Scalar), ["8080"]);
        assert_eq!(of(YamlSpan::String), ["\"x: y\"", "item"]);
        assert_eq!(of(YamlSpan::Comment), ["# the port", "# note"]);
    }

    fn pod(name: &str, owner: Option<(&str, &str)>, hash: &str, spec: Value) -> Value {
        let mut pod = json!({"metadata": {"name": name, "labels": {"pod-template-hash": hash}}, "spec": spec});
        if let Some((kind, owner)) = owner {
            pod["metadata"]["ownerReferences"] = json!([{
                "apiVersion": if kind == "ReplicaSet" { "apps/v1" } else { "batch/v1" },
                "kind": kind, "name": owner, "controller": true
            }]);
        }
        pod
    }

    #[test]
    fn used_by_groups_pods_by_workload() {
        let spec = json!({
            "volumes": [
                {"name": "config", "configMap": {"name": "app", "items": [{"key": "a.yaml", "path": "a"}]}},
                {"name": "all", "projected": {"sources": [{"configMap": {"name": "app"}}, {"secret": {"name": "app"}}]}}
            ],
            "containers": [{"name": "c",
                "env": [{"name": "LEVEL", "valueFrom": {"configMapKeyRef": {"name": "app", "key": "LOG_LEVEL"}}},
                        {"name": "LOG_LEVEL", "valueFrom": {"configMapKeyRef": {"name": "app", "key": "LOG_LEVEL"}}},
                        {"name": "PW", "valueFrom": {"secretKeyRef": {"name": "db", "key": "password"}}}],
                "envFrom": [{"configMapRef": {"name": "app"}, "prefix": "APP_"}]}]
        });
        let pods = [
            pod(
                "web-6d9f-abc",
                Some(("ReplicaSet", "web-6d9f")),
                "6d9f",
                spec.clone(),
            ),
            pod(
                "web-6d9f-def",
                Some(("ReplicaSet", "web-6d9f")),
                "6d9f",
                spec.clone(),
            ),
            pod(
                "migrate-1-x",
                Some(("Job", "migrate-1")),
                "",
                json!({"containers": [{"name": "m", "envFrom": [{"configMapRef": {"name": "app"}}]}]}),
            ),
            pod("other", None, "", json!({"containers": [{"name": "o"}]})),
        ];
        let groups = used_by(pods.iter(), Source::ConfigMap, "app");
        assert_eq!(groups.len(), 2);
        let web = groups.iter().find(|g| g.kind == "Deployment").unwrap();
        assert_eq!(web.name, "web");
        assert_eq!(web.pods, 2);
        assert_eq!(
            web.uses,
            [
                "env LEVEL ← LOG_LEVEL",
                "env LOG_LEVEL",
                "envFrom (prefix APP_)",
                "projected volume all → all keys",
                "volume config → a.yaml"
            ]
        );
        let job = groups.iter().find(|g| g.kind == "Job").unwrap();
        assert_eq!(job.uses, ["envFrom"]);
        let secret = used_by(pods.iter(), Source::Secret, "db");
        assert_eq!(secret[0].uses, ["env PW ← password"]);
        let projected = used_by(pods.iter(), Source::Secret, "app");
        assert_eq!(projected[0].uses, ["projected volume all → all keys"]);
    }
}
