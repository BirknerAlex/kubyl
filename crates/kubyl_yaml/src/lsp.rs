//! gpui-component editor providers: hover docs and completion from [`crate::intel`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui::{App, Task, WeakEntity, Window};
use gpui_component::input::{CompletionProvider, HoverProvider, Rope, RopeExt as _};
use kubyl_core::Gvk;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Hover, HoverContents, MarkupContent, MarkupKind, TextEdit,
};
use serde_json::Value;

use crate::intel::{self, CandidateKind, RefKind};
use crate::view::YamlEditor;

/// Hover and completion for one editor.
pub struct YamlLsp {
    pub view: WeakEntity<YamlEditor>,
}

type Found = Option<(Gvk, (Arc<Value>, String))>;

impl YamlLsp {
    /// The schema of the document at `offset` (starts loading it when needed).
    fn schema_at(&self, text: &str, offset: usize, cx: &mut App) -> Found {
        let gvk = intel::doc_header(text, offset)?;
        let view = self.view.upgrade()?;
        let found = view.update(cx, |view, cx| view.schema_lookup(&gvk, cx))?;
        Some((gvk, found))
    }
}

fn lookup_from(found: Found) -> impl Fn(&Gvk) -> Option<(Arc<Value>, String)> {
    move |gvk: &Gvk| {
        found
            .as_ref()
            .filter(|(g, _)| g == gvk)
            .map(|(_, f)| f.clone())
    }
}

fn lsp_range(rope: &Rope, range: std::ops::Range<usize>) -> lsp_types::Range {
    lsp_types::Range {
        start: rope.offset_to_position(range.start),
        end: rope.offset_to_position(range.end),
    }
}

impl HoverProvider for YamlLsp {
    fn hover(
        &self,
        rope: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let text = rope.to_string();
        let lookup = lookup_from(self.schema_at(&text, offset, cx));
        let hover = intel::hover(&text, offset, &lookup).map(|(range, info)| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: info.markdown(),
            }),
            range: Some(lsp_range(rope, range)),
        });
        Task::ready(Ok(hover))
    }
}

impl CompletionProvider for YamlLsp {
    fn completions(
        &self,
        rope: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let text = rope.to_string();
        let lookup = lookup_from(self.schema_at(&text, offset, cx));
        let Some(view) = self.view.upgrade() else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let kinds = view.read(cx).served_kinds(cx);
        // Names come from the watch caches; kinds without one are watched from now on.
        let wanted: Rc<RefCell<Vec<RefKind>>> = Rc::default();
        let (range, candidates) = {
            let app: &App = cx;
            let view_ref = view.read(app);
            let names = |refs: &RefKind| {
                let (names, missing) = view_ref.reference_names(refs, app);
                if missing {
                    wanted.borrow_mut().push(refs.clone());
                }
                names
            };
            let ctx = intel::CompletionContext {
                lookup: &lookup,
                kinds: &kinds,
                names: &names,
            };
            intel::complete(&text, offset, &ctx)
        };
        for refs in wanted.take() {
            view.update(cx, |view, cx| view.watch_references(&refs, cx));
        }
        let edit_range = lsp_range(rope, range);
        let items = candidates
            .into_iter()
            .enumerate()
            .map(|(ix, c)| CompletionItem {
                label: c.label.clone(),
                kind: Some(match c.kind {
                    CandidateKind::Field => CompletionItemKind::FIELD,
                    CandidateKind::Value => CompletionItemKind::ENUM_MEMBER,
                    CandidateKind::Kind => CompletionItemKind::CLASS,
                    CandidateKind::Reference => CompletionItemKind::REFERENCE,
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
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '/' | '_'))
    }
}
