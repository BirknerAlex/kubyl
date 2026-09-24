//! Fuzzy matching (`nucleo-matcher`) with the bonuses the palette needs: exact aliases first
//! (`:po` is pods, `:cert` is certificates), then prefixes, then fuzzy matches.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Added when the query equals one of the candidate's keys (name, alias, short name).
pub const EXACT_BONUS: u32 = 1000;
/// Added when the title starts with the query.
pub const PREFIX_BONUS: u32 = 200;
/// Weakest fuzzy match kept, per query character. Contiguous matches score ~70 per character;
/// matches scattered over a long name (`cert` in `customresourcedefinitions`) score less than 18.
const MIN_SCORE_PER_CHAR: u32 = 18;

/// A query, ready to score many candidates.
pub struct Query {
    text: String,
    pattern: Pattern,
    matcher: Matcher,
    buf: Vec<char>,
    /// Only substring matches count (for secondary groups).
    strict: bool,
}

/// The score of a match and the matched character positions in the title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Match {
    pub score: u32,
    /// Char indices into the title, sorted and unique.
    pub positions: Vec<usize>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        let text = text.trim().to_lowercase();
        Self {
            pattern: Pattern::new(
                &text,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            ),
            text,
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
            strict: false,
        }
    }

    /// Strict queries only match candidates that contain the query as a substring.
    pub fn set_strict(&mut self, strict: bool) {
        self.strict = strict;
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Scores `title` plus other keys the candidate answers to (aliases, short names…).
    /// `None` when nothing matches. An empty query matches everything with score 0.
    pub fn score(&mut self, title: &str, keys: &[&str]) -> Option<Match> {
        if self.is_empty() {
            return Some(Match::default());
        }
        if self.strict
            && !std::iter::once(title)
                .chain(keys.iter().copied())
                .any(|k| k.to_lowercase().contains(&self.text))
        {
            return None;
        }
        let mut indices = Vec::new();
        let title_score = self.pattern.indices(
            Utf32Str::new(title, &mut self.buf),
            &mut self.matcher,
            &mut indices,
        );
        let mut best = title_score;
        for key in keys {
            let score = self
                .pattern
                .score(Utf32Str::new(key, &mut self.buf), &mut self.matcher);
            best = best.max(score);
        }
        let mut score = best?;
        if score < MIN_SCORE_PER_CHAR * self.text.chars().count() as u32 {
            return None;
        }
        let lower = title.to_lowercase();
        if lower == self.text || keys.iter().any(|k| k.eq_ignore_ascii_case(&self.text)) {
            score += EXACT_BONUS;
        } else if lower.starts_with(&self.text) {
            score += PREFIX_BONUS;
        }
        let positions = if title_score.is_some() {
            indices.sort_unstable();
            indices.dedup();
            indices.into_iter().map(|i| i as usize).collect()
        } else {
            Vec::new()
        };
        Some(Match { score, positions })
    }
}

/// Byte ranges of the chars at `positions` in `text`, merged where adjacent (for highlights).
pub fn byte_ranges(text: &str, positions: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (ix, (start, ch)) in text.char_indices().enumerate() {
        if positions.binary_search(&ix).is_err() {
            continue;
        }
        let end = start + ch.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_aliases_beat_fuzzy_matches() {
        let mut query = Query::new("cert");
        let certificates = query
            .score("certificates", &["cert", "certs", "certificate"])
            .unwrap();
        let requests = query.score("certificaterequests", &["cr", "crs"]).unwrap();
        let csr = query.score("certificatesigningrequests", &["csr"]).unwrap();
        assert!(certificates.score > requests.score);
        assert!(certificates.score > csr.score);
        assert_eq!(certificates.positions, vec![0, 1, 2, 3]);
        assert!(query.score("pods", &["po"]).is_none());
        // Scattered matches are dropped, partial ones kept.
        assert!(query.score("customresourcedefinitions", &[]).is_none());
        assert!(query.score("podcertificaterequests", &[]).is_some());
    }

    #[test]
    fn short_names_match_without_highlighting_the_title() {
        let mut query = Query::new("po");
        let pods = query.score("pods", &["po", "pod"]).unwrap();
        let policies = query.score("podsecuritypolicies", &["psp"]).unwrap();
        assert!(pods.score > policies.score);
        let mut query = Query::new("deploy");
        let deployments = query.score("deployments", &["deploy"]).unwrap();
        assert!(deployments.score >= EXACT_BONUS);
    }

    #[test]
    fn strict_queries_need_a_substring() {
        let mut query = Query::new("cert");
        query.set_strict(true);
        assert!(query.score("Clusters: Switch Cluster…", &[]).is_none());
        assert!(query.score("Certificate: Renew", &[]).is_some());
        assert!(query.score("payments", &["certificates"]).is_some());
    }

    #[test]
    fn empty_query_matches_everything() {
        let mut query = Query::new("  ");
        assert!(query.is_empty());
        assert_eq!(query.score("anything", &[]).unwrap().score, 0);
    }

    #[test]
    fn byte_ranges_merge_adjacent_chars() {
        assert_eq!(byte_ranges("certificates", &[0, 1, 2, 3]), vec![0..4]);
        assert_eq!(byte_ranges("a·b", &[1, 2]), vec![1..4]);
        assert_eq!(byte_ranges("pods", &[0, 2]), vec![0..1, 2..3]);
    }
}
