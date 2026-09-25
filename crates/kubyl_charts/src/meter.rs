//! A usage bar with an optional marker, e.g. usage against a limit with the request marked.

use gpui::{App, Hsla, IntoElement, RenderOnce, Window, div, prelude::*, relative};
use kubyl_ui::{ActiveColors, u};

/// `.bar` from the mockups plus a thin marker line. Red from 85%, yellow from 70%.
#[derive(IntoElement)]
pub struct Meter {
    percent: f32,
    marker: Option<f32>,
    color: Option<Hsla>,
    width: Option<f32>,
}

impl Meter {
    pub fn new(percent: f32) -> Self {
        Self {
            percent: percent.clamp(0.0, 100.0),
            marker: None,
            color: None,
            width: None,
        }
    }

    /// Marks a second value (percent of the same scale), e.g. the request.
    pub fn marker(mut self, percent: Option<f32>) -> Self {
        self.marker = percent.map(|p| p.clamp(0.0, 100.0));
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// Fixed width in unscaled pixels (default: full width).
    pub fn width(mut self, px: f32) -> Self {
        self.width = Some(px);
        self
    }
}

/// The bar color for a usage percentage.
pub fn usage_color(percent: f32, colors: &kubyl_ui::Colors) -> Hsla {
    if percent >= 85.0 {
        colors.red
    } else if percent >= 70.0 {
        colors.yellow
    } else {
        colors.accent
    }
}

impl RenderOnce for Meter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.colors();
        let color = self
            .color
            .unwrap_or_else(|| usage_color(self.percent, colors));
        div()
            .relative()
            .flex_none()
            .h(u(5.0))
            .rounded(u(3.0))
            .bg(colors.bar_track)
            .map(|this| match self.width {
                Some(width) => this.w(u(width)),
                None => this.w_full(),
            })
            .child(
                div()
                    .h_full()
                    .rounded(u(3.0))
                    .bg(color)
                    .w(relative(self.percent / 100.0)),
            )
            .when_some(self.marker, |this, marker| {
                this.child(
                    div()
                        .absolute()
                        .top(u(-2.0))
                        .left(relative(marker / 100.0))
                        .w(u(1.5))
                        .h(u(9.0))
                        .bg(colors.text_muted),
                )
            })
    }
}
