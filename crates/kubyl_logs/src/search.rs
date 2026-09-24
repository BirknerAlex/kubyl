//! Text/regex search over the ring buffer: match count, next/prev, "filter to matches".

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

    /// Indices (into `lines`) of every matching line, in order.
    pub fn find_all(&self, lines: &[&LogLine]) -> Vec<usize> {
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| self.matches(&l.text))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Cursor over a set of match indices: next/prev with wraparound.
#[derive(Default)]
pub struct MatchCursor {
    matches: Vec<usize>,
    current: Option<usize>,
}

impl MatchCursor {
    pub fn set_matches(&mut self, matches: Vec<usize>) {
        self.current = if matches.is_empty() { None } else { Some(0) };
        self.matches = matches;
    }

    pub fn count(&self) -> usize {
        self.matches.len()
    }

    /// 1-based position of the current match, for "3 / 12" style labels.
    pub fn position(&self) -> Option<usize> {
        self.current.map(|c| c + 1)
    }

    pub fn current_line(&self) -> Option<usize> {
        self.current.map(|c| self.matches[c])
    }

    pub fn go_next(&mut self) -> Option<usize> {
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

    pub fn go_prev(&mut self) -> Option<usize> {
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
}
