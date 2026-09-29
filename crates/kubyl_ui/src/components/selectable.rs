//! Text that can be selected with the mouse and copied (`⌘C` / `Ctrl+C`), for values on
//! details pages (IPs, versions, names).

use gpui::{
    App, ClipboardItem, ElementId, IntoElement, KeyBinding, RenderOnce, SharedString, Window,
    actions,
};
use gpui_base::{SelectableText, TextSelection};

use crate::ActiveColors;

actions!(
    kubyl_ui,
    [
        /// Copies the text selected on a details page.
        CopySelectedText,
    ]
);

pub(crate) fn init(cx: &mut App) {
    // Global: inputs, the terminal and the logs bind their own copy in deeper contexts, and the
    // handler passes the keystroke on when nothing is selected (`Ctrl+C` in a shell).
    cx.bind_keys([KeyBinding::new("secondary-c", CopySelectedText, None)]);
}

/// Copies the window's text selection. Attach to the window's root view:
/// `.on_action(copy_selected_text)`.
pub fn copy_selected_text(_: &CopySelectedText, window: &mut Window, cx: &mut App) {
    let text = TextSelection::selected_text(window, cx);
    let text = text.trim();
    if text.is_empty() {
        cx.propagate();
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
}

/// A text run that joins the window's text selection. Inherits the parent's text style.
#[derive(IntoElement)]
pub struct Selectable {
    id: ElementId,
    text: SharedString,
}

impl Selectable {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

impl RenderOnce for Selectable {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        SelectableText::new(self.id, self.text).selection_color(cx.colors().accent.opacity(0.3))
    }
}
