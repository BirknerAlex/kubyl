//! The YAML tab: the file as it will be written (comments and key order kept), in phase 04's
//! editor with the kubeconfig schema (validation, hover, completion). Secret values are masked
//! until revealed; edits flow back into the document (and the form) as long as they parse.

use std::ops::Range;
use std::rc::Rc;

use anyhow::Result;
use gpui::{App, AppContext as _, Context, Entity, Subscription, Task, WeakEntity, Window};
use gpui_component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::{
    CompletionProvider, EditorState, HoverProvider, InputEvent, Rope, RopeExt as _,
};
use kubyl_yaml::parse::{self, Node, Path as YPath, Seg};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Hover, HoverContents, MarkupContent, MarkupKind, TextEdit,
};
use serde_json::Value;

use crate::editor::KubeconfigEditor;
use crate::model::{self, Doc, Kind};
use crate::validate::{self, Problem, Severity};
use crate::{schema, yaml};

/// What masked secret values look like in the YAML tab.
pub const MASK: &str = "••••••••";

pub struct YamlTab {
    pub editor: Entity<EditorState>,
    /// Masked values: user name, path in the user's body, the real value.
    masked: Vec<(String, Vec<String>, String)>,
    pub revealed: bool,
    /// Set while a change comes from typing in this tab (the text isn't reset then).
    pub editing: bool,
    /// The buffer doesn't parse; the document keeps its last good state.
    pub parse_error: Option<String>,
    _subscription: Subscription,
}

impl YamlTab {
    pub fn new(
        view: WeakEntity<KubeconfigEditor>,
        window: &mut Window,
        cx: &mut Context<KubeconfigEditor>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("yaml")
                .line_number(true)
                .folding(true)
                .searchable(true)
                .soft_wrap(false);
            let lsp = Rc::new(KubeconfigLsp { view: view.clone() });
            state.lsp_mut().hover_provider = Some(lsp.clone());
            state.lsp_mut().completion_provider = Some(lsp);
            state.lsp_mut().completion_menu.max_width = gpui::px(420.);
            state
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.yaml_typed(window, cx);
                }
            },
        );
        Self {
            editor,
            masked: Vec::new(),
            revealed: false,
            editing: false,
            parse_error: None,
            _subscription: subscription,
        }
    }

    /// Replaces the buffer with the document as it will be written.
    pub fn set_text(
        &mut self,
        old_text: Option<&str>,
        doc: &Doc,
        window: &mut Window,
        cx: &mut App,
    ) {
        let text = match old_text {
            Some(old) => yaml::write(old, &doc.0).text,
            None => yaml::render(&doc.0),
        };
        let (text, masked) = if self.revealed {
            (text, Vec::new())
        } else {
            mask(&text, doc)
        };
        self.masked = masked;
        self.parse_error = None;
        let current = self.editor.read(cx).value().to_string();
        if current != text {
            self.editor
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let focus = gpui::Focusable::focus_handle(self.editor.read(cx), cx);
        window.focus(&focus, cx);
    }

    /// Puts the cursor on an entry's `name:` line (the list's selection) and scrolls there.
    /// `false`: the editor wasn't laid out yet, try again after a frame.
    pub fn reveal(
        &self,
        doc: &Doc,
        entry: &crate::model::EntryRef,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(line_height) = self.editor.read(cx).line_height() else {
            return false;
        };
        let text = self.editor.read(cx).value().to_string();
        let parsed = parse::parse(&text);
        let Some(root) = parsed.roots().next() else {
            return true;
        };
        let Some(ix) = doc.names(entry.kind).iter().position(|n| n == &entry.name) else {
            return true;
        };
        let path = YPath(vec![
            Seg::Key(entry.kind.list_key().into()),
            Seg::Index(ix),
            Seg::Key("name".into()),
        ]);
        let Some(node) = root.find(&path) else {
            return true;
        };
        let offset = node.span.start;
        self.editor.update(cx, |state, cx| {
            let position = state.text().offset_to_position(offset);
            state.set_cursor_position(position, window, cx);
            // The entry a few lines below the top rather than at the bottom edge.
            let top = position.line.saturating_sub(3) as f32;
            state.set_scroll_offset(gpui::point(gpui::px(0.), -(line_height * top)), cx);
        });
        true
    }

    /// The document for the buffer, with masked values put back. `Err`: it doesn't parse.
    pub fn parse(&self, cx: &App) -> Result<Doc, String> {
        let text = self.editor.read(cx).value().to_string();
        let mut doc = Doc::parse(&text)?;
        for (user, path, original) in &self.masked {
            let refs: Vec<&str> = path.iter().map(String::as_str).collect();
            if let Some(body) = doc.body_mut(Kind::User, user)
                && let Some(value) = model::get_json_mut(body, &refs)
                && value.as_str() == Some(MASK)
            {
                *value = Value::String(original.clone());
            }
        }
        Ok(doc)
    }

    /// Diagnostics: syntax, schema (unknown fields, types, enums) and the editor's problems.
    pub fn refresh_diagnostics(&self, doc: &Doc, problems: &[Problem], cx: &mut App) {
        let text = self.editor.read(cx).value().to_string();
        let parsed = parse::parse(&text);
        let mut found: Vec<(Range<usize>, Severity, String)> =
            kubyl_yaml::validate::syntax_problems(&parsed)
                .into_iter()
                .map(|p| (p.range, Severity::Error, p.message))
                .collect();
        if let Some(root) = parsed.roots().next() {
            let schema_doc = schema::document();
            if let Some(schema) = kubyl_yaml::schema::Schema::for_gvk(&schema_doc, &schema::gvk()) {
                for p in kubyl_yaml::validate::validate(root, &schema) {
                    let severity = match p.severity {
                        kubyl_yaml::validate::Severity::Error => Severity::Error,
                        kubyl_yaml::validate::Severity::Warning => Severity::Warning,
                        kubyl_yaml::validate::Severity::Info => Severity::Info,
                    };
                    found.push((p.range, severity, p.message));
                }
            }
            for problem in problems.iter().filter(|p| p.severity != Severity::Info) {
                let path = validate::yaml_path(doc, problem);
                if let Some(range) = find_range(root, &path) {
                    found.push((range, problem.severity, problem.message.clone()));
                }
            }
        }
        self.editor.update(cx, |state, cx| {
            let rope = state.text().clone();
            let len = rope.len();
            let text = rope.to_string();
            if let Some(set) = state.diagnostics_mut() {
                set.reset(&rope);
                for (range, severity, message) in found {
                    let start = range.start.min(len);
                    let mut end = range.end.min(len);
                    if end <= start {
                        end = text
                            .get(start..)
                            .and_then(|s| s.chars().next())
                            .map_or(start, |c| start + c.len_utf8());
                    }
                    set.push(
                        Diagnostic::new(
                            rope.offset_to_position(start)..rope.offset_to_position(end),
                            message,
                        )
                        .with_severity(match severity {
                            Severity::Error => DiagnosticSeverity::Error,
                            Severity::Warning => DiagnosticSeverity::Warning,
                            Severity::Info => DiagnosticSeverity::Info,
                        })
                        .with_source("kubyl"),
                    );
                }
            }
            cx.notify();
        });
    }
}

/// The span of the deepest node on `path` (the entry's key when the value is missing).
fn find_range(root: &Node, path: &YPath) -> Option<Range<usize>> {
    let mut prefix = path.clone();
    while !prefix.0.is_empty() {
        if let Some(entry) = root.find_entry(&prefix) {
            return Some(entry.key.span.clone());
        }
        if let Some(node) = root.find(&prefix) {
            return Some(node.span.clone());
        }
        prefix.0.pop();
    }
    None
}

/// Masks the secret values of `text` (the rendering of `doc`). Returns the new text and what
/// was masked.
pub fn mask(text: &str, doc: &Doc) -> (String, Vec<(String, Vec<String>, String)>) {
    let parsed = parse::parse(text);
    let Some(root) = parsed.roots().next() else {
        return (text.to_string(), Vec::new());
    };
    let Some(users) = root.get("users").and_then(|u| match &u.value {
        parse::NodeValue::Seq(items) => Some(items.clone()),
        _ => None,
    }) else {
        return (text.to_string(), Vec::new());
    };
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut masked = Vec::new();
    for item in &users {
        let Some(name) = item.get("name").and_then(Node::as_str) else {
            continue;
        };
        let Some(body) = doc.body(Kind::User, name) else {
            continue;
        };
        for path in model::secret_paths(body) {
            let mut segs = vec![Seg::Key("user".into())];
            for part in &path {
                segs.push(match part.parse::<usize>() {
                    Ok(ix) => Seg::Index(ix),
                    Err(_) => Seg::Key(part.clone()),
                });
            }
            let Some(node) = item.find(&YPath(segs)) else {
                continue;
            };
            let Some(original) = node.as_str() else {
                continue;
            };
            if node.span.is_empty() {
                continue;
            }
            masked.push((name.to_string(), path.clone(), original.to_string()));
            edits.push((node.span.clone(), MASK.to_string()));
        }
    }
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = text.to_string();
    for (range, replacement) in edits {
        out.replace_range(range, &replacement);
    }
    (out, masked)
}

impl KubeconfigEditor {
    /// The YAML tab's text for the current document.
    pub(crate) fn show_yaml(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.snapshot.as_ref().map(|s| s.text.clone());
        let doc = self.doc.clone();
        self.yaml.set_text(old.as_deref(), &doc, window, cx);
    }

    /// The user typed in the YAML tab.
    fn yaml_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.yaml.parse(cx) {
            Ok(doc) => {
                self.yaml.parse_error = None;
                if doc != self.doc {
                    self.doc = doc;
                    self.yaml.editing = true;
                    self.changed(window, cx);
                    self.yaml.editing = false;
                } else {
                    self.yaml.refresh_diagnostics(&self.doc, &self.problems, cx);
                }
            }
            Err(err) => {
                self.yaml.parse_error = Some(err);
                self.yaml.refresh_diagnostics(&self.doc, &self.problems, cx);
                cx.notify();
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn yaml_typed_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.yaml_typed(window, cx);
    }

    pub(crate) fn toggle_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.yaml.revealed = !self.yaml.revealed;
        self.show_yaml(window, cx);
        cx.notify();
    }
}

/// Hover and completion in the YAML tab.
struct KubeconfigLsp {
    view: WeakEntity<KubeconfigEditor>,
}

fn lookup(gvk: &kubyl_core::Gvk) -> Option<(std::sync::Arc<Value>, String)> {
    (gvk == &schema::gvk()).then(|| (schema::document(), schema::SOURCE.to_string()))
}

fn lsp_range(rope: &Rope, range: Range<usize>) -> lsp_types::Range {
    lsp_types::Range {
        start: rope.offset_to_position(range.start),
        end: rope.offset_to_position(range.end),
    }
}

impl HoverProvider for KubeconfigLsp {
    fn hover(
        &self,
        rope: &Rope,
        offset: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let text = rope.to_string();
        let hover = kubyl_yaml::intel::hover(&text, offset, &lookup).map(|(range, info)| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.markdown(),
            }),
            range: Some(lsp_range(rope, range)),
        });
        Task::ready(Ok(hover))
    }
}

impl CompletionProvider for KubeconfigLsp {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let text = rope.to_string();
        let kinds = vec![("v1".to_string(), "Config".to_string())];
        let no_names = |_: &kubyl_yaml::intel::RefKind| Vec::new();
        let ctx = kubyl_yaml::intel::CompletionContext {
            lookup: &lookup,
            kinds: &kinds,
            names: &no_names,
        };
        let (mut range, mut candidates) = kubyl_yaml::intel::complete(&text, offset, &ctx);
        // Names of this file's entries after `cluster:`, `user:` and `current-context:`.
        let line = parse::line_range(&text, offset);
        let before = &text[line.start..offset.min(text.len())];
        let trimmed = before.trim_start().trim_start_matches("- ");
        let refs = [
            ("cluster:", Kind::Cluster),
            ("user:", Kind::User),
            ("current-context:", Kind::Context),
        ];
        if let Some(view) = self.view.upgrade()
            && let Some((prefix, kind)) = refs.iter().find(|(p, _)| trimmed.starts_with(p))
        {
            let typed = trimmed[prefix.len()..].trim_start();
            let names = view.read(cx).doc.names(*kind);
            range = offset - typed.len()..offset;
            candidates = names
                .into_iter()
                .filter(|n| n.starts_with(typed))
                .map(|n| kubyl_yaml::intel::Candidate {
                    label: n.clone(),
                    detail: kind.label().to_string(),
                    insert: n,
                    kind: kubyl_yaml::intel::CandidateKind::Reference,
                    required: false,
                })
                .collect();
        }
        let edit_range = lsp_range(rope, range);
        let items = candidates
            .into_iter()
            .enumerate()
            .map(|(ix, c)| CompletionItem {
                label: c.label.clone(),
                kind: Some(match c.kind {
                    kubyl_yaml::intel::CandidateKind::Field => CompletionItemKind::FIELD,
                    kubyl_yaml::intel::CandidateKind::Value => CompletionItemKind::ENUM_MEMBER,
                    kubyl_yaml::intel::CandidateKind::Kind => CompletionItemKind::CLASS,
                    kubyl_yaml::intel::CandidateKind::Reference => CompletionItemKind::REFERENCE,
                }),
                detail: Some(if c.required {
                    format!("{} · required", c.detail)
                } else {
                    c.detail
                }),
                sort_text: Some(format!("{ix:04}")),
                filter_text: Some(c.label),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: edit_range,
                    new_text: c.insert,
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _: &mut App) -> bool {
        !new_text.is_empty()
            && new_text.len() <= 2
            && new_text
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '/' | '_' | ' '))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_secrets_and_puts_them_back() {
        let text = "users:\n- name: a # admin\n  user:\n    token: s3cret\n    exec:\n      command: x\n      env:\n      - name: API_TOKEN\n        value: hush\n- name: b\n  user:\n    client-key-data: S0VZ\n";
        let doc = Doc::parse(text).unwrap();
        let (masked, table) = mask(text, &doc);
        assert!(!masked.contains("s3cret") && !masked.contains("hush") && !masked.contains("S0VZ"));
        assert!(masked.contains("# admin"));
        assert_eq!(table.len(), 3);
        // The masked text parses; restoring gives the original document back.
        let mut back = Doc::parse(&masked).unwrap();
        for (user, path, original) in &table {
            let refs: Vec<&str> = path.iter().map(String::as_str).collect();
            let body = back.body_mut(Kind::User, user).unwrap();
            *model::get_json_mut(body, &refs).unwrap() = Value::String(original.clone());
        }
        assert_eq!(back, doc);
    }
}
