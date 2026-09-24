use gpui::{
    App, ClickEvent, ElementId, FontWeight, IntoElement, RenderOnce, SharedString, Window,
    prelude::*,
};
use gpui_component::h_flex;

use crate::{ActiveColors, Icon, IconName, u};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// Bordered, raised background (`.btn`).
    #[default]
    Default,
    /// No border or background until hovered (`.btn.g`).
    Ghost,
    /// Accent fill (`.btn.p`).
    Primary,
    /// Red label for destructive actions (`.btn.d`).
    Danger,
}

/// A 26px button with an optional icon and label.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<IconName>,
    variant: ButtonVariant,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            variant: ButtonVariant::Default,
            disabled: false,
            on_click: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    pub fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let (bg, border, fg, hover_bg) = match self.variant {
            ButtonVariant::Default => (
                colors.button_background,
                colors.border,
                colors.text,
                colors.hover,
            ),
            ButtonVariant::Ghost => (
                gpui::transparent_black(),
                gpui::transparent_black(),
                colors.text_muted,
                colors.hover,
            ),
            ButtonVariant::Primary => (
                colors.accent,
                colors.accent,
                colors.on_accent,
                colors.accent.opacity(0.88),
            ),
            ButtonVariant::Danger => (
                colors.button_background,
                colors.border,
                colors.red,
                colors.hover,
            ),
        };
        h_flex()
            .id(self.id)
            .flex_none()
            .h(u(26.0))
            .px(u(10.0))
            .gap(u(6.0))
            .rounded(u(5.0))
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_size(u(13.0))
            .whitespace_nowrap()
            .when(self.variant == ButtonVariant::Primary, |this| {
                this.font_weight(FontWeight::SEMIBOLD)
            })
            .when(self.disabled, |this| this.opacity(0.5))
            .when(!self.disabled, |this| {
                this.cursor_pointer().hover(move |style| style.bg(hover_bg))
            })
            .when_some(self.icon, |this, icon| {
                this.child(Icon::new(icon).size(13.0).color(fg))
            })
            .when_some(self.label, |this, label| this.child(label))
            .when_some(
                self.on_click.filter(|_| !self.disabled),
                |this, on_click| this.on_click(on_click),
            )
    }
}

/// A square 26px icon-only button (`.ib`), used in headers and toolbars.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    icon_size: f32,
    toggled: bool,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            icon_size: 14.0,
            toggled: false,
            on_click: None,
        }
    }

    pub fn icon_size(mut self, px: f32) -> Self {
        self.icon_size = px;
        self
    }

    /// Shows the icon in the accent color (dock visible, filter active…).
    pub fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = toggled;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let color = if self.toggled {
            colors.accent
        } else {
            colors.text_dim
        };
        let hover = colors.hover;
        h_flex()
            .id(self.id)
            .flex_none()
            .size(u(26.0))
            .justify_center()
            .rounded(u(5.0))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .child(Icon::new(self.icon).size(self.icon_size).color(color))
            .when_some(self.on_click, |this, on_click| this.on_click(on_click))
    }
}
