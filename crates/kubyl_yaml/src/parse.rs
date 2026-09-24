//! A YAML tree with byte spans, built from `granit-parser` events (the saphyr parser fork that
//! `serde-saphyr` uses).
//!
//! The editor never re-serializes the user's buffer: diagnostics, hover, completion and targeted
//! edits (secret masking, managedFields) work on these spans, and [`Node::to_json`] turns a
//! document into the object that is sent to the API server.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;

use granit_parser::{Event, Parser, ScalarStyle, Span};
use serde_json::{Map, Number, Value};

/// A parsed YAML node and the bytes it covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub value: NodeValue,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeValue {
    Map(Vec<Entry>),
    Seq(Vec<Node>),
    Scalar(Scalar),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: Node,
    pub value: Node,
}

impl Entry {
    /// The key as text (`""` for non-scalar keys).
    pub fn key_str(&self) -> &str {
        match &self.key.value {
            NodeValue::Scalar(s) => &s.text,
            _ => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scalar {
    pub text: String,
    pub style: Style,
    /// The core-schema suffix of an explicit tag (`str`, `int`…), or the full custom tag.
    pub tag: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Plain,
    Quoted,
    Block,
}

/// One document of a (multi-document) YAML stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// `None` for an empty document.
    pub root: Option<Node>,
    /// The bytes of the document (from its first token to its end).
    pub span: Range<usize>,
}

/// A syntax error.
#[derive(Clone, Debug, PartialEq)]
pub struct SyntaxError {
    pub message: String,
    /// Byte range to underline (usually one character).
    pub range: Range<usize>,
}

/// Every document that parsed, and the first syntax error.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub docs: Vec<Document>,
    pub error: Option<SyntaxError>,
}

impl Parsed {
    /// Documents that aren't empty.
    pub fn roots(&self) -> impl Iterator<Item = &Node> {
        self.docs.iter().filter_map(|d| d.root.as_ref())
    }

    /// The document containing `offset` (or the last one before it).
    pub fn doc_at(&self, offset: usize) -> Option<&Document> {
        self.docs
            .iter()
            .rev()
            .find(|d| d.span.start <= offset)
            .or(self.docs.first())
    }
}

/// Parses `text`. On a syntax error, documents before it are kept.
pub fn parse(text: &str) -> Parsed {
    let offsets = CharOffsets::new(text);
    let mut builder = Builder {
        offsets: &offsets,
        stack: Vec::new(),
        anchors: HashMap::new(),
        docs: Vec::new(),
        doc_start: 0,
    };
    let mut parsed = Parsed::default();
    for next in Parser::new_from_str(text) {
        match next {
            Ok((event, span)) => builder.event(event, span),
            Err(err) => {
                let at = offsets.byte(err.marker().byte_offset(), err.marker().index());
                let end = text[at..].chars().next().map_or(at, |c| at + c.len_utf8());
                parsed.error = Some(SyntaxError {
                    message: syntax_message(&err.info()),
                    range: at..end.max(at),
                });
                break;
            }
        }
    }
    parsed.docs = builder.docs;
    parsed
}

fn syntax_message(info: &str) -> String {
    let first = info.lines().next().unwrap_or(info).trim();
    if first.is_empty() {
        "invalid YAML".into()
    } else {
        first.to_string()
    }
}

/// Converts character indexes to byte offsets for markers that lack a byte offset.
struct CharOffsets<'a> {
    text: &'a str,
    ascii: bool,
}

impl<'a> CharOffsets<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            ascii: text.is_ascii(),
        }
    }

    fn byte(&self, byte: Option<usize>, chars: usize) -> usize {
        if let Some(b) = byte {
            return b.min(self.text.len());
        }
        if self.ascii {
            return chars.min(self.text.len());
        }
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(i, _)| i)
    }

    fn range(&self, span: &Span) -> Range<usize> {
        let start = self.byte(span.start.byte_offset(), span.start.index());
        let end = self.byte(span.end.byte_offset(), span.end.index());
        start..end.max(start)
    }
}

enum Frame {
    Map {
        start: usize,
        anchor: usize,
        entries: Vec<Entry>,
        key: Option<Node>,
    },
    Seq {
        start: usize,
        anchor: usize,
        items: Vec<Node>,
    },
}

struct Builder<'a> {
    offsets: &'a CharOffsets<'a>,
    stack: Vec<Frame>,
    anchors: HashMap<usize, Node>,
    docs: Vec<Document>,
    doc_start: usize,
}

impl Builder<'_> {
    fn event(&mut self, event: Event<'_>, span: Span) {
        let range = self.offsets.range(&span);
        match event {
            Event::DocumentStart(..) => {
                self.doc_start = range.start;
                self.docs.push(Document {
                    root: None,
                    span: range.start..range.start,
                });
            }
            Event::DocumentEnd => {
                if let Some(doc) = self.docs.last_mut() {
                    // `---` at the end of a stream: an empty document, not a null.
                    if doc
                        .root
                        .as_ref()
                        .is_some_and(|r| r.span.is_empty() && matches!(r.as_str(), Some("" | "~")))
                    {
                        doc.root = None;
                    }
                    let end = doc.root.as_ref().map_or(range.end, |r| r.span.end);
                    doc.span = self.doc_start..end.max(range.start);
                }
            }
            Event::Scalar(text, style, anchor, tag) => {
                let node = Node {
                    value: NodeValue::Scalar(Scalar {
                        text: text.into_owned(),
                        style: match style {
                            ScalarStyle::Plain => Style::Plain,
                            ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted => Style::Quoted,
                            _ => Style::Block,
                        },
                        tag: tag.map(|t| {
                            t.core_suffix()
                                .map(String::from)
                                .unwrap_or_else(|| t.original())
                        }),
                    }),
                    span: range,
                };
                if anchor != 0 {
                    self.anchors.insert(anchor, node.clone());
                }
                self.push(node);
            }
            Event::Alias(id) => {
                let node = self.anchors.get(&id).cloned().map(|mut n| {
                    n.span = range.clone();
                    n
                });
                self.push(node.unwrap_or(Node {
                    value: NodeValue::Scalar(Scalar {
                        text: String::new(),
                        style: Style::Plain,
                        tag: None,
                    }),
                    span: range,
                }));
            }
            Event::MappingStart(_, anchor, _) => self.stack.push(Frame::Map {
                start: range.start,
                anchor,
                entries: Vec::new(),
                key: None,
            }),
            Event::SequenceStart(_, anchor, _) => self.stack.push(Frame::Seq {
                start: range.start,
                anchor,
                items: Vec::new(),
            }),
            Event::MappingEnd | Event::SequenceEnd => {
                let Some(frame) = self.stack.pop() else {
                    return;
                };
                let (node, anchor) = match frame {
                    Frame::Map {
                        start,
                        anchor,
                        entries,
                        ..
                    } => {
                        let end = entries.last().map_or(range.end, |e| e.value.span.end);
                        (
                            Node {
                                value: NodeValue::Map(entries),
                                span: start..end.max(start),
                            },
                            anchor,
                        )
                    }
                    Frame::Seq {
                        start,
                        anchor,
                        items,
                    } => {
                        let end = items.last().map_or(range.end, |i| i.span.end);
                        (
                            Node {
                                value: NodeValue::Seq(items),
                                span: start..end.max(start),
                            },
                            anchor,
                        )
                    }
                };
                if anchor != 0 {
                    self.anchors.insert(anchor, node.clone());
                }
                self.push(node);
            }
            _ => {}
        }
    }

    fn push(&mut self, node: Node) {
        match self.stack.last_mut() {
            Some(Frame::Map { entries, key, .. }) => match key.take() {
                Some(k) => entries.push(Entry {
                    key: k,
                    value: node,
                }),
                None => *key = Some(node),
            },
            Some(Frame::Seq { items, .. }) => items.push(node),
            None => {
                if let Some(doc) = self.docs.last_mut() {
                    doc.span.end = node.span.end;
                    doc.root = Some(node);
                }
            }
        }
    }
}

// ----- Paths -----

/// One step of a path into a document.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Seg {
    Key(String),
    Index(usize),
}

/// `spec.containers[0].image`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Path(pub Vec<Seg>);

impl Path {
    pub fn keys(keys: &[&str]) -> Self {
        Self(keys.iter().map(|k| Seg::Key(k.to_string())).collect())
    }

    /// Parses `spec.containers[0].image` (the field paths in API server errors). A leading `.`
    /// (as in SSA conflict messages) is ignored.
    pub fn parse(text: &str) -> Self {
        let mut segs = Vec::new();
        let mut current = String::new();
        let mut chars = text.trim().trim_start_matches('.').chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '.' => {
                    if !current.is_empty() {
                        segs.push(Seg::Key(std::mem::take(&mut current)));
                    }
                }
                '[' => {
                    if !current.is_empty() {
                        segs.push(Seg::Key(std::mem::take(&mut current)));
                    }
                    let mut inner = String::new();
                    for c in chars.by_ref() {
                        if c == ']' {
                            break;
                        }
                        inner.push(c);
                    }
                    match inner.parse() {
                        Ok(ix) => segs.push(Seg::Index(ix)),
                        // `[name=web]` (list-map keys): not addressable here, stop at the list.
                        Err(_) => break,
                    }
                }
                c => current.push(c),
            }
        }
        if !current.is_empty() {
            segs.push(Seg::Key(current));
        }
        Self(segs)
    }

    pub fn starts_with(&self, prefix: &[&str]) -> bool {
        self.0.len() >= prefix.len()
            && self
                .0
                .iter()
                .zip(prefix)
                .all(|(s, p)| matches!(s, Seg::Key(k) if k == p))
    }

    pub fn push_key(&self, key: &str) -> Self {
        let mut p = self.clone();
        p.0.push(Seg::Key(key.to_string()));
        p
    }

    pub fn push_index(&self, ix: usize) -> Self {
        let mut p = self.clone();
        p.0.push(Seg::Index(ix));
        p
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.0.iter().enumerate() {
            match seg {
                Seg::Key(k) if i == 0 => write!(f, "{k}")?,
                Seg::Key(k) => write!(f, ".{k}")?,
                Seg::Index(ix) => write!(f, "[{ix}]")?,
            }
        }
        Ok(())
    }
}

/// What part of an entry an offset is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Key,
    Value,
}

impl Node {
    pub fn as_map(&self) -> Option<&[Entry]> {
        match &self.value {
            NodeValue::Map(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_scalar(&self) -> Option<&Scalar> {
        match &self.value {
            NodeValue::Scalar(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_scalar().map(|s| s.text.as_str())
    }

    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.as_map()?.iter().find(|e| e.key_str() == key)
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entry(key).map(|e| &e.value)
    }

    /// The node at `path`.
    pub fn find(&self, path: &Path) -> Option<&Node> {
        let mut node = self;
        for seg in &path.0 {
            node = match (seg, &node.value) {
                (Seg::Key(k), NodeValue::Map(_)) => node.get(k)?,
                (Seg::Index(ix), NodeValue::Seq(items)) => items.get(*ix)?,
                _ => return None,
            };
        }
        Some(node)
    }

    /// The entry at `path` (its last segment must be a key).
    pub fn find_entry(&self, path: &Path) -> Option<&Entry> {
        let (last, parent) = path.0.split_last()?;
        let Seg::Key(key) = last else {
            return None;
        };
        self.find(&Path(parent.to_vec()))?.entry(key)
    }

    /// The deepest path at `offset`, and whether it is on a key or a value. An offset on a key
    /// returns the path of that key's entry.
    pub fn path_at(&self, offset: usize) -> (Path, Part) {
        let mut path = Path::default();
        let mut node = self;
        loop {
            match &node.value {
                NodeValue::Map(entries) => {
                    let Some(entry) = entries.iter().find(|e| {
                        e.key.span.start <= offset && offset <= e.value.span.end.max(e.key.span.end)
                    }) else {
                        return (path, Part::Value);
                    };
                    path.0.push(Seg::Key(entry.key_str().to_string()));
                    if offset <= entry.key.span.end {
                        return (path, Part::Key);
                    }
                    node = &entry.value;
                }
                NodeValue::Seq(items) => {
                    let Some((ix, item)) = items
                        .iter()
                        .enumerate()
                        .find(|(_, i)| i.span.start <= offset && offset <= i.span.end)
                    else {
                        return (path, Part::Value);
                    };
                    path.0.push(Seg::Index(ix));
                    node = item;
                }
                NodeValue::Scalar(_) => return (path, Part::Value),
            }
        }
    }

    /// Converts to JSON with the YAML 1.2 core schema (what the API server receives).
    pub fn to_json(&self) -> Value {
        match &self.value {
            NodeValue::Map(entries) => {
                let mut map = Map::new();
                for entry in entries {
                    let key = match &entry.key.value {
                        NodeValue::Scalar(s) => s.text.clone(),
                        _ => continue,
                    };
                    map.insert(key, entry.value.to_json());
                }
                Value::Object(map)
            }
            NodeValue::Seq(items) => Value::Array(items.iter().map(Node::to_json).collect()),
            NodeValue::Scalar(s) => s.to_json(),
        }
    }
}

impl Scalar {
    /// The JSON value: quoted and block scalars are strings, plain ones are resolved.
    pub fn to_json(&self) -> Value {
        match self.tag.as_deref() {
            Some("str") | Some("binary") => return Value::String(self.text.clone()),
            Some("int") | Some("float") | Some("bool") | Some("null") => {
                return resolve_plain(&self.text);
            }
            _ => {}
        }
        match self.style {
            Style::Plain => resolve_plain(&self.text),
            _ => Value::String(self.text.clone()),
        }
    }

    /// The JSON type name of a plain scalar (`string`, `integer`, `number`, `boolean`, `null`).
    pub fn type_name(&self) -> &'static str {
        match self.to_json() {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            _ => "string",
        }
    }
}

/// YAML 1.2 core schema resolution of a plain scalar.
pub fn resolve_plain(text: &str) -> Value {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "-.inf" | ".nan" | ".NaN" | ".NAN" => {
            return Value::String(text.to_string());
        }
        _ => {}
    }
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if let Some(hex) = unsigned.strip_prefix("0x")
        && let Ok(n) = i64::from_str_radix(hex, 16)
    {
        return Value::Number(if text.starts_with('-') { -n } else { n }.into());
    }
    if let Some(oct) = unsigned.strip_prefix("0o")
        && let Ok(n) = i64::from_str_radix(oct, 8)
    {
        return Value::Number(if text.starts_with('-') { -n } else { n }.into());
    }
    if !unsigned.is_empty() && unsigned.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(n) = text.parse::<i64>() {
            return Value::Number(n.into());
        }
        if let Ok(n) = text.parse::<u64>() {
            return Value::Number(n.into());
        }
    }
    let looks_float = unsigned
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'-' | b'+'))
        && unsigned.bytes().any(|b| b.is_ascii_digit())
        && unsigned
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit() || b == b'.');
    if looks_float
        && let Ok(f) = text.parse::<f64>()
        && let Some(n) = Number::from_f64(f)
    {
        return Value::Number(n);
    }
    Value::String(text.to_string())
}

// ----- Text helpers -----

/// Byte offset of the start of `line` (0-based).
pub fn line_start(text: &str, line: usize) -> usize {
    if line == 0 {
        return 0;
    }
    text.match_indices('\n')
        .nth(line - 1)
        .map_or(text.len(), |(i, _)| i + 1)
}

/// 0-based line of `offset`.
pub fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

/// The line containing `offset`, without its line break.
pub fn line_range(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    start..end
}

/// The key path of the (possibly incomplete) line at `offset`, from indentation alone. Used
/// for completion while the buffer doesn't parse. Returns the parent path of the key being
/// typed on that line and the column of that key.
pub fn parent_path_by_indent(text: &str, offset: usize) -> (Path, usize) {
    let current = line_range(text, offset);
    let line = &text[current.start..offset.max(current.start)];
    let here = LineShape::of(line);
    let mut rev: Vec<Seg> = Vec::new();
    let mut target = here.key_col;
    if let Some(dash) = here.dash_col {
        rev.push(Seg::Index(0));
        target = dash;
    }
    let mut end = current.start;
    while end > 0 && target > 0 {
        let prev = line_range(text, end - 1);
        end = prev.start;
        let content = &text[prev];
        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            continue;
        }
        let shape = LineShape::of(content);
        match shape.dash_col {
            // A sibling key inside the same list item (`- name: x` above `  image: y`).
            Some(dash) if dash < target && shape.key_col == target => {
                rev.push(Seg::Index(0));
                target = dash;
            }
            Some(dash) if dash >= target => {}
            _ if shape.key_col < target => {
                if let Some(key) = shape.open_key {
                    rev.push(Seg::Key(key));
                }
                match shape.dash_col {
                    Some(dash) => {
                        rev.push(Seg::Index(0));
                        target = dash;
                    }
                    None => target = shape.key_col,
                }
            }
            _ => {}
        }
    }
    rev.reverse();
    (Path(rev), here.key_col)
}

/// The layout of one line: where its list marker and key start, and its key if the value is
/// empty (`key:` opens a nested block).
struct LineShape {
    dash_col: Option<usize>,
    key_col: usize,
    open_key: Option<String>,
}

impl LineShape {
    fn of(line: &str) -> Self {
        let spaces = line.len() - line.trim_start_matches(' ').len();
        let mut rest = &line[spaces..];
        let mut dash_col = None;
        let mut key_col = spaces;
        while let Some(after) = rest
            .strip_prefix('-')
            .filter(|a| a.is_empty() || a.starts_with(' '))
        {
            dash_col = Some(key_col);
            let pad = 1 + (after.len() - after.trim_start_matches(' ').len());
            key_col += pad;
            rest = &rest[pad.min(rest.len())..];
        }
        let body = rest.split(" #").next().unwrap_or(rest).trim_end();
        let open_key = body
            .strip_suffix(':')
            .filter(|k| !k.contains(": "))
            .map(|k| k.trim().trim_matches('"').trim_matches('\'').to_string());
        Self {
            dash_col,
            key_col,
            open_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CERT: &str = "apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: api-tls
  namespace: payments
spec:
  secretName: api-tls
  duration: 2160h # 90d
  dnsNames:
    - api.payments.example.com
  privateKey:
    size: 256
    rotationPolicy: Allways
";

    #[test]
    fn spans_point_at_keys_and_values() {
        let parsed = parse(CERT);
        assert!(parsed.error.is_none());
        let root = parsed.roots().next().unwrap();
        let entry = root
            .find_entry(&Path::parse("spec.privateKey.rotationPolicy"))
            .unwrap();
        assert_eq!(&CERT[entry.key.span.clone()], "rotationPolicy");
        assert_eq!(&CERT[entry.value.span.clone()], "Allways");
        let item = root.find(&Path::parse("spec.dnsNames[0]")).unwrap();
        assert_eq!(&CERT[item.span.clone()], "api.payments.example.com");
        let offset = CERT.find("2160h").unwrap() + 2;
        let (path, part) = root.path_at(offset);
        assert_eq!(path.to_string(), "spec.duration");
        assert_eq!(part, Part::Value);
        let offset = CERT.find("secretName").unwrap() + 3;
        assert_eq!(
            root.path_at(offset),
            (Path::parse("spec.secretName"), Part::Key)
        );
    }

    #[test]
    fn converts_to_json_with_the_core_schema() {
        let parsed = parse(
            "a: 1\nb: \"1\"\nc: true\nd: yes\ne: ~\nf: 1.5\ng: 0x10\nh: !!str 3\ni: |\n  x\n  y\nj: [1, two]\n",
        );
        let json = parsed.roots().next().unwrap().to_json();
        assert_eq!(
            json,
            json!({"a": 1, "b": "1", "c": true, "d": "yes", "e": null, "f": 1.5, "g": 16,
                   "h": "3", "i": "x\ny\n", "j": [1, "two"]})
        );
    }

    #[test]
    fn multi_document_streams_and_errors() {
        let text = "kind: A\n---\nkind: B\n---\n";
        let parsed = parse(text);
        let kinds: Vec<_> = parsed
            .roots()
            .map(|r| r.get("kind").unwrap().as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["A", "B"]);
        assert_eq!(
            parsed
                .doc_at(text.find("B").unwrap())
                .unwrap()
                .root
                .as_ref()
                .unwrap()
                .get("kind")
                .unwrap()
                .as_str(),
            Some("B")
        );

        let parsed = parse("a: 1\nb: [1, 2\nc: 3\n");
        let error = parsed.error.unwrap();
        assert!(!error.message.is_empty());
    }

    #[test]
    fn non_ascii_offsets_are_bytes() {
        let text = "a: \"ä ü\"\nb: x\n";
        let root = parse(text).docs.remove(0).root.unwrap();
        let b = root.get("b").unwrap();
        assert_eq!(&text[b.span.clone()], "x");
    }

    #[test]
    fn paths_parse_and_print() {
        let p = Path::parse("spec.containers[0].image");
        assert_eq!(
            p.0,
            vec![
                Seg::Key("spec".into()),
                Seg::Key("containers".into()),
                Seg::Index(0),
                Seg::Key("image".into())
            ]
        );
        assert_eq!(p.to_string(), "spec.containers[0].image");
        assert_eq!(Path::parse(".spec.replicas").to_string(), "spec.replicas");
    }

    #[test]
    fn parent_paths_from_indentation() {
        let text = "spec:\n  privateKey:\n    ro";
        let (path, indent) = parent_path_by_indent(text, text.len());
        assert_eq!(path.to_string(), "spec.privateKey");
        assert_eq!(indent, 4);

        let text = "spec:\n  containers:\n    - name: web\n      im";
        let (path, _) = parent_path_by_indent(text, text.len());
        assert_eq!(path.to_string(), "spec.containers[0]");

        let text = "spec:\n  containers:\n    - na";
        let (path, _) = parent_path_by_indent(text, text.len());
        assert_eq!(path.to_string(), "spec.containers[0]");

        let text = "metadata:\n  name: x\nsp";
        let (path, _) = parent_path_by_indent(text, text.len());
        assert_eq!(path.to_string(), "");
    }
}
