//! Chart data and the pure math behind drawing it (scales, stacking, hover lookup).

use gpui::{Hsla, SharedString};

/// One series, aligned to [`ChartData::times`]. `None` is a gap (no sample at that time).
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// Stable identity (namespace, pod…): legend toggles follow it across refreshes.
    pub id: SharedString,
    pub name: SharedString,
    pub color: Hsla,
    /// Drawn dashed; used for the folded "other" series.
    pub dashed: bool,
    pub values: Vec<Option<f64>>,
}

impl Series {
    pub fn new(
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        color: Hsla,
        values: Vec<Option<f64>>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            color,
            dashed: false,
            values,
        }
    }

    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }

    /// The newest value that isn't a gap.
    pub fn last(&self) -> Option<f64> {
        self.values.iter().rev().find_map(|v| *v)
    }
}

/// Aligned samples: every series has one value (or gap) per timestamp.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartData {
    /// Unix seconds, ascending.
    pub times: Vec<f64>,
    pub series: Vec<Series>,
}

impl ChartData {
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
            || self
                .series
                .iter()
                .all(|s| s.values.iter().all(Option::is_none))
    }
}

/// Aligns `(time, value)` samples to the grid `start, start + step, … ≤ end`. A sample lands on
/// the nearest grid point within half a step; the rest of the grid stays a gap.
pub fn align(samples: &[(f64, f64)], start: f64, end: f64, step: f64) -> Vec<Option<f64>> {
    if step <= 0.0 || end < start {
        return Vec::new();
    }
    let count = ((end - start) / step).floor() as usize + 1;
    let mut values = vec![None; count];
    for &(time, value) in samples {
        let index = ((time - start) / step).round();
        if index < 0.0 || !value.is_finite() {
            continue;
        }
        let index = index as usize;
        if index < count && (start + index as f64 * step - time).abs() <= step / 2.0 {
            values[index] = Some(value);
        }
    }
    values
}

/// The grid [`align`] uses: `start, start + step, … ≤ end`.
pub fn grid(start: f64, end: f64, step: f64) -> Vec<f64> {
    if step <= 0.0 || end < start {
        return Vec::new();
    }
    let count = ((end - start) / step).floor() as usize + 1;
    (0..count).map(|i| start + i as f64 * step).collect()
}

/// A "nice" upper bound for an axis (1, 2, 2.5 or 5 × 10ⁿ) at or above `max`.
pub fn nice_max(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let magnitude = 10f64.powf(max.log10().floor());
    [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|candidate| *candidate >= max * (1.0 - 1e-9))
        .unwrap_or(10.0 * magnitude)
}

/// Like [`nice_max`], in binary units (1, 2, 2.5, 5 × 1024ⁿ…), so byte axes read `512Mi`,
/// `1Gi`, `1.5Gi` instead of `954Mi`.
pub fn nice_max_binary(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let unit = 1024f64.powf(max.log(1024.0).floor().max(0.0));
    nice_max(max / unit) * unit
}

/// Running sums of the visible series, bottom to top, for stacked areas. Gaps count as zero.
pub fn stack(series: &[&Series], len: usize) -> Vec<Vec<f64>> {
    let mut totals = vec![0.0; len];
    series
        .iter()
        .map(|s| {
            for (i, total) in totals.iter_mut().enumerate() {
                *total += s.values.get(i).copied().flatten().unwrap_or(0.0);
            }
            totals.clone()
        })
        .collect()
}

/// Largest value to fit on the y axis: per point for lines, the stack total for stacked areas.
pub fn max_value(series: &[&Series], len: usize, stacked: bool) -> f64 {
    if stacked {
        stack(series, len)
            .last()
            .map(|top| top.iter().copied().fold(0.0, f64::max))
            .unwrap_or(0.0)
    } else {
        series
            .iter()
            .flat_map(|s| s.values.iter().flatten().copied())
            .fold(0.0, f64::max)
    }
}

/// Index of the timestamp closest to `fraction` (0 = left edge, 1 = right edge) of the time span.
pub fn nearest_index(times: &[f64], fraction: f64) -> Option<usize> {
    let (first, last) = (*times.first()?, *times.last()?);
    let target = first + (last - first) * fraction.clamp(0.0, 1.0);
    times
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - target)
                .abs()
                .partial_cmp(&(*b - target).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// A named series of aligned values, before colors are assigned.
pub type Named = (SharedString, Vec<Option<f64>>);

/// Picks the `n` series with the largest sum (largest first) and folds the rest into one
/// "other" series, which is `None` when nothing was folded.
pub fn top_n(series: Vec<Named>, n: usize) -> (Vec<Named>, Option<Vec<Option<f64>>>) {
    let total = |values: &[Option<f64>]| values.iter().flatten().sum::<f64>();
    let mut ranked: Vec<(usize, f64)> = series
        .iter()
        .enumerate()
        .map(|(i, (_, values))| (i, total(values)))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let keep: std::collections::HashSet<usize> = ranked.iter().take(n).map(|(i, _)| *i).collect();
    let len = series.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
    let mut other: Option<Vec<Option<f64>>> = None;
    let mut top = Vec::new();
    for (i, (name, values)) in series.into_iter().enumerate() {
        if keep.contains(&i) {
            top.push((name, values));
            continue;
        }
        let sum = other.get_or_insert_with(|| vec![None; len]);
        for (slot, value) in sum.iter_mut().zip(values) {
            if let Some(value) = value {
                *slot = Some(slot.unwrap_or(0.0) + value);
            }
        }
    }
    // Largest first, like the legend reads.
    top.sort_by(|a, b| {
        total(&b.1)
            .partial_cmp(&total(&a.1))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    (top, other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_samples_to_the_grid() {
        let values = align(
            &[(0.0, 1.0), (31.0, 2.0), (90.0, 4.0), (500.0, 9.0)],
            0.0,
            90.0,
            30.0,
        );
        assert_eq!(values, vec![Some(1.0), Some(2.0), None, Some(4.0)]);
        assert_eq!(grid(0.0, 90.0, 30.0), vec![0.0, 30.0, 60.0, 90.0]);
        assert!(align(&[], 10.0, 0.0, 5.0).is_empty());
    }

    #[test]
    fn nice_maxima() {
        assert_eq!(nice_max(0.0), 1.0);
        assert_eq!(nice_max(0.83), 1.0);
        assert_eq!(nice_max(1.2), 2.0);
        assert_eq!(nice_max(2.2), 2.5);
        assert_eq!(nice_max(3.0), 5.0);
        assert_eq!(nice_max(7.5), 10.0);
        assert_eq!(nice_max(250.0), 250.0);
        assert_eq!(nice_max(1.5e9), 2e9);
        let gi = 1024.0 * 1024.0 * 1024.0;
        assert_eq!(nice_max_binary(1.4 * gi), 2.0 * gi);
        assert_eq!(nice_max_binary(0.3 * gi), 500.0 * 1024.0 * 1024.0);
        assert_eq!(nice_max_binary(0.5), 0.5);
    }

    #[test]
    fn stacks_and_maxima() {
        let a = Series::new("a", "a", Hsla::default(), vec![Some(1.0), None, Some(3.0)]);
        let b = Series::new(
            "b",
            "b",
            Hsla::default(),
            vec![Some(2.0), Some(2.0), Some(1.0)],
        );
        assert_eq!(
            stack(&[&a, &b], 3),
            vec![vec![1.0, 0.0, 3.0], vec![3.0, 2.0, 4.0]]
        );
        assert_eq!(max_value(&[&a, &b], 3, true), 4.0);
        assert_eq!(max_value(&[&a, &b], 3, false), 3.0);
        assert_eq!(a.last(), Some(3.0));
    }

    #[test]
    fn nearest_timestamp() {
        let times = [0.0, 10.0, 20.0, 30.0];
        assert_eq!(nearest_index(&times, 0.0), Some(0));
        assert_eq!(nearest_index(&times, 0.4), Some(1));
        assert_eq!(nearest_index(&times, 0.6), Some(2));
        assert_eq!(nearest_index(&times, 2.0), Some(3));
        assert_eq!(nearest_index(&[], 0.5), None);
    }

    #[test]
    fn folds_small_series_into_other() {
        let s = |name: &str, v: f64| (SharedString::from(name.to_string()), vec![Some(v), None]);
        let (top, other) = top_n(vec![s("a", 1.0), s("b", 5.0), s("c", 3.0), s("d", 2.0)], 2);
        let names: Vec<&str> = top.iter().map(|(n, _)| n.as_ref()).collect();
        assert_eq!(names, ["b", "c"]);
        assert_eq!(other, Some(vec![Some(3.0), None]));
        let (top, other) = top_n(vec![s("a", 1.0)], 4);
        assert_eq!(top.len(), 1);
        assert_eq!(other, None);
    }
}
