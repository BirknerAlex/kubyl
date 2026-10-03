use gpui::{App, ElementId, Hsla, IntoElement, RenderOnce, SharedString, Window, prelude::*};
use gpui_component::h_flex;

use crate::{ActiveColors, Icon, IconName, Selectable, StatusDot, fonts, u};

/// A small label (`.chip`): labels, filters, namespaces.
#[derive(IntoElement)]
pub struct Chip {
    label: SharedString,
    selected: bool,
    mono: bool,
    dot: Option<Hsla>,
    icon: Option<IconName>,
    removable: bool,
    color: Option<Hsla>,
    selectable: Option<ElementId>,
}

impl Chip {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            selected: false,
            mono: false,
            dot: None,
            icon: None,
            removable: false,
            color: None,
            selectable: None,
        }
    }

    /// Accent style (`.chip.on`), for active filters.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Monospace text (`.mchip`), for labels like `app=checkout-api`.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    pub fn dot(mut self, color: Hsla) -> Self {
        self.dot = Some(color);
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Shows a trailing ×.
    pub fn removable(mut self) -> Self {
        self.removable = true;
        self
    }

    pub fn text_color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// The label joins the window's text selection under `id`, which must differ from the other
    /// selectable labels of the same parent. Not for clickable chips.
    pub fn selectable_as(mut self, id: impl Into<ElementId>) -> Self {
        self.selectable = Some(id.into());
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let (bg, fg) = if self.selected {
            (colors.chip_selected_background, colors.chip_selected_text)
        } else {
            (colors.chip_background, colors.text_muted)
        };
        let fg = self.color.unwrap_or(fg);
        h_flex()
            .flex_none()
            .h(u(20.0))
            .px(u(7.0))
            .gap(u(5.0))
            .rounded(u(4.0))
            .bg(bg)
            .text_color(fg)
            .whitespace_nowrap()
            .when(self.selected, |this| {
                this.border_1().border_color(colors.chip_selected_border)
            })
            .map(|this| {
                if self.mono {
                    this.font_family(fonts::MONO).text_size(u(11.0))
                } else {
                    this.text_size(u(11.5))
                }
            })
            .when_some(self.dot, |this, color| this.child(StatusDot::new(color)))
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(11.0).color(fg))
            })
            .map(|this| match self.selectable {
                Some(id) => this.child(Selectable::new(id, self.label)),
                None => this.child(self.label),
            })
            .when(self.removable, |this| {
                this.child(Icon::new(IconName::X).size(10.0).color(fg))
            })
    }
}
