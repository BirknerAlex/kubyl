//! Text/regex search over the ring buffer: match count, next/prev, "filter to matches".

use std::collections::VecDeque;

use regex::{Regex, RegexBuilder};

use crate::line::LogLine;

/// A compiled search query.
pub struct Search {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    compiled: Option<Regex>,
}

impl Search {
    /// Builds a search. For plain-text mode the query is matched as a substring; `regex: true`
    /// compiles `query` as a regular expression. Returns `Err` for an invalid pattern.
    pub fn new(query: &str, regex: bool, case_sensitive: bool) -> Result<Self, String> {
        let compiled = if regex && !query.is_empty() {
            Some(
                RegexBuilder::new(query)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
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
        if self.query.is_empty() {
            return false;
        }
        if let Some(re) = &self.compiled {
            re.is_match(text)
        } else if self.case_sensitive {
            text.contains(&self.query)
        } else {
            text.to_lowercase().contains(&self.query.to_lowercase())
        }
    }

    /// Seqs (the ring buffer's stable per-line identity) of every matching line, in order.
    pub fn find_all(&self, lines: &[&LogLine]) -> Vec<u64> {
        lines
            .iter()
            .filter(|l| self.matches(&l.text))
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
/// query/filter change (`set_matches`) resets navigation back to the first match.
#[derive(Default)]
pub struct MatchCursor {
    matches: VecDeque<u64>,
    current: Option<usize>,
}

impl MatchCursor {
    /// Replaces the match set outright and resets navigation to the first match. Use this only
    /// when the search query/regex/case-sensitivity or the level filter actually changes; for
    /// new data arriving under an unchanged query, use `push_back`/`remove_front_if` instead.
    pub fn set_matches(&mut self, matches: Vec<u64>) {
        self.current = if matches.is_empty() { None } else { Some(0) };
        self.matches = matches.into();
    }

    /// Appends a newly-arrived matching line without disturbing `current`.
    pub fn push_back(&mut self, seq: u64) {
        self.matches.push_back(seq);
        if self.current.is_none() {
            self.current = Some(0);
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

    /// 1-based position of the current match, for "3 / 12" style labels.
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
            .map(|(i, t)| LogLine::new(i as u64, "pod".into(), "c".into(), t.to_string()))
            .collect()
    }

    #[test]
    fn plain_text_search_counts_and_is_case_insensitive_by_default() {
        let lines = lines(&["a Timeout happened", "all good", "another timeout"]);
        let refs: Vec<&LogLine> = lines.iter().collect();
        let search = Search::new("timeout", false, false).unwrap();
        let matches = search.find_all(&refs);
        assert_eq!(matches, [0, 2]);
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
    fn invalid_regex_is_rejected() {
        assert!(Search::new("(", true, false).is_err());
    }

    #[test]
    fn match_cursor_wraps_both_directions() {
        let mut cursor = MatchCursor::default();
        cursor.set_matches(vec![3, 7, 9]);
        assert_eq!(cursor.current_line(), Some(3));
        assert_eq!(cursor.go_next(), Some(7));
        assert_eq!(cursor.go_next(), Some(9));
        assert_eq!(cursor.go_next(), Some(3));
        assert_eq!(cursor.go_prev(), Some(9));
        assert_eq!(cursor.position(), Some(3));
    }

    #[test]
    fn push_back_does_not_disturb_current_position() {
        // Simulates new matching lines streaming in while the user has navigated to the 2nd of
        // 3 matches: appending more matches (as a batch arrives) must not reset `current`.
        let mut cursor = MatchCursor::default();
        cursor.set_matches(vec![1, 2, 3]);
        cursor.go_next(); // now on seq 2 (position 2)
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
        cursor.go_next(); // current = 1 (seq 20)

        // Not the front: no-op.
        cursor.remove_front_if(20);
        assert_eq!(cursor.count(), 3);

        // Evicting the front match shifts current down by one so it still points at seq 20.
        cursor.remove_front_if(10);
        assert_eq!(cursor.count(), 2);
        assert_eq!(cursor.current_line(), Some(20));
    }
}
