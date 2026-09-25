//! Path painting shared by the sparkline and the line chart.

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, Window, point, px};

/// Scales `u(px)` sizes to window pixels (they follow the UI zoom).
pub(crate) fn scaled(window: &Window, px_at_default: f32) -> Pixels {
    px(px_at_default * f32::from(window.rem_size()) / 16.0)
}

/// Strokes the polyline through `points`; `None` breaks it (a gap in the data).
pub(crate) fn stroke(
    points: &[Option<Point<Pixels>>],
    color: Hsla,
    width: Pixels,
    dash: Option<Pixels>,
    window: &mut Window,
) {
    let mut builder = PathBuilder::stroke(width);
    if let Some(dash) = dash {
        builder = builder.dash_array(&[dash, dash]);
    }
    let mut any = false;
    for segment in segments(points) {
        if segment.len() == 1 {
            // A lone sample: a short tick so it doesn't vanish.
            let p = segment[0];
            builder.move_to(point(p.x - width, p.y));
            builder.line_to(point(p.x + width, p.y));
        } else {
            builder.move_to(segment[0]);
            for p in &segment[1..] {
                builder.line_to(*p);
            }
        }
        any = true;
    }
    if any && let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Fills the area between `top` and `bottom` (the baseline or the series below, when stacked),
/// segment by segment so gaps stay empty.
pub(crate) fn fill_between(
    top: &[Option<Point<Pixels>>],
    bottom: &[Point<Pixels>],
    color: Hsla,
    window: &mut Window,
) {
    let mut builder = PathBuilder::fill();
    let mut any = false;
    let mut start = 0;
    while start < top.len() {
        if top[start].is_none() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < top.len() && top[end].is_some() {
            end += 1;
        }
        if end - start >= 2 {
            builder.move_to(top[start].unwrap_or_default());
            for p in top[start + 1..end].iter().flatten() {
                builder.line_to(*p);
            }
            for p in bottom[start..end].iter().rev() {
                builder.line_to(*p);
            }
            builder.close();
            any = true;
        }
        start = end;
    }
    if any && let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Runs of consecutive points.
fn segments(points: &[Option<Point<Pixels>>]) -> Vec<Vec<Point<Pixels>>> {
    let mut out = Vec::new();
    let mut current = Vec::new();
    for p in points {
        match p {
            Some(p) => current.push(*p),
            None if !current.is_empty() => out.push(std::mem::take(&mut current)),
            None => {}
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// X position of sample `index` of `count`, spread over the bounds' width.
pub(crate) fn x_at(bounds: &Bounds<Pixels>, index: usize, count: usize) -> Pixels {
    if count <= 1 {
        return bounds.left() + bounds.size.width / 2.0;
    }
    bounds.left() + bounds.size.width * (index as f32 / (count - 1) as f32)
}

/// Y position of `value` on a `min..max` scale, with `pad` pixels kept free at top and bottom.
pub(crate) fn y_at(bounds: &Bounds<Pixels>, value: f64, min: f64, max: f64, pad: Pixels) -> Pixels {
    let range = max - min;
    let fraction = if range > 0.0 {
        ((value - min) / range).clamp(0.0, 1.0) as f32
    } else {
        0.5
    };
    bounds.bottom() - pad - (bounds.size.height - pad * 2.0) * fraction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_gaps() {
        let p = |x: f32| Some(point(px(x), px(0.0)));
        let runs = segments(&[p(0.0), p(1.0), None, None, p(3.0), None]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len(), 2);
        assert_eq!(runs[1].len(), 1);
    }

    #[test]
    fn scales_values() {
        let bounds = Bounds::new(point(px(10.0), px(0.0)), gpui::size(px(100.0), px(50.0)));
        assert_eq!(x_at(&bounds, 0, 3), px(10.0));
        assert_eq!(x_at(&bounds, 2, 3), px(110.0));
        assert_eq!(y_at(&bounds, 0.0, 0.0, 10.0, px(0.0)), px(50.0));
        assert_eq!(y_at(&bounds, 10.0, 0.0, 10.0, px(0.0)), px(0.0));
        assert_eq!(y_at(&bounds, 5.0, 5.0, 5.0, px(0.0)), px(25.0));
    }
}
