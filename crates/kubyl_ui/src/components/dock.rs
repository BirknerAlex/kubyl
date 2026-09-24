use gpui::{AnyElement, App, IntoElement, RenderOnce, SharedString, Window};
use smallvec::SmallVec;

use crate::PanelHeader;

/// Header of a dock panel (`Pod details  ☆ ×`): a [`PanelHeader`] with a separator below.
#[derive(IntoElement)]
pub struct DockHeader {
    title: SharedString,
    end: SmallVec<[AnyElement; 3]>,
}

impl DockHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            end: SmallVec::new(),
        }
    }

    pub fn end_child(mut self, child: impl IntoElement) -> Self {
        self.end.push(child.into_any_element());
        self
    }
}

impl RenderOnce for DockHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.end
            .into_iter()
            .fold(PanelHeader::new(self.title).bordered(), |header, child| {
                header.end_child(child)
            })
    }
}
