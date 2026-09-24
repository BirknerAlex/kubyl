use gpui::{
    AnyElement, App, FontWeight, IntoElement, MouseButton, RenderOnce, SharedString, Window, div,
    prelude::*,
};
use gpui_component::{h_flex, v_flex};
use smallvec::SmallVec;

use crate::{ActiveColors, u};

type DismissHandler = Box<dyn Fn(&mut Window, &mut App)>;

/// A centered dialog over a dimmed backdrop. Render it as the last child of the window's root
/// so it covers everything; clicking the backdrop calls `on_dismiss`.
#[derive(IntoElement)]
pub struct Modal {
    title: SharedString,
    body: SmallVec<[AnyElement; 2]>,
    footer: SmallVec<[AnyElement; 3]>,
    width: f32,
    on_dismiss: Option<DismissHandler>,
}

impl Modal {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            body: SmallVec::new(),
            footer: SmallVec::new(),
            width: 460.0,
            on_dismiss: None,
        }
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.body.push(child.into_any_element());
        self
    }

    /// Adds a footer element (usually a [`crate::Button`]); footer items are right-aligned.
    pub fn footer(mut self, child: impl IntoElement) -> Self {
        self.footer.push(child.into_any_element());
        self
    }

    /// Width in unscaled pixels (default 460).
    pub fn width(mut self, px: f32) -> Self {
        self.width = px;
        self
    }

    pub fn on_dismiss(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Modal {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        div()
            .id("modal-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.modal_backdrop)
            .occlude()
            .when_some(self.on_dismiss, |this, f| {
                this.on_mouse_down(MouseButton::Left, move |_, window, cx| f(window, cx))
            })
            .child(
                v_flex()
                    .id("modal")
                    .w(u(self.width))
                    .max_w_full()
                    .rounded(u(8.0))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.panel)
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(u(18.0))
                            .pt(u(16.0))
                            .pb(u(8.0))
                            .text_size(u(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text)
                            .child(self.title),
                    )
                    .child(
                        v_flex()
                            .px(u(18.0))
                            .pb(u(16.0))
                            .gap(u(8.0))
                            .text_color(colors.text_muted)
                            .children(self.body),
                    )
                    .when(!self.footer.is_empty(), |this| {
                        this.child(
                            h_flex()
                                .justify_end()
                                .gap(u(8.0))
                                .px(u(18.0))
                                .py(u(12.0))
                                .border_t_1()
                                .border_color(colors.border_variant)
                                .children(self.footer),
                        )
                    }),
            )
    }
}
