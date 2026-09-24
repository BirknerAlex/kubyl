use gpui::{AnyElement, App, Hsla, IntoElement, RenderOnce, SharedString, Window, div, prelude::*};
use gpui_component::h_flex;
use smallvec::SmallVec;

use crate::{ActiveColors, Icon, IconName, sizes, u};

/// The bottom status bar (`.status`) with left and right item slots.
#[derive(IntoElement, Default)]
pub struct StatusBar {
    left: SmallVec<[AnyElement; 8]>,
    right: SmallVec<[AnyElement; 8]>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn left(mut self, item: impl IntoElement) -> Self {
        self.left.push(item.into_any_element());
        self
    }

    pub fn right(mut self, item: impl IntoElement) -> Self {
        self.right.push(item.into_any_element());
        self
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .flex_none()
            .h(u(sizes::STATUS_BAR))
            .px(u(10.0))
            .gap(u(14.0))
            .bg(colors.panel)
            .border_t_1()
            .border_color(colors.border)
            .text_size(u(sizes::SMALL_FONT))
            .text_color(colors.text_dim)
            .whitespace_nowrap()
            .overflow_hidden()
            .children(self.left)
            .child(div().flex_1())
            .children(self.right)
    }
}

/// An icon + text item for the status bar.
#[derive(IntoElement)]
pub struct StatusBarText {
    icon: Option<(IconName, Option<Hsla>)>,
    text: SharedString,
    color: Option<Hsla>,
}

impl StatusBarText {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            icon: None,
            text: text.into(),
            color: None,
        }
    }

    pub fn icon(mut self, icon: IconName, color: Option<Hsla>) -> Self {
        self.icon = Some((icon, color));
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for StatusBarText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        h_flex()
            .gap(u(5.0))
            .when_some(self.icon, |this, (icon, color)| {
                this.child(
                    Icon::new(icon)
                        .size(12.0)
                        .color(color.unwrap_or(colors.text_dim)),
                )
            })
            .when_some(self.color, |this, color| this.text_color(color))
            .child(self.text)
    }
}
