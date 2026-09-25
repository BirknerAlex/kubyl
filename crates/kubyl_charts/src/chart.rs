//! Line, area and stacked-area charts with a hover crosshair, tooltip and legend toggles.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

use gpui::{
    Bounds, Context, FontWeight, IntoElement, MouseMoveEvent, Pixels, Point, Render, SharedString,
    Window, canvas, div, fill, point, prelude::*, relative, size,
};
use kubyl_ui::{ActiveColors, Colors, fonts, h_flex, u, v_flex};

use crate::data::{ChartData, Series, max_value, nearest_index, nice_max, nice_max_binary, stack};
use crate::paint::{fill_between, scaled, stroke, x_at, y_at};

/// How series are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartKind {
    #[default]
    Line,
    /// Lines with a light fill down to the baseline.
    Area,
    /// Series stacked on top of each other (totals read at the top edge).
    StackedArea,
}

/// Formats axis and tooltip values (`1.5 cores`, `640 MiB`…).
pub type ValueFormat = Rc<dyn Fn(f64) -> String>;

/// A chart view. Owners keep an `Entity<LineChart>` and push new data with
/// [`LineChart::set_data`]; hover and legend state survive refreshes.
pub struct LineChart {
    data: ChartData,
    kind: ChartKind,
    format_value: ValueFormat,
    hidden: HashSet<SharedString>,
    /// Pointer position across the plot (0 = left, 1 = right).
    hover: Option<f32>,
    plot: Rc<Cell<Option<Bounds<Pixels>>>>,
    height: f32,
    placeholder: SharedString,
    binary: bool,
    /// Small multiples in the details dock: no legend (the caller shows the latest values) and
    /// only the top gridline labelled.
    compact: bool,
}

impl LineChart {
    pub fn new(kind: ChartKind, format_value: impl Fn(f64) -> String + 'static) -> Self {
        Self {
            data: ChartData::default(),
            kind,
            format_value: Rc::new(format_value),
            hidden: HashSet::new(),
            hover: None,
            plot: Rc::default(),
            height: 150.0,
            placeholder: "Loading…".into(),
            binary: false,
            compact: false,
        }
    }

    /// No legend, one axis label; for small charts next to their own value readout.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// Scales the value axis in binary steps (bytes).
    pub fn binary_scale(mut self) -> Self {
        self.binary = true;
        self
    }

    /// Plot height in unscaled pixels (default 150).
    pub fn with_height(mut self, px: f32) -> Self {
        self.height = px;
        self
    }

    pub fn set_data(&mut self, data: ChartData, cx: &mut Context<Self>) {
        if self.data != data {
            self.data = data;
            cx.notify();
        }
    }

    pub fn data(&self) -> &ChartData {
        &self.data
    }

    /// Text shown while there is no data (loading, errors, "no samples").
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        if self.placeholder != text {
            self.placeholder = text;
            cx.notify();
        }
    }

    pub fn set_format(&mut self, format: impl Fn(f64) -> String + 'static) {
        self.format_value = Rc::new(format);
    }

    /// Hides or shows a series (legend click).
    pub fn toggle(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        if !self.hidden.remove(id) {
            self.hidden.insert(id.clone());
        }
        cx.notify();
    }

    pub fn is_hidden(&self, id: &SharedString) -> bool {
        self.hidden.contains(id)
    }

    fn visible(&self) -> Vec<Series> {
        self.data
            .series
            .iter()
            .filter(|s| !self.hidden.contains(&s.id))
            .cloned()
            .collect()
    }

    fn mouse_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let hover = self.plot.get().and_then(|bounds| {
            bounds
                .contains(&event.position)
                .then(|| f32::from(event.position.x - bounds.left()) / f32::from(bounds.size.width))
        });
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    fn tooltip(&self, index: usize, colors: &Colors) -> impl IntoElement {
        let time = self.data.times.get(index).copied().unwrap_or_default();
        let span = match (self.data.times.first(), self.data.times.last()) {
            (Some(first), Some(last)) => last - first,
            _ => 0.0,
        };
        let visible = self.visible();
        let mut total = 0.0;
        let rows: Vec<_> = visible
            .iter()
            .map(|s| {
                let value = s.values.get(index).copied().flatten();
                total += value.unwrap_or(0.0);
                h_flex()
                    .gap(u(6.0))
                    .child(swatch(s.color, s.dashed))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(colors.text_muted)
                            .child(s.name.clone()),
                    )
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_color(colors.text)
                            .child(value.map(|v| (self.format_value)(v)).unwrap_or("—".into())),
                    )
            })
            .collect();
        v_flex()
            .min_w(u(150.0))
            .max_w(u(240.0))
            .gap(u(3.0))
            .px(u(8.0))
            .py(u(6.0))
            .rounded(u(5.0))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated)
            .text_size(u(11.5))
            .shadow_md()
            .child(
                div()
                    .text_color(colors.text_dim)
                    .font_family(fonts::MONO)
                    .child(format_time(time, span)),
            )
            .children(rows)
            .when(
                self.kind == ChartKind::StackedArea && visible.len() > 1,
                |this| {
                    this.child(
                        h_flex()
                            .justify_between()
                            .pt(u(2.0))
                            .border_t_1()
                            .border_color(colors.border_variant)
                            .child(div().text_color(colors.text_dim).child("Total"))
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child((self.format_value)(total)),
                            ),
                    )
                },
            )
    }

    fn legend(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        h_flex()
            .flex_wrap()
            .gap_x(u(14.0))
            .gap_y(u(4.0))
            .mt(u(8.0))
            .children(self.data.series.iter().enumerate().map(|(i, s)| {
                let hidden = self.hidden.contains(&s.id);
                let id = s.id.clone();
                h_flex()
                    .id(("legend", i))
                    .gap(u(6.0))
                    .text_size(u(11.5))
                    .cursor_pointer()
                    .text_color(if hidden {
                        colors.text_faint
                    } else {
                        colors.text_muted
                    })
                    .hover(|this| this.text_color(colors.text))
                    .child(swatch(
                        if hidden { colors.text_faint } else { s.color },
                        s.dashed,
                    ))
                    .child(s.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(&id, cx)))
            }))
    }
}

/// The 10×2 line swatch of the legend (dashed for "other").
fn swatch(color: gpui::Hsla, dashed: bool) -> impl IntoElement {
    if dashed {
        h_flex()
            .flex_none()
            .w(u(10.0))
            .gap(u(2.0))
            .child(div().w(u(4.0)).h(u(2.0)).bg(color))
            .child(div().w(u(4.0)).h(u(2.0)).bg(color))
    } else {
        h_flex()
            .flex_none()
            .w(u(10.0))
            .child(div().w_full().h(u(2.0)).bg(color))
    }
}

/// `14:05` (or `Mon 14:05` past a day, `14:05:30` under an hour) in local time.
pub fn format_time(unix: f64, span: f64) -> String {
    let Ok(timestamp) = jiff::Timestamp::from_second(unix as i64) else {
        return String::new();
    };
    let zoned = timestamp.to_zoned(jiff::tz::TimeZone::system());
    let format = if span > 86_400.0 {
        "%a %H:%M"
    } else if span <= 3_600.0 {
        "%H:%M:%S"
    } else {
        "%H:%M"
    };
    zoned.strftime(format).to_string()
}

impl Render for LineChart {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let visible = self.visible();
        let len = self.data.times.len();
        let stacked = self.kind == ChartKind::StackedArea;
        let refs: Vec<&Series> = visible.iter().collect();
        let peak = max_value(&refs, len, stacked);
        let top = peak * 1.05;
        let max = if self.binary {
            nice_max_binary(top)
        } else {
            nice_max(top)
        };
        let empty = self.data.is_empty();
        let hover_index = self
            .hover
            .filter(|_| !empty)
            .and_then(|f| nearest_index(&self.data.times, f as f64));
        let hover_fraction = hover_index.map(|i| {
            if len > 1 {
                i as f32 / (len - 1) as f32
            } else {
                0.5
            }
        });

        let plot = self.plot.clone();
        let kind = self.kind;
        let grid_color = colors.border_variant;
        let cross_color = colors.border;
        let ring = colors.panel;
        let paint_series = visible.clone();
        let chart = canvas(
            move |bounds, _, _| plot.set(Some(bounds)),
            move |bounds, _, window, _| {
                for f in [0.25, 0.5, 0.75] {
                    let y = bounds.top() + bounds.size.height * f;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.left(), y),
                            size(bounds.size.width, gpui::px(1.0)),
                        ),
                        grid_color,
                    ));
                }
                if len == 0 {
                    return;
                }
                let line = scaled(window, 1.6);
                let dash = scaled(window, 3.0);
                let zero = gpui::px(0.0);
                let baseline: Vec<Point<Pixels>> = (0..len)
                    .map(|i| point(x_at(&bounds, i, len), bounds.bottom()))
                    .collect();
                let series: Vec<&Series> = paint_series.iter().collect();
                let tops: Vec<Vec<Option<Point<Pixels>>>> = if kind == ChartKind::StackedArea {
                    stack(&series, len)
                        .into_iter()
                        .map(|totals| {
                            totals
                                .iter()
                                .enumerate()
                                .map(|(i, v)| {
                                    Some(point(
                                        x_at(&bounds, i, len),
                                        y_at(&bounds, *v, 0.0, max, zero),
                                    ))
                                })
                                .collect()
                        })
                        .collect()
                } else {
                    series
                        .iter()
                        .map(|s| {
                            s.values
                                .iter()
                                .enumerate()
                                .map(|(i, v)| {
                                    v.map(|v| {
                                        point(
                                            x_at(&bounds, i, len),
                                            y_at(&bounds, v, 0.0, max, zero),
                                        )
                                    })
                                })
                                .collect()
                        })
                        .collect()
                };
                for (i, (s, top)) in series.iter().zip(&tops).enumerate() {
                    match kind {
                        ChartKind::Line => {}
                        ChartKind::Area => {
                            fill_between(top, &baseline, s.color.opacity(0.10), window)
                        }
                        ChartKind::StackedArea => {
                            let below: Vec<Point<Pixels>> = if i == 0 {
                                baseline.clone()
                            } else {
                                tops[i - 1].iter().map(|p| p.unwrap_or_default()).collect()
                            };
                            fill_between(top, &below, s.color.opacity(0.30), window);
                        }
                    }
                    stroke(top, s.color, line, s.dashed.then_some(dash), window);
                }
                if let Some(index) = hover_index {
                    let x = x_at(&bounds, index, len);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x, bounds.top()),
                            size(gpui::px(1.0), bounds.size.height),
                        ),
                        cross_color,
                    ));
                    let outer = scaled(window, 5.0);
                    let inner = scaled(window, 3.5);
                    for (s, top) in series.iter().zip(&tops) {
                        let Some(Some(center)) = top.get(index) else {
                            continue;
                        };
                        for (radius, color) in [(outer, ring), (inner, s.color)] {
                            window.paint_quad(
                                fill(
                                    Bounds::centered_at(*center, size(radius * 2.0, radius * 2.0)),
                                    color,
                                )
                                .corner_radii(radius),
                            );
                        }
                    }
                }
            },
        )
        .size_full();

        let label_at: &[f32] = if self.compact {
            &[0.75]
        } else {
            &[0.75, 0.5, 0.25]
        };
        let axis_labels = label_at.iter().map(|&f| {
            div()
                .absolute()
                .left(u(2.0))
                .top(relative(1.0 - f))
                .mt(u(-14.0))
                .text_size(u(10.5))
                .font_family(fonts::MONO)
                .text_color(colors.text_faint)
                .child((self.format_value)(max * f as f64))
        });

        v_flex()
            .w_full()
            .child(
                div()
                    .id("plot")
                    .relative()
                    .w_full()
                    .h(u(self.height))
                    .child(chart)
                    // All zeros (no drops, no errors): a flat line needs no scale.
                    .when(!empty && peak > 0.0, |this| this.children(axis_labels))
                    .when(empty, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child(self.placeholder.clone()),
                        )
                    })
                    .when_some(hover_index.zip(hover_fraction), |this, (index, f)| {
                        let tooltip = div()
                            .absolute()
                            .top(u(4.0))
                            .child(self.tooltip(index, &colors));
                        this.child(if f > 0.5 {
                            tooltip.right(relative(1.0 - f)).mr(u(10.0))
                        } else {
                            tooltip.left(relative(f)).ml(u(10.0))
                        })
                    })
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        this.mouse_moved(event, cx)
                    }))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !*hovered && this.hover.is_some() {
                            this.hover = None;
                            cx.notify();
                        }
                    })),
            )
            .when(!self.data.series.is_empty() && !self.compact, |this| {
                this.child(self.legend(cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[test]
    fn time_labels_depend_on_the_span() {
        let t = 1_790_000_000.0;
        assert_eq!(format_time(t, 900.0).len(), 8);
        assert_eq!(format_time(t, 6.0 * 3600.0).len(), 5);
        assert!(format_time(t, 7.0 * 86_400.0).contains(' '));
    }

    #[gpui::test]
    fn legend_toggles_survive_new_data(cx: &mut TestAppContext) {
        let chart = cx.new(|_| LineChart::new(ChartKind::Line, |v| format!("{v}")));
        let data = |v: f64| ChartData {
            times: vec![0.0, 1.0],
            series: vec![
                Series::new("a", "a", gpui::Hsla::default(), vec![Some(v), Some(v)]),
                Series::new("b", "b", gpui::Hsla::default(), vec![Some(1.0), None]),
            ],
        };
        chart.update(cx, |chart, cx| {
            chart.set_data(data(1.0), cx);
            chart.toggle(&"a".into(), cx);
            chart.set_data(data(2.0), cx);
            assert!(chart.is_hidden(&"a".into()));
            assert_eq!(chart.visible().len(), 1);
            chart.toggle(&"a".into(), cx);
            assert_eq!(chart.visible().len(), 2);
        });
    }
}
