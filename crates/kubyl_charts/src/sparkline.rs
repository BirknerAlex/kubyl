//! A small trend line (KPI tiles, the pod details usage section).

use gpui::{App, Hsla, IntoElement, RenderOnce, Window, canvas, point, prelude::*};
use kubyl_ui::u;

use crate::paint::{fill_between, scaled, stroke, x_at, y_at};

/// Auto-scaled line (min..max of the values) with a light area fill, like the mockup's
/// `spark()`. Fewer than two values draw nothing.
#[derive(IntoElement)]
pub struct Sparkline {
    values: Vec<f64>,
    color: Hsla,
    fill: bool,
    height: f32,
    stroke: f32,
    zero_based: bool,
}

impl Sparkline {
    pub fn new(values: Vec<f64>, color: Hsla) -> Self {
        Self {
            values,
            color,
            fill: true,
            height: 30.0,
            stroke: 1.5,
            zero_based: false,
        }
    }

    /// Height in unscaled pixels (default 30).
    pub fn height(mut self, px: f32) -> Self {
        self.height = px;
        self
    }

    pub fn fill(mut self, fill: bool) -> Self {
        self.fill = fill;
        self
    }

    /// Scale from zero instead of the smallest value.
    pub fn zero_based(mut self) -> Self {
        self.zero_based = true;
        self
    }
}

impl RenderOnce for Sparkline {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let Self {
            values,
            color,
            fill,
            height,
            stroke: width,
            zero_based,
        } = self;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if values.len() < 2 {
                    return;
                }
                let max = values.iter().copied().fold(f64::MIN, f64::max);
                let min = if zero_based {
                    0.0
                } else {
                    values.iter().copied().fold(f64::MAX, f64::min)
                };
                let pad = scaled(window, 2.0);
                let points: Vec<_> = values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        Some(point(
                            x_at(&bounds, i, values.len()),
                            y_at(&bounds, *v, min, max, pad),
                        ))
                    })
                    .collect();
                if fill {
                    let baseline: Vec<_> = (0..values.len())
                        .map(|i| point(x_at(&bounds, i, values.len()), bounds.bottom()))
                        .collect();
                    fill_between(&points, &baseline, color.opacity(0.12), window);
                }
                stroke(&points, color, scaled(window, width), None, window);
            },
        )
        .w_full()
        .h(u(height))
    }
}
