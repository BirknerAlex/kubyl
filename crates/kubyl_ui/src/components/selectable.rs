//! Text that can be selected with the mouse and copied (`⌘C` / `Ctrl+C`), for values on
//! details pages (IPs, versions, names).

use std::ops::Range;

use gpui::{
    App, ClipboardItem, ElementId, HighlightStyle, IntoElement, KeyBinding, RenderOnce,
    SharedString, Window, actions,
};
use gpui_base::TextSelection;

use super::selection::{SelectableLabel, select_all_in_last_scope};

actions!(
    kubyl_ui,
    [
        /// Copies the text selected on a details page.
        CopySelectedText,
        /// Selects all selectable text of the panel last clicked in.
        SelectAllText,
    ]
);

pub(crate) fn init(cx: &mut App) {
    // Global: inputs, the terminal and the logs bind their own copy in deeper contexts, and the
    // handler passes the keystroke on when nothing is selected (`Ctrl+C` in a shell).
    cx.bind_keys([KeyBinding::new("secondary-c", CopySelectedText, None)]);
    // Not in the palette: running it from there would start with a press that resets the anchor.
    cx.bind_keys([KeyBinding::new(
        "secondary-a",
        SelectAllText,
        Some("Workspace"),
    )]);
}

/// Selects all text of the scope the last mouse press was in; passes `⌘A` on when there is
/// none. Attach to the window's root view: `.on_action(select_all_text)`.
pub fn select_all_text(_: &SelectAllText, window: &mut Window, cx: &mut App) {
    if !select_all_in_last_scope(window, cx) {
        cx.propagate();
    }
}

/// Whether a drag selected text (a click, even with a little pointer travel, selects none):
/// clickable text uses it to not act after a drag.
pub fn has_text_selection(window: &mut Window, cx: &mut App) -> bool {
    super::selection::has_selected_text(window, cx)
}

/// Clears the window's text selection.
pub fn clear_text_selection(window: &mut Window, cx: &mut App) {
    TextSelection::clear(window, cx);
}

/// Copies the window's text selection. Attach to the window's root view:
/// `.on_action(copy_selected_text)`.
pub fn copy_selected_text(_: &CopySelectedText, window: &mut Window, cx: &mut App) {
    match selected_text(window, cx) {
        Some(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
        None => cx.propagate(),
    }
}

/// The window's selected text, laid out like the UI; `None` when nothing is selected.
pub fn selected_text(window: &mut Window, cx: &mut App) -> Option<String> {
    super::selection::selected_text(window, cx)
}

/// A text run that joins the window's text selection. Inherits the parent's text style.
#[derive(IntoElement)]
pub struct Selectable {
    id: ElementId,
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

impl Selectable {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            highlights: Vec::new(),
        }
    }

    /// Styles byte ranges of the text (syntax highlighting).
    pub fn highlights(mut self, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Self {
        self.highlights = highlights;
        self
    }
}

impl RenderOnce for Selectable {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        SelectableLabel::new(self.id, self.text).highlights(self.highlights)
    }
}
