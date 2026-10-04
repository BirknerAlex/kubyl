//! Reading and writing kubeconfig YAML without losing comments or key order.
//!
//! [`write`] compares the text a file had when it was loaded with the edited document and
//! changes only what changed: scalars are replaced in place (keeping their quote style and the
//! comment after them), keys and list items are removed or appended at the indentation around
//! them, flow collections (`{ cluster: a, user: b }`) are rewritten in flow style. The result
//! is parsed again and must equal the edited document; when it doesn't (or the edit needs
//! something this module doesn't do in place), the whole file is rendered from the document
//! and [`Written::lost_comments`] lists the comments that don't survive. The save preview shows
//! them.
//!
//! Scalars are quoted whenever a YAML 1.1 parser could read them as anything but a string:
//! kubectl and client-go read kubeconfigs with `sigs.k8s.io/yaml` (go-yaml v2), where `yes`,
//! `on`, `0123` or `1:20` aren't strings.

use std::collections::HashMap;
use std::ops::Range;

use kubyl_yaml::parse::{self, Node, NodeValue};
use serde_json::{Map, Value};

/// Parses a kubeconfig. An empty file is an empty document.
pub fn load(text: &str) -> Result<Value, String> {
    let parsed = parse::parse(text);
    if let Some(err) = &parsed.error {
        let line = parse::line_of(text, err.range.start) + 1;
        return Err(format!("line {line}: {}", err.message));
    }
    let roots: Vec<&Node> = parsed.roots().collect();
    match roots.as_slice() {
        [] => Ok(Value::Object(Map::new())),
        [root] => match root.value {
            NodeValue::Map(_) => Ok(root.to_json()),
            _ => Err("not a kubeconfig: the file isn't a YAML mapping".into()),
        },
        _ => Err("not a kubeconfig: the file has more than one YAML document".into()),
    }
}

/// The result of [`write`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub text: String,
    /// The old text was edited in place (`false`: rendered from scratch).
    pub in_place: bool,
    /// Comments of the old text that the new one doesn't have (trimmed, without `#`).
    pub lost_comments: Vec<String>,
}

/// The text for `new`, edited from `old` where possible. See the module docs.
pub fn write(old: &str, new: &Value) -> Written {
    let in_place = write_in_place(old, new);
    let (text, in_place) = match in_place {
        Some(text) if load(&text).as_ref() == Ok(new) => (text, true),
        Some(_) => {
            tracing::warn!("an in-place kubeconfig edit didn't round-trip; rendering the file");
            (render(new), false)
        }
        None => (render(new), false),
    };
    let lost_comments = lost(&comments(old), &comments(&text));
    Written {
        text,
        in_place,
        lost_comments,
    }
}

fn write_in_place(old: &str, new: &Value) -> Option<String> {
    if !new.is_object() {
        return None;
    }
    let parsed = parse::parse(old);
    if parsed.error.is_some() || parsed.docs.len() > 1 {
        return None;
    }
    let Some(root) = parsed.roots().next() else {
        // An empty file: nothing to keep.
        return None;
    };
    if !matches!(root.value, NodeValue::Map(_)) || is_flow(old, root) {
        return None;
    }
    let mut writer = Writer {
        text: old,
        edits: Vec::new(),
    };
    writer.node(root, new, Slot::Root).ok()?;
    let mut edits = writer.edits;
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    for pair in edits.windows(2) {
        if pair[0].range.end > pair[1].range.start {
            return None;
        }
    }
    let crlf = old.contains("\r\n");
    let mut out = old.to_string();
    for edit in edits.into_iter().rev() {
        let text = if crlf {
            edit.text.replace('\n', "\r\n")
        } else {
            edit.text
        };
        out.replace_range(edit.range, &text);
    }
    Some(out)
}

// ----- Rendering from scratch -----

/// A whole document in block style, keys in the document's order (kubectl's layout: list items
/// at the indentation of their key).
pub fn render(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "{}\n".into(),
        Value::Object(map) => {
            let mut out = String::new();
            render_map(map, 0, &mut out);
            out
        }
        other => format!("{}\n", scalar(other, Context::Block, None)),
    }
}

/// Appends `key: value` lines at `indent`.
fn render_map(map: &Map<String, Value>, indent: usize, out: &mut String) {
    for (key, value) in map {
        render_entry(key, value, indent, out);
    }
}

fn render_entry(key: &str, value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    let key = scalar(&Value::String(key.to_string()), Context::Key, None);
    match value {
        Value::Object(map) if !map.is_empty() => {
            out.push_str(&format!("{pad}{key}:\n"));
            render_map(map, indent + 2, out);
        }
        Value::Array(items) if !items.is_empty() => {
            out.push_str(&format!("{pad}{key}:\n"));
            for item in items {
                render_item(item, indent, out);
            }
        }
        other => out.push_str(&format!("{pad}{key}: {}\n", inline(other))),
    }
}

/// Appends `- item` at `dash` (the column of the `-`).
fn render_item(value: &Value, dash: usize, out: &mut String) {
    let pad = " ".repeat(dash);
    match value {
        Value::Object(map) if !map.is_empty() => {
            let mut body = String::new();
            render_map(map, dash + 2, &mut body);
            // The first key goes on the dash line.
            out.push_str(&pad);
            out.push_str("- ");
            out.push_str(&body[dash + 2..]);
        }
        Value::Array(items) if !items.is_empty() => {
            let mut body = String::new();
            for item in items {
                render_item(item, dash + 2, &mut body);
            }
            out.push_str(&pad);
            out.push_str("- ");
            out.push_str(&body[dash + 2..]);
        }
        other => out.push_str(&format!("{pad}- {}\n", inline(other))),
    }
}

/// A value on one line: scalars, `{}` and `[]`.
fn inline(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Array(items) if items.is_empty() => "[]".into(),
        Value::Object(_) | Value::Array(_) => flow(value, false),
        other => scalar(other, Context::Block, None),
    }
}

/// `{ a: b, c: [d, e] }` (`spaced`: spaces inside braces).
fn flow(value: &Value, spaced: bool) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Array(items) if items.is_empty() => "[]".into(),
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}: {}",
                        scalar(&Value::String(k.clone()), Context::Flow, None),
                        flow(v, spaced)
                    )
                })
                .collect();
            if spaced {
                format!("{{ {} }}", inner.join(", "))
            } else {
                format!("{{{}}}", inner.join(", "))
            }
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(|v| flow(v, spaced)).collect();
            format!("[{}]", inner.join(", "))
        }
        other => scalar(other, Context::Flow, None),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    Block,
    Flow,
    Key,
}

/// How an existing scalar was written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    Plain,
    Single,
    Double,
}

/// A scalar as YAML text. `prefer`: the style of the scalar it replaces.
fn scalar(value: &Value, context: Context, prefer: Option<Quote>) -> String {
    let text = match value {
        Value::Null => return "null".into(),
        Value::Bool(b) => return b.to_string(),
        Value::Number(n) => return n.to_string(),
        Value::String(s) => s.as_str(),
        // Not scalars; callers don't pass them.
        other => return flow(other, false),
    };
    let single_ok = !text.contains('\n') && text.chars().all(|c| !c.is_control() || c == '\t');
    match prefer {
        Some(Quote::Single) if single_ok => return format!("'{}'", text.replace('\'', "''")),
        Some(Quote::Double) => return double_quoted(text),
        _ => {}
    }
    if plain_ok(text, context) {
        text.to_string()
    } else {
        double_quoted(text)
    }
}

fn double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether `text` reads back as the same string when written plain, for YAML 1.2 and for
/// YAML 1.1 (go-yaml v2, which kubectl uses).
fn plain_ok(text: &str, context: Context) -> bool {
    let Some(first) = text.chars().next() else {
        return false;
    };
    if text.trim() != text || text.contains(['\n', '\r', '\t']) {
        return false;
    }
    if text.chars().any(|c| c.is_control()) {
        return false;
    }
    // Indicators that start something else.
    if matches!(
        first,
        '?' | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    ) {
        return false;
    }
    // `-` starts a list item unless something other than a space follows; `-1` is a number.
    if first == '-' {
        let second = text.chars().nth(1);
        if second.is_none_or(|c| c == ' ' || c.is_ascii_digit() || c == '.') {
            return false;
        }
    }
    if text.contains(": ") || text.contains(" #") || text.ends_with(':') {
        return false;
    }
    if context != Context::Block && text.contains([',', '[', ']', '{', '}']) {
        return false;
    }
    if context == Context::Key && text.contains(':') {
        return false;
    }
    !special_1_1(text) && parse::resolve_plain(text) == Value::String(text.to_string())
}

/// Plain scalars that YAML 1.1 resolves to something other than a string.
fn special_1_1(text: &str) -> bool {
    const WORDS: &[&str] = &[
        "y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO", "true", "True", "TRUE", "false",
        "False", "FALSE", "on", "On", "ON", "off", "Off", "OFF", "null", "Null", "NULL", "~", "=",
        "<<", ".inf", ".Inf", ".INF", "-.inf", "+.inf", ".nan", ".NaN", ".NAN",
    ];
    if WORDS.contains(&text) {
        return true;
    }
    // Numbers in any 1.1 notation (0o17, 0x1F, 0b101, 1_000, 1:20, .5, 1e3) and timestamps
    // (2001-12-14): anything that starts like a number is quoted.
    let rest = text.strip_prefix(['+', '-']).unwrap_or(text);
    rest.starts_with(|c: char| c.is_ascii_digit())
        || (rest.starts_with('.') && rest[1..].starts_with(|c: char| c.is_ascii_digit()))
}

// ----- Editing in place -----

struct Edit {
    range: Range<usize>,
    text: String,
}

/// Where a node sits.
#[derive(Clone, Copy)]
enum Slot<'a> {
    Root,
    /// The value of `key:` whose key starts at column `indent`.
    Entry {
        key: &'a Node,
        indent: usize,
    },
    /// A list item whose `-` is at `dash`.
    Item {
        dash: usize,
    },
}

struct Writer<'a> {
    text: &'a str,
    edits: Vec<Edit>,
}

/// Not done in place; the caller renders the file.
struct Unsupported;

type Step = Result<(), Unsupported>;

impl<'a> Writer<'a> {
    fn edit(&mut self, range: Range<usize>, text: impl Into<String>) {
        self.edits.push(Edit {
            range,
            text: text.into(),
        });
    }

    fn node(&mut self, node: &'a Node, new: &Value, slot: Slot<'a>) -> Step {
        if node.to_json() == *new {
            return Ok(());
        }
        match (&node.value, new) {
            (NodeValue::Scalar(_), Value::Object(_) | Value::Array(_)) => {
                self.replace_value(node, new, slot)
            }
            (NodeValue::Scalar(_), _) => self.scalar(node, new, slot),
            (NodeValue::Map(_) | NodeValue::Seq(_), _) if is_flow(self.text, node) => {
                self.flow(node, new, slot)
            }
            (NodeValue::Map(entries), Value::Object(map)) if !map.is_empty() => {
                self.map(entries, map, matches!(slot, Slot::Root))
            }
            (NodeValue::Seq(items), Value::Array(array)) if !array.is_empty() => {
                self.seq(items, array)
            }
            _ => self.replace_value(node, new, slot),
        }
    }

    fn scalar(&mut self, node: &'a Node, new: &Value, slot: Slot<'a>) -> Step {
        let start = node.span.start;
        // A block scalar's span starts at its content, after the `|`/`>` line.
        if node
            .as_scalar()
            .is_some_and(|s| s.style == parse::Style::Block)
        {
            return self.replace_value(node, new, slot);
        }
        let quote = match self.text.as_bytes().get(start) {
            Some(b'\'') => Quote::Single,
            Some(b'"') => Quote::Double,
            _ => Quote::Plain,
        };
        if node.span.is_empty() {
            // `key:` with no value.
            let Slot::Entry { key, .. } = slot else {
                return Err(Unsupported);
            };
            let colon = self.colon_after(key)?;
            self.edit(
                colon + 1..colon + 1,
                format!(" {}", scalar(new, Context::Block, None)),
            );
            return Ok(());
        }
        let prefer = (quote != Quote::Plain).then_some(quote);
        // Scalars inside flow collections never get here: a changed flow is rewritten whole.
        let text = scalar(new, Context::Block, prefer);
        self.edit(node.span.clone(), text);
        Ok(())
    }

    /// A block mapping. New keys go after the last entry (at the end of the file for the
    /// top level).
    fn map(&mut self, entries: &'a [parse::Entry], new: &Map<String, Value>, root: bool) -> Step {
        let mut seen: HashMap<&str, &parse::Entry> = HashMap::new();
        for entry in entries {
            if !matches!(entry.key.value, NodeValue::Scalar(_)) || is_flow(self.text, &entry.key) {
                return Err(Unsupported);
            }
            if seen.insert(entry.key_str(), entry).is_some() {
                // Duplicate keys: leave the file to a full render.
                return Err(Unsupported);
            }
        }
        let indent = column(
            self.text,
            entries.first().ok_or(Unsupported)?.key.span.start,
        );
        // Keys on the line of a list item's `-` can't be removed line-wise.
        let first_key_on_dash = self.dash_before(&entries[0].key).is_some();
        for (ix, entry) in entries.iter().enumerate() {
            match new.get(entry.key_str()) {
                None => {
                    if ix == 0 && first_key_on_dash {
                        return Err(Unsupported);
                    }
                    let range = self.entry_lines(entry, ix > 0)?;
                    self.edit(range, "");
                }
                Some(value) => self.node(
                    &entry.value,
                    value,
                    Slot::Entry {
                        key: &entry.key,
                        indent,
                    },
                )?,
            }
        }
        let added: Vec<(&String, &Value)> = new
            .iter()
            .filter(|(k, _)| !seen.contains_key(k.as_str()))
            .collect();
        if !added.is_empty() {
            let last = entries.last().expect("checked non-empty");
            let at = if root {
                self.text.len()
            } else {
                self.after(value_end(&last.value).max(last.key.span.end), indent)
            };
            let mut text = String::new();
            for (key, value) in added {
                render_entry(key, value, indent, &mut text);
            }
            self.insert_lines(at, text);
        }
        Ok(())
    }

    /// A block sequence, items aligned with [`align`].
    fn seq(&mut self, items: &'a [Node], new: &[Value]) -> Step {
        let first = items.first().ok_or(Unsupported)?;
        let dash = self.dash_before(first).ok_or(Unsupported)?;
        let dash_col = column(self.text, dash);
        let pairs = align(items, new);
        // Old items without a partner are removed; new ones without a partner are inserted
        // after the previous old item that stays (or before the first item).
        let mut previous_kept: Option<&Node> = None;
        let mut pending: Vec<&Value> = Vec::new();
        let flush = |writer: &mut Self, previous: Option<&Node>, pending: &mut Vec<&Value>| {
            if pending.is_empty() {
                return Ok(());
            }
            let mut text = String::new();
            for value in pending.drain(..) {
                render_item(value, dash_col, &mut text);
            }
            let at = match previous {
                Some(prev) => writer.after(value_end(prev), dash_col),
                None => parse::line_range(writer.text, dash).start,
            };
            writer.insert_lines(at, text);
            Ok::<(), Unsupported>(())
        };
        for pair in &pairs {
            match pair {
                Pair::Both(old, new_ix) => {
                    flush(self, previous_kept, &mut pending)?;
                    let old_node = &items[*old];
                    let item_dash = self.dash_before(old_node).ok_or(Unsupported)?;
                    self.node(old_node, &new[*new_ix], Slot::Item { dash: item_dash })?;
                    previous_kept = Some(old_node);
                }
                Pair::Old(old) => {
                    let old_node = &items[*old];
                    let range = self.item_lines(old_node, *old > 0)?;
                    self.edit(range, "");
                }
                Pair::New(new_ix) => pending.push(&new[*new_ix]),
            }
        }
        // With no old item kept, the new ones take the place of the first.
        flush(self, previous_kept, &mut pending)?;
        Ok(())
    }

    /// A flow collection that changed: rewritten in flow style when it stays short. An empty
    /// one (`contexts: []`) that gets nested entries becomes a block.
    fn flow(&mut self, node: &'a Node, new: &Value, slot: Slot<'a>) -> Step {
        let end = self.flow_end(node)?;
        let was_empty = match &node.value {
            NodeValue::Map(entries) => entries.is_empty(),
            NodeValue::Seq(items) => items.is_empty(),
            NodeValue::Scalar(_) => false,
        };
        let nested = match new {
            Value::Object(map) => map.values().any(|v| v.is_object() || v.is_array()),
            Value::Array(items) => items.iter().any(|v| v.is_object() || v.is_array()),
            _ => true,
        };
        if was_empty && nested {
            return self.replace_value(node, new, slot);
        }
        let spaced = self.text.as_bytes().get(node.span.start + 1) == Some(&b' ');
        let text = match new {
            Value::Object(_) | Value::Array(_) => flow(new, spaced),
            other => scalar(other, Context::Block, None),
        };
        let line = parse::line_range(self.text, node.span.start);
        let multi_line = self.text[node.span.start..end].contains('\n');
        if !multi_line && (line.len() + text.len()).saturating_sub(end - node.span.start) <= 120 {
            self.edit(node.span.start..end, text);
            return Ok(());
        }
        self.replace_value(node, new, slot)
    }

    /// The end of a flow collection, after its closing bracket.
    fn flow_end(&self, node: &Node) -> Result<usize, Unsupported> {
        matching_bracket(self.text, node.span.start).ok_or(Unsupported)
    }

    /// Replaces a whole value (a type change, a block scalar, a collection that became empty).
    fn replace_value(&mut self, node: &'a Node, new: &Value, slot: Slot<'a>) -> Step {
        let end = if is_flow(self.text, node) {
            self.flow_end(node)?
        } else {
            // A block scalar's span can include its last line break; keep the break.
            let mut end = value_end(node);
            while end > node.span.start && matches!(self.text.as_bytes()[end - 1], b'\n' | b'\r') {
                end -= 1;
            }
            end
        };
        match slot {
            Slot::Root => Err(Unsupported),
            Slot::Entry { key, indent } => {
                let colon = self.colon_after(key)?;
                let text = match new {
                    Value::Object(map) if !map.is_empty() => {
                        let mut out = String::from("\n");
                        render_map(map, indent + 2, &mut out);
                        out.pop();
                        out
                    }
                    Value::Array(items) if !items.is_empty() => {
                        let mut out = String::from("\n");
                        for item in items {
                            render_item(item, indent, &mut out);
                        }
                        out.pop();
                        out
                    }
                    other => format!(" {}", inline(other)),
                };
                self.edit(colon + 1..end.max(colon + 1), text);
                Ok(())
            }
            Slot::Item { dash } => {
                let col = column(self.text, dash);
                let mut out = String::new();
                render_item(new, col, &mut out);
                out.pop();
                // `render_item` starts with the indentation and `- `.
                let text = out[col + 2..].to_string();
                self.edit(node.span.start..end, text);
                Ok(())
            }
        }
    }

    /// The `:` after a key.
    fn colon_after(&self, key: &Node) -> Result<usize, Unsupported> {
        let mut at = key.span.end;
        // Quoted keys end before their closing quote in some parsers.
        let bytes = self.text.as_bytes();
        while at < bytes.len() && matches!(bytes[at], b' ' | b'"' | b'\'') {
            at += 1;
        }
        (bytes.get(at) == Some(&b':'))
            .then_some(at)
            .ok_or(Unsupported)
    }

    /// The offset of the `-` in front of a list item on the same line, if the item has one.
    fn dash_before(&self, node: &Node) -> Option<usize> {
        let line = parse::line_range(self.text, node.span.start);
        let before = &self.text[line.start..node.span.start];
        let trimmed = before.trim_end_matches(' ');
        if !trimmed.ends_with('-') {
            return None;
        }
        let dash = line.start + trimmed.len() - 1;
        // Only `  - ` (one marker, nothing else before it).
        self.text[line.start..dash]
            .chars()
            .all(|c| c == ' ')
            .then_some(dash)
    }

    /// The lines of a map entry, with comment lines before it (when `leading`) and deeper
    /// comment lines after it.
    fn entry_lines(
        &self,
        entry: &parse::Entry,
        leading: bool,
    ) -> Result<Range<usize>, Unsupported> {
        let key_line = parse::line_range(self.text, entry.key.span.start);
        if !self.text[key_line.start..entry.key.span.start]
            .chars()
            .all(|c| c == ' ')
        {
            return Err(Unsupported);
        }
        let indent = entry.key.span.start - key_line.start;
        let end = value_end(&entry.value).max(entry.key.span.end);
        let start = if leading {
            self.comments_above(key_line.start, indent)
        } else {
            key_line.start
        };
        Ok(start..self.after(end, indent))
    }

    /// The lines of a list item, like [`Self::entry_lines`].
    fn item_lines(&self, item: &Node, leading: bool) -> Result<Range<usize>, Unsupported> {
        let dash = self.dash_before(item).ok_or(Unsupported)?;
        let line = parse::line_range(self.text, dash);
        let col = dash - line.start;
        let start = if leading {
            self.comments_above(line.start, col)
        } else {
            line.start
        };
        Ok(start..self.after(value_end(item), col))
    }

    /// The start of the comment-only lines directly above `line_start` that are indented at
    /// least `indent` (no blank line between).
    fn comments_above(&self, line_start: usize, indent: usize) -> usize {
        let mut start = line_start;
        while start > 0 {
            let prev = parse::line_range(self.text, start - 1);
            let content = &self.text[prev.clone()];
            let trimmed = content.trim_start_matches(' ');
            let col = content.len() - trimmed.len();
            if trimmed.starts_with('#') && col >= indent {
                start = prev.start;
            } else {
                break;
            }
        }
        start
    }

    /// The start of the line after `offset`'s line, skipping comment-only lines indented more
    /// than `indent` (they belong to what ends at `offset`).
    fn after(&self, offset: usize, indent: usize) -> usize {
        let mut at = line_after(self.text, offset);
        while at < self.text.len() {
            let line = parse::line_range(self.text, at);
            let content = &self.text[line.clone()];
            let trimmed = content.trim_start_matches(' ');
            let col = content.len() - trimmed.len();
            if trimmed.starts_with('#') && col > indent {
                at = line_after(self.text, line.start);
            } else {
                break;
            }
        }
        at
    }

    /// Inserts whole lines at a line start (or the end of the text).
    fn insert_lines(&mut self, at: usize, text: String) {
        let needs_break = at > 0 && at == self.text.len() && !self.text.ends_with('\n');
        let text = if needs_break {
            format!("\n{text}")
        } else {
            text
        };
        self.edit(at..at, text);
    }
}

/// Where a value's text ends. Block scalars include their lines; for collections the parser
/// already ends the span at the last value.
fn value_end(node: &Node) -> usize {
    node.span.end
}

/// The start of the line after the one containing `offset` (or the end of the text).
fn line_after(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    // A span that ends right after a line break (block scalars) ends that line already.
    if offset > 0 && text.as_bytes()[offset - 1] == b'\n' {
        return offset;
    }
    text[offset..]
        .find('\n')
        .map_or(text.len(), |i| offset + i + 1)
}

fn column(text: &str, offset: usize) -> usize {
    offset - parse::line_range(text, offset).start
}

/// The offset after the bracket that closes the one at `open`, skipping quoted strings.
fn matching_bracket(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if !matches!(bytes.get(open), Some(b'{' | b'[')) {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    i += 1;
                }
            }
            b'#' if i > 0 && matches!(bytes[i - 1], b' ' | b'\t') => {
                // A comment inside a multi-line flow collection.
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_flow(text: &str, node: &Node) -> bool {
    matches!(node.value, NodeValue::Map(_) | NodeValue::Seq(_))
        && matches!(text.as_bytes().get(node.span.start), Some(b'{' | b'['))
}

// ----- Aligning list items -----

enum Pair {
    Both(usize, usize),
    Old(usize),
    New(usize),
}

/// Pairs old and new list items: by `name` (kubeconfig entries), then equal values, and what's
/// left between two pairs by position (a renamed or edited entry). The result is in order.
fn align(old: &[Node], new: &[Value]) -> Vec<Pair> {
    let old_json: Vec<Value> = old.iter().map(Node::to_json).collect();
    let identity = |v: &Value| -> String {
        match v.get("name").and_then(Value::as_str) {
            Some(name) => format!("name:{name}"),
            None => format!("value:{v}"),
        }
    };
    let old_ids: Vec<String> = old_json.iter().map(identity).collect();
    let new_ids: Vec<String> = new.iter().map(identity).collect();
    let ops = similar::capture_diff_slices(similar::Algorithm::Myers, &old_ids, &new_ids);
    let mut pairs = Vec::new();
    let mut gap_old: Vec<usize> = Vec::new();
    let mut gap_new: Vec<usize> = Vec::new();
    let flush = |pairs: &mut Vec<Pair>, gap_old: &mut Vec<usize>, gap_new: &mut Vec<usize>| {
        let n = gap_old.len().min(gap_new.len());
        for i in 0..n {
            pairs.push(Pair::Both(gap_old[i], gap_new[i]));
        }
        for &o in &gap_old[n..] {
            pairs.push(Pair::Old(o));
        }
        for &n_ix in &gap_new[n..] {
            pairs.push(Pair::New(n_ix));
        }
        gap_old.clear();
        gap_new.clear();
    };
    for op in ops {
        match op {
            similar::DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                flush(&mut pairs, &mut gap_old, &mut gap_new);
                for i in 0..len {
                    pairs.push(Pair::Both(old_index + i, new_index + i));
                }
            }
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => gap_old.extend(old_index..old_index + old_len),
            similar::DiffOp::Insert {
                new_index, new_len, ..
            } => gap_new.extend(new_index..new_index + new_len),
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                gap_old.extend(old_index..old_index + old_len);
                gap_new.extend(new_index..new_index + new_len);
            }
        }
    }
    flush(&mut pairs, &mut gap_old, &mut gap_new);
    pairs
}

// ----- Comments -----

/// The comments in `text` (trimmed, without `#`), in order.
pub fn comments(text: &str) -> Vec<String> {
    let parsed = parse::parse(text);
    let mut scalars: Vec<Range<usize>> = Vec::new();
    for root in parsed.roots() {
        collect_scalar_spans(root, &mut scalars);
    }
    scalars.sort_by_key(|r| r.start);
    let inside = |offset: usize| {
        scalars
            .iter()
            .any(|r| r.start <= offset && offset < r.end && r.start != r.end)
    };
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#'
            && (i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'\n'))
            && !inside(i)
        {
            let end = text[i..].find('\n').map_or(text.len(), |n| i + n);
            out.push(text[i + 1..end].trim().to_string());
            i = end;
        }
        i += 1;
    }
    out
}

fn collect_scalar_spans(node: &Node, out: &mut Vec<Range<usize>>) {
    match &node.value {
        NodeValue::Scalar(_) => out.push(node.span.clone()),
        NodeValue::Map(entries) => {
            for entry in entries {
                collect_scalar_spans(&entry.key, out);
                collect_scalar_spans(&entry.value, out);
            }
        }
        NodeValue::Seq(items) => {
            for item in items {
                collect_scalar_spans(item, out);
            }
        }
    }
}

/// `old` minus `new`, as multisets.
fn lost(old: &[String], new: &[String]) -> Vec<String> {
    let mut remaining: HashMap<&str, usize> = HashMap::new();
    for c in new {
        *remaining.entry(c.as_str()).or_default() += 1;
    }
    old.iter()
        .filter(|c| match remaining.get_mut(c.as_str()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) const COMMENTED: &str = r#"# Managed by hand. Don't let tools reorder this!
apiVersion: v1
kind: Config
preferences: {}
current-context: kind-dev # the one I use most
clusters:
  # local kind
  - name: kind-dev
    cluster:
      server: https://127.0.0.1:52341 # port changes on restart
      certificate-authority-data: Zm9v
  - cluster:
      server: https://prod.example.com
      insecure-skip-tls-verify: true
    name: prod
contexts:
  - name: kind-dev
    context: { cluster: kind-dev, user: kind-dev, namespace: payments }
  - name: prod
    context:
      cluster: prod
      user: aws # EKS
users:
  - name: kind-dev
    user:
      client-certificate-data: Zm9v
      client-key-data: Zm9v
  - name: aws
    user:
      exec:
        apiVersion: client.authentication.k8s.io/v1beta1
        command: aws
        args: [eks, get-token, --cluster-name, prod]
        env:
          - name: AWS_PROFILE
            value: prod
# trailing comment
"#;

    /// Applies `f` to the loaded document, writes it, and checks the result.
    fn edit(f: impl FnOnce(&mut Value)) -> Written {
        let mut doc = load(COMMENTED).unwrap();
        f(&mut doc);
        let written = write(COMMENTED, &doc);
        assert_eq!(load(&written.text).unwrap(), doc, "{}", written.text);
        written
    }

    fn changed_lines(written: &Written) -> (Vec<String>, Vec<String>) {
        let old: Vec<&str> = COMMENTED.lines().collect();
        let new: Vec<&str> = written.text.lines().collect();
        let diff = similar::capture_diff_slices(similar::Algorithm::Myers, &old, &new);
        let mut removed = Vec::new();
        let mut added = Vec::new();
        for op in diff {
            match op {
                similar::DiffOp::Equal { .. } => {}
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                } => removed.extend(
                    old[old_index..old_index + old_len]
                        .iter()
                        .map(|s| s.to_string()),
                ),
                similar::DiffOp::Insert {
                    new_index, new_len, ..
                } => added.extend(
                    new[new_index..new_index + new_len]
                        .iter()
                        .map(|s| s.to_string()),
                ),
                similar::DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    removed.extend(
                        old[old_index..old_index + old_len]
                            .iter()
                            .map(|s| s.to_string()),
                    );
                    added.extend(
                        new[new_index..new_index + new_len]
                            .iter()
                            .map(|s| s.to_string()),
                    );
                }
            }
        }
        (removed, added)
    }

    fn contexts(doc: &mut Value) -> &mut Vec<Value> {
        doc["contexts"].as_array_mut().unwrap()
    }

    #[test]
    fn loads_documents() {
        let doc = load(COMMENTED).unwrap();
        assert_eq!(doc["current-context"], "kind-dev");
        assert_eq!(doc["contexts"][0]["context"]["namespace"], "payments");
        assert_eq!(load("").unwrap(), json!({}));
        assert!(load("- a").is_err());
        assert!(load("a: [").unwrap_err().starts_with("line "));
        assert!(load("a: 1\n---\nb: 2\n").is_err());
    }

    #[test]
    fn replaces_a_value_in_a_flow_mapping() {
        let w = edit(|d| contexts(d)[0]["context"]["namespace"] = json!("orders"));
        assert!(w.in_place);
        assert_eq!(
            changed_lines(&w),
            (
                vec![
                    "    context: { cluster: kind-dev, user: kind-dev, namespace: payments }"
                        .into()
                ],
                vec![
                    "    context: { cluster: kind-dev, user: kind-dev, namespace: orders }".into()
                ]
            )
        );
        assert!(w.lost_comments.is_empty());
    }

    #[test]
    fn adds_a_key_to_a_block_mapping() {
        let w = edit(|d| {
            contexts(d)[1]["context"]
                .as_object_mut()
                .unwrap()
                .insert("namespace".into(), json!("billing"));
        });
        assert!(w.in_place);
        assert_eq!(
            changed_lines(&w),
            (vec![], vec!["      namespace: billing".into()])
        );
        assert!(
            w.text
                .contains("      user: aws # EKS\n      namespace: billing\n")
        );
    }

    #[test]
    fn keeps_trailing_comments_and_quote_styles() {
        let w = edit(|d| d["clusters"][0]["cluster"]["server"] = json!("https://127.0.0.1:6443"));
        assert_eq!(
            changed_lines(&w).1,
            ["      server: https://127.0.0.1:6443 # port changes on restart"]
        );
        let w = edit(|d| d["current-context"] = json!("prod"));
        assert_eq!(
            changed_lines(&w).1,
            ["current-context: prod # the one I use most"]
        );
        let quoted = "a: 'x' # c\nb: \"y\"\n";
        let mut doc = load(quoted).unwrap();
        doc["a"] = json!("it's");
        doc["b"] = json!("z");
        assert_eq!(write(quoted, &doc).text, "a: 'it''s' # c\nb: \"z\"\n");
    }

    #[test]
    fn quotes_what_yaml_1_1_would_misread() {
        for (value, expected) in [
            ("yes", "\"yes\""),
            ("on", "\"on\""),
            ("0123", "\"0123\""),
            ("1:20", "\"1:20\""),
            ("1e3", "\"1e3\""),
            ("2001-12-14", "\"2001-12-14\""),
            ("~", "\"~\""),
            ("", "\"\""),
            ("a: b # c", "\"a: b # c\""),
            ("-", "\"-\""),
            (" padded", "\" padded\""),
            ("kind-dev", "kind-dev"),
            ("--cluster-name", "--cluster-name"),
            ("https://x:6443/p?q=1", "https://x:6443/p?q=1"),
            (
                "arn:aws:eks:eu-west-1:1:cluster/x",
                "arn:aws:eks:eu-west-1:1:cluster/x",
            ),
        ] {
            assert_eq!(
                scalar(&json!(value), Context::Block, None),
                expected,
                "{value}"
            );
        }
        assert_eq!(scalar(&json!("a,b"), Context::Flow, None), "\"a,b\"");
        assert_eq!(scalar(&json!("a,b"), Context::Block, None), "a,b");
        let w = edit(|d| contexts(d)[1]["context"]["user"] = json!("yes"));
        assert_eq!(changed_lines(&w).1, ["      user: \"yes\" # EKS"]);
    }

    #[test]
    fn removes_list_items_and_keeps_the_trailing_comment() {
        let w = edit(|d| {
            d["users"].as_array_mut().unwrap().remove(1);
        });
        assert!(w.in_place);
        assert!(
            w.text
                .ends_with("      client-key-data: Zm9v\n# trailing comment\n"),
            "{}",
            w.text
        );
        assert!(w.lost_comments.is_empty());

        let w = edit(|d| {
            d["users"].as_array_mut().unwrap().remove(0);
        });
        assert!(w.text.contains("users:\n  - name: aws\n"), "{}", w.text);
        // A comment above the first item stays (it may be about the whole list).
        let w = edit(|d| {
            d["clusters"].as_array_mut().unwrap().remove(0);
        });
        assert!(
            w.text.contains("clusters:\n  # local kind\n  - cluster:\n"),
            "{}",
            w.text
        );
    }

    #[test]
    fn removes_keys() {
        let w = edit(|d| {
            d["clusters"][1]["cluster"]
                .as_object_mut()
                .unwrap()
                .remove("insecure-skip-tls-verify");
        });
        assert_eq!(
            changed_lines(&w),
            (vec!["      insecure-skip-tls-verify: true".into()], vec![])
        );
    }

    #[test]
    fn appends_list_items_at_the_list_indentation() {
        let w = edit(|d| {
            d["clusters"].as_array_mut().unwrap().push(json!({
                "name": "new",
                "cluster": {"server": "https://new.example.com", "certificate-authority-data": "QUJD"}
            }));
        });
        assert!(w.in_place);
        assert!(
            w.text.contains(
                "    name: prod\n  - name: new\n    cluster:\n      server: https://new.example.com\n      certificate-authority-data: QUJD\ncontexts:\n"
            ),
            "{}",
            w.text
        );
    }

    #[test]
    fn renames_entries_in_place() {
        let w = edit(|d| {
            contexts(d)[1]["name"] = json!("prod-eu");
            d["current-context"] = json!("prod-eu");
        });
        assert!(w.in_place);
        assert_eq!(
            changed_lines(&w).1,
            [
                "current-context: prod-eu # the one I use most",
                "  - name: prod-eu"
            ]
        );
    }

    #[test]
    fn rewrites_flow_sequences_and_switches_value_types() {
        let w = edit(|d| {
            d["users"][1]["user"]["exec"]["args"] = json!([
                "eks",
                "get-token",
                "--cluster-name",
                "prod2",
                "--region",
                "eu-west-1"
            ]);
        });
        assert_eq!(
            changed_lines(&w).1,
            ["        args: [eks, get-token, --cluster-name, prod2, --region, eu-west-1]"]
        );
        // Switching a client-certificate user to a token.
        let w = edit(|d| d["users"][0]["user"] = json!({"token": "abc"}));
        assert!(w.in_place);
        assert!(
            w.text
                .contains("  - name: kind-dev\n    user:\n      token: abc\n  - name: aws"),
            "{}",
            w.text
        );
        // A collection that becomes empty.
        let w = edit(|d| d["users"][1]["user"]["exec"]["env"] = json!([]));
        assert!(
            w.text.contains("        env: []\n# trailing comment"),
            "{}",
            w.text
        );
        let w = edit(|d| d["preferences"] = json!({"colors": true}));
        assert!(
            w.text.contains("preferences: {colors: true}\n"),
            "{}",
            w.text
        );
    }

    #[test]
    fn adds_top_level_keys_and_fills_empty_values() {
        let w = edit(|d| {
            d.as_object_mut().unwrap().insert(
                "extensions".into(),
                json!([{"name": "a", "extension": {"x": 1}}]),
            );
        });
        assert!(
            w.text
                .ends_with("# trailing comment\nextensions:\n- name: a\n  extension:\n    x: 1\n"),
            "{}",
            w.text
        );
        let text = "a:\nb: 1\n";
        let mut doc = load(text).unwrap();
        doc["a"] = json!("x");
        assert_eq!(write(text, &doc).text, "a: x\nb: 1\n");
    }

    #[test]
    fn kubectl_layout_lists_at_the_key_indentation() {
        let text = "apiVersion: v1\nclusters:\n- cluster:\n    server: https://a\n  name: a\ncontexts: []\n";
        let mut doc = load(text).unwrap();
        doc["clusters"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "b", "cluster": {"server": "https://b"}}));
        doc["contexts"] = json!([{"name": "a", "context": {"cluster": "a"}}]);
        let w = write(text, &doc);
        assert!(w.in_place);
        assert_eq!(
            w.text,
            "apiVersion: v1\nclusters:\n- cluster:\n    server: https://a\n  name: a\n- name: b\n  cluster:\n    server: https://b\ncontexts:\n- name: a\n  context:\n    cluster: a\n"
        );
    }

    #[test]
    fn falls_back_to_rendering_and_reports_lost_comments() {
        // A flow-style root can't be edited line-wise.
        let text = "{apiVersion: v1, clusters: []} # flow\n";
        let mut doc = load(text).unwrap();
        doc["kind"] = json!("Config");
        let w = write(text, &doc);
        assert!(!w.in_place);
        assert_eq!(w.lost_comments, ["flow"]);
        assert_eq!(load(&w.text).unwrap(), doc);
    }

    #[test]
    fn renders_new_documents_like_kubectl() {
        let doc = json!({
            "apiVersion": "v1",
            "kind": "Config",
            "clusters": [{"name": "kind", "cluster": {"server": "https://127.0.0.1:6443"}}],
            "contexts": [],
            "current-context": "kind",
            "preferences": {},
        });
        assert_eq!(
            render(&doc),
            "apiVersion: v1\nkind: Config\nclusters:\n- name: kind\n  cluster:\n    server: https://127.0.0.1:6443\ncontexts: []\ncurrent-context: kind\npreferences: {}\n"
        );
    }

    #[test]
    fn finds_comments_outside_scalars() {
        assert_eq!(
            comments("a: 'x # not a comment' # yes\n# line\nb: c#d\n"),
            ["yes", "line"]
        );
        assert_eq!(
            lost(&["a".into(), "a".into(), "b".into()], &["a".into()]),
            ["a", "b"]
        );
    }

    const KUBECTL: &str = "apiVersion: v1\nclusters:\n- cluster:\n    certificate-authority-data: QUJD\n    server: https://127.0.0.1:52341\n  name: kind-a\n- cluster:\n    server: https://b.example.com\n    tls-server-name: b.internal\n  name: b\n- cluster:\n    insecure-skip-tls-verify: true\n    server: https://c.example.com:6443\n  name: c\ncontexts:\n- context:\n    cluster: kind-a\n    user: kind-a\n  name: kind-a\n- context:\n    cluster: b\n    namespace: team\n    user: b-user\n  name: b\n- context:\n    cluster: c\n    user: c-user\n  name: c\ncurrent-context: kind-a\nkind: Config\npreferences: {}\nusers:\n- name: kind-a\n  user:\n    client-certificate-data: QUJD\n    client-key-data: REVG\n- name: b-user\n  user:\n    exec:\n      apiVersion: client.authentication.k8s.io/v1beta1\n      args:\n      - eks\n      - get-token\n      - --cluster-name\n      - b\n      command: aws\n      env: null\n      provideClusterInfo: false\n- name: c-user\n  user:\n    token: abc.def.ghi\n";

    /// Every rename, removal and scalar change of every entry stays an in-place edit.
    #[test]
    fn every_entry_edit_is_in_place() {
        for text in [KUBECTL, COMMENTED] {
            let original = load(text).unwrap();
            for list in ["clusters", "contexts", "users"] {
                let len = original[list].as_array().unwrap().len();
                for ix in 0..len {
                    let mut renamed = original.clone();
                    renamed[list][ix]["name"] = json!(format!("renamed-{ix}"));
                    let w = write(text, &renamed);
                    assert!(
                        w.in_place && w.lost_comments.is_empty(),
                        "rename {list}[{ix}]:\n{}",
                        w.text
                    );
                    assert_eq!(load(&w.text).unwrap(), renamed);

                    let mut removed = original.clone();
                    removed[list].as_array_mut().unwrap().remove(ix);
                    let w = write(text, &removed);
                    assert!(w.in_place, "remove {list}[{ix}]:\n{}", w.text);
                    assert_eq!(load(&w.text).unwrap(), removed);

                    let mut appended = original.clone();
                    let copy = appended[list][ix].clone();
                    let mut copy = copy;
                    copy["name"] = json!("copy");
                    appended[list].as_array_mut().unwrap().push(copy);
                    let w = write(text, &appended);
                    assert!(
                        w.in_place && w.lost_comments.is_empty(),
                        "append {list}[{ix}]:\n{}",
                        w.text
                    );
                    assert_eq!(load(&w.text).unwrap(), appended);
                }
            }
            // Removing everything leaves empty lists.
            let mut empty = original.clone();
            for list in ["clusters", "contexts", "users"] {
                empty[list] = json!([]);
            }
            let w = write(text, &empty);
            assert!(w.in_place, "{}", w.text);
            assert_eq!(load(&w.text).unwrap(), empty);
        }
    }

    #[test]
    fn edits_block_sequences_of_scalars() {
        let mut doc = load(KUBECTL).unwrap();
        let args = doc["users"][1]["user"]["exec"]["args"]
            .as_array_mut()
            .unwrap();
        args[3] = json!("prod");
        args.remove(0);
        args.push(json!("--region"));
        args.push(json!("eu-west-1"));
        doc["users"][1]["user"]["exec"]["env"] = json!([{"name": "AWS_PROFILE", "value": "prod"}]);
        let w = write(KUBECTL, &doc);
        assert!(w.in_place);
        assert!(
            w.text.contains("      args:\n      - get-token\n      - --cluster-name\n      - prod\n      - --region\n      - eu-west-1\n      command: aws\n      env:\n      - name: AWS_PROFILE\n        value: prod\n"),
            "{}",
            w.text
        );
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let text = KUBECTL.replace('\n', "\r\n");
        let mut doc = load(&text).unwrap();
        doc["contexts"][0]["context"]["namespace"] = json!("payments");
        doc["clusters"].as_array_mut().unwrap().remove(2);
        let w = write(&text, &doc);
        assert!(w.in_place);
        assert!(!w.text.replace("\r\n", "").contains('\n'), "{:?}", w.text);
        assert_eq!(load(&w.text).unwrap(), doc);
    }

    #[test]
    fn replaces_block_scalars() {
        let text = "a: |\n  one\n  two\nb: x\n";
        let mut doc = load(text).unwrap();
        doc["a"] = json!("three");
        let w = write(text, &doc);
        assert_eq!(w.text, "a: three\nb: x\n");
        assert!(w.in_place);
    }
}
