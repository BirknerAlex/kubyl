//! Row bookkeeping of a list: identities, sort keys and natural ordering.

use std::cmp::Ordering;
use std::sync::Arc;

use kubyl_core::CellValue;
use kubyl_resources::ObjectKey;
use serde_json::Value;

/// Identifies a row across updates: the source (store) index and the object key.
pub type RowId = (usize, ObjectKey);

/// One visible row.
#[derive(Clone, Debug)]
pub struct Row {
    pub source: usize,
    pub key: ObjectKey,
    pub object: Arc<Value>,
}

impl Row {
    pub fn id(&self) -> RowId {
        (self.source, self.key.clone())
    }
}

/// A comparable value for sorting.
#[derive(Clone, Debug, PartialEq)]
pub enum SortKey {
    Text(String),
    Number(f64),
    Missing,
}

impl SortKey {
    pub fn compare(&self, other: &SortKey) -> Ordering {
        match (self, other) {
            (SortKey::Text(a), SortKey::Text(b)) => natural_cmp(a, b),
            (SortKey::Number(a), SortKey::Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (SortKey::Missing, SortKey::Missing) => Ordering::Equal,
            // Missing values sort last in ascending order.
            (SortKey::Missing, _) => Ordering::Greater,
            (_, SortKey::Missing) => Ordering::Less,
            (SortKey::Number(_), SortKey::Text(_)) => Ordering::Less,
            (SortKey::Text(_), SortKey::Number(_)) => Ordering::Greater,
        }
    }

    /// The sort key of a rendered cell: numbers (`12`, `3/4`, `184m`) sort numerically.
    pub fn of_cell(cell: &CellValue) -> SortKey {
        let label = match cell {
            CellValue::Text(s) => s.as_ref(),
            CellValue::Tinted { label, .. } | CellValue::Status { label, .. } => label.as_ref(),
            CellValue::Usage { label, .. } => {
                return kubyl_resources::format::parse_quantity(label)
                    .map(SortKey::Number)
                    .unwrap_or_else(|| SortKey::Text(label.to_string()));
            }
            CellValue::Buttons(buttons) => {
                return buttons
                    .first()
                    .map(|b| SortKey::Text(b.label.to_string()))
                    .unwrap_or(SortKey::Missing);
            }
            CellValue::Empty => return SortKey::Missing,
        };
        SortKey::Text(label.to_string())
    }
}

/// Compares strings with embedded numbers numerically (`pod-2` < `pod-10`), case-insensitively.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut left = String::new();
                while let Some(c) = a.peek().copied().filter(char::is_ascii_digit) {
                    left.push(c);
                    a.next();
                }
                let mut right = String::new();
                while let Some(c) = b.peek().copied().filter(char::is_ascii_digit) {
                    right.push(c);
                    b.next();
                }
                let left = left.trim_start_matches('0');
                let right = right.trim_start_matches('0');
                let ordering = left.len().cmp(&right.len()).then_with(|| left.cmp(right));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(x), Some(y)) => {
                let ordering = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// One segment of a [`NameKey`]: a run of digits (compared by value) or of other characters
/// (compared case-insensitively).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Segment {
    /// Digits without leading zeros, compared by length first.
    Number(usize, String),
    Text(String),
}

/// A precomputed natural sort key: `pod-10` sorts after `pod-2`. Computing it once per object
/// keeps sorting 5,000 rows cheap.
pub type NameKey = (Vec<Segment>, String);

pub fn natural_key(name: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut digits = false;
    for c in name.chars() {
        let is_digit = c.is_ascii_digit();
        if !current.is_empty() && is_digit != digits {
            segments.push(segment(std::mem::take(&mut current), digits));
        }
        digits = is_digit;
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        segments.push(segment(current, digits));
    }
    segments
}

fn segment(text: String, digits: bool) -> Segment {
    if digits {
        let trimmed = text.trim_start_matches('0').to_string();
        Segment::Number(trimmed.len(), trimmed)
    } else {
        Segment::Text(text)
    }
}

/// Name, then namespace.
pub fn name_key(object: &Value) -> NameKey {
    (
        natural_key(kubyl_resources::format::name(object)),
        kubyl_resources::format::namespace(object)
            .unwrap_or_default()
            .to_string(),
    )
}

/// Where the selection goes after the rows changed: the same row if it still exists, else the
/// row now at its old position.
pub fn keep_selection(
    rows: &[Row],
    selected: Option<&RowId>,
    old_index: Option<usize>,
) -> Option<usize> {
    let selected = selected?;
    rows.iter()
        .position(|r| r.source == selected.0 && r.key == selected.1)
        .or_else(|| {
            old_index
                .filter(|_| !rows.is_empty())
                .map(|i| i.min(rows.len() - 1))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_ordering() {
        let mut names = vec!["pod-10", "pod-2", "Pod-1", "pod-02b", "api"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["api", "Pod-1", "pod-2", "pod-02b", "pod-10"]);
    }

    #[test]
    fn natural_keys_match_natural_cmp() {
        let mut names = vec!["pod-10", "pod-2", "Pod-1", "pod-02b", "api", "pod"];
        let mut by_key = names.clone();
        names.sort_by(|a, b| natural_cmp(a, b));
        by_key.sort_by_key(|n| natural_key(n));
        assert_eq!(names, by_key);
    }

    #[test]
    fn cells_sort_by_number_when_possible() {
        let a = SortKey::of_cell(&CellValue::Usage {
            label: "184m".into(),
            percent: 0.0,
        });
        let b = SortKey::of_cell(&CellValue::Usage {
            label: "1".into(),
            percent: 0.0,
        });
        assert_eq!(a.compare(&b), Ordering::Less);
        let restarts = |n: &str| SortKey::of_cell(&CellValue::Text(n.to_string().into()));
        assert_eq!(restarts("9").compare(&restarts("14")), Ordering::Less);
        assert_eq!(SortKey::Missing.compare(&restarts("1")), Ordering::Greater);
    }

    #[test]
    fn selection_follows_the_row_or_its_position() {
        let row = |name: &str| Row {
            source: 0,
            key: name.into(),
            object: Arc::new(Value::Null),
        };
        let rows = vec![row("a"), row("c")];
        assert_eq!(
            keep_selection(&rows, Some(&(0, "c".into())), Some(2)),
            Some(1)
        );
        assert_eq!(
            keep_selection(&rows, Some(&(0, "b".into())), Some(1)),
            Some(1)
        );
        assert_eq!(
            keep_selection(&rows, Some(&(0, "z".into())), Some(9)),
            Some(1)
        );
        assert_eq!(keep_selection(&[], Some(&(0, "z".into())), Some(1)), None);
        assert_eq!(keep_selection(&rows, None, None), None);
    }
}
