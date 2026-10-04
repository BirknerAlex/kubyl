//! Chart math without colors: aligning samples to a time grid, axis bounds, top-n series.

use kubyl_base::SharedString;

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
