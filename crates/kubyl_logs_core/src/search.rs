//! Text/regex search over the ring buffer: match count, next/prev, highlight ranges, "filter to
//! matches".

use std::collections::VecDeque;
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use crate::line::LogLine;

/// A compiled search query. Plain text is matched literally (as an escaped regex), so both
/// modes share matching and highlighting.
pub struct Search {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    compiled: Regex,
}

impl Search {
    /// Builds a search. `regex: false` matches `query` literally. Returns `Err` for an invalid
    /// pattern.
    pub fn new(query: &str, regex: bool, case_sensitive: bool) -> Result<Self, String> {
        let pattern = if regex {
            query.to_string()
        } else {
            regex::escape(query)
        };
        let compiled = RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            query: query.to_string(),
            regex,
            case_sensitive,
            compiled,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty()
    }

    pub fn matches(&self, text: &str) -> bool {
        !self.query.is_empty() && self.compiled.is_match(text)
    }

    /// Byte ranges of every (non-empty) match in `text`, for highlighting.
    pub fn ranges(&self, text: &str) -> Vec<Range<usize>> {
        if self.query.is_empty() {
            return Vec::new();
        }
        self.compiled
            .find_iter(text)
            .filter(|m| !m.is_empty())
            .map(|m| m.range())
            .collect()
    }

    /// Seqs (the ring buffer's stable per-line identity) of every matching line, in order.
    pub fn find_all(&self, lines: &[&LogLine]) -> Vec<u64> {
        lines
            .iter()
            .filter(|l| !l.marker && self.matches(&l.text))
            .map(|l| l.seq)
            .collect()
    }
}

/// Cursor over a set of matching lines (identified by their ring buffer `seq`, not a
/// recompute-volatile index): next/prev with wraparound.
///
/// Matches are keyed by `seq` rather than position so that appending a newly-arrived matching
/// line (`push_back`) or dropping an evicted one (`remove_front_if`) doesn't require rebuilding
/// the whole set and doesn't disturb the user's current match position — only an actual
/// query/filter change (`set_matches`) resets navigation.
#[derive(Default)]
pub struct MatchCursor {
    matches: VecDeque<u64>,
    current: Option<usize>,
}

impl MatchCursor {
    /// Replaces the match set outright and puts the cursor on the last match (the newest line,
    /// where a following log view is). Use this only when the query or a filter changes.
    pub fn set_matches(&mut self, matches: Vec<u64>) {
        self.current = matches.len().checked_sub(1);
        self.matches = matches.into();
    }

    /// Appends a newly-arrived matching line without disturbing `current`.
    pub fn push_back(&mut self, seq: u64) {
        self.matches.push_back(seq);
        if self.current.is_none() {
            self.current = Some(self.matches.len() - 1);
        }
    }

    /// Drops `seq` from the front of the match set if it's there (i.e. the ring buffer just
    /// evicted it), adjusting `current` to stay valid.
    pub fn remove_front_if(&mut self, seq: u64) {
        if self.matches.front() != Some(&seq) {
            return;
        }
        self.matches.pop_front();
        if let Some(current) = self.current {
            self.current = if self.matches.is_empty() {
                None
            } else {
                Some(current.saturating_sub(1))
            };
        }
    }

    pub fn count(&self) -> usize {
        self.matches.len()
    }

    /// 1-based position of the current match, for "3 of 12" style labels.
    pub fn position(&self) -> Option<usize> {
        self.current.map(|c| c + 1)
    }

    pub fn current_line(&self) -> Option<u64> {
        self.current.and_then(|c| self.matches.get(c).copied())
    }

    pub fn go_next(&mut self) -> Option<u64> {
        if self.matches.is_empty() {
            return None;
        }
        let next = match self.current {
            Some(c) => (c + 1) % self.matches.len(),
            None => 0,
        };
        self.current = Some(next);
        self.current_line()
    }

    pub fn go_prev(&mut self) -> Option<u64> {
        if self.matches.is_empty() {
            return None;
        }
        let prev = match self.current {
            Some(0) | None => self.matches.len() - 1,
            Some(c) => c - 1,
        };
        self.current = Some(prev);
        self.current_line()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(texts: &[&str]) -> Vec<LogLine> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| LogLine::new(i as u64, "pod".into(), "c".into(), None, t.to_string()))
            .collect()
    }

    #[test]
    fn plain_text_search_counts_and_is_case_insensitive_by_default() {
        let lines = lines(&["a Timeout happened", "all good", "another timeout"]);
        let refs: Vec<&LogLine> = lines.iter().collect();
        let search = Search::new("timeout", false, false).unwrap();
        assert_eq!(search.find_all(&refs), [0, 2]);
    }

    #[test]
    fn plain_text_is_literal() {
        let search = Search::new("a.b(", false, false).unwrap();
        assert!(search.matches("x a.b( y"));
        assert!(!search.matches("axb("));
    }

    #[test]
    fn case_sensitive_search_narrows_matches() {
        let lines = lines(&["Timeout", "timeout"]);
        let refs: Vec<&LogLine> = lines.iter().collect();
        let search = Search::new("timeout", false, true).unwrap();
        assert_eq!(search.find_all(&refs), [1]);
    }

    #[test]
    fn regex_search_compiles_and_matches() {
        let lines = lines(&["error code=500", "error code=404", "ok"]);
        let refs: Vec<&LogLine> = lines.iter().collect();
        let search = Search::new(r"code=\d{3}", true, false).unwrap();
        assert_eq!(search.find_all(&refs), [0, 1]);
    }

    #[test]
    fn ranges_cover_every_match() {
        let search = Search::new("timeout", false, false).unwrap();
        assert_eq!(
            search.ranges("Timeout then timeout"),
            [0..7, 13..20].to_vec()
        );
        let empty = Search::new("x*", true, false).unwrap();
        assert!(empty.ranges("abc").is_empty());
    }

    #[test]
    fn invalid_regex_is_rejected() {
        assert!(Search::new("(", true, false).is_err());
    }

    #[test]
    fn match_cursor_starts_at_the_newest_and_wraps() {
        let mut cursor = MatchCursor::default();
        cursor.set_matches(vec![3, 7, 9]);
        assert_eq!(cursor.current_line(), Some(9));
        assert_eq!(cursor.go_next(), Some(3));
        assert_eq!(cursor.go_next(), Some(7));
        assert_eq!(cursor.go_prev(), Some(3));
        assert_eq!(cursor.go_prev(), Some(9));
        assert_eq!(cursor.position(), Some(3));
    }

    #[test]
    fn push_back_does_not_disturb_current_position() {
        let mut cursor = MatchCursor::default();
        cursor.set_matches(vec![1, 2, 3]);
        cursor.go_prev(); // seq 2
        assert_eq!(cursor.position(), Some(2));
        cursor.push_back(4);
        cursor.push_back(5);
        assert_eq!(cursor.position(), Some(2));
        assert_eq!(cursor.current_line(), Some(2));
        assert_eq!(cursor.count(), 5);
    }

    #[test]
    fn remove_front_if_shifts_current_and_ignores_non_front_seqs() {
        let mut cursor = MatchCursor::default();
        cursor.set_matches(vec![10, 20, 30]);
        cursor.go_next(); // wraps to seq 10
        cursor.go_next(); // seq 20
        cursor.remove_front_if(20);
        assert_eq!(cursor.count(), 3);
        cursor.remove_front_if(10);
        assert_eq!(cursor.count(), 2);
        assert_eq!(cursor.current_line(), Some(20));
    }
}
