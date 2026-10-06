//! `@` mentions in the agent composer: what's being typed, which objects match, and the text
//! after picking one.

/// An object that can be mentioned.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
}

impl Candidate {
    /// `Pod shop/web-0`.
    pub fn label(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{} {ns}/{}", self.kind, self.name),
            None => format!("{} {}", self.kind, self.name),
        }
    }
}

/// The query of a mention being typed at the end of `text`: the word after a trailing `@`
/// (which starts the text or follows whitespace). `Some("")` right after typing `@`.
pub fn query(text: &str) -> Option<&str> {
    let start = text.rfind(|c: char| c.is_whitespace()).map_or(0, |i| {
        i + text[i..].chars().next().map_or(1, char::len_utf8)
    });
    text[start..].strip_prefix('@')
}

/// The best matches for `query`, at most `limit`, as indices into `candidates`: names that start
/// with it first, then names that contain it, then kind or namespace matches; shorter names
/// first within a rank.
pub fn rank(candidates: &[Candidate], query: &str, limit: usize) -> Vec<usize> {
    let query = query.to_lowercase();
    let mut scored: Vec<(u8, usize, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let name = c.name.to_lowercase();
            let rank = if name.starts_with(&query) {
                0
            } else if name.contains(&query) {
                1
            } else if c.kind.to_lowercase().starts_with(&query)
                || c.namespace
                    .as_deref()
                    .is_some_and(|ns| ns.to_lowercase().starts_with(&query))
            {
                2
            } else {
                return None;
            };
            Some((rank, c.name.len(), i))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(limit).map(|(_, _, i)| i).collect()
}

/// `text` with the trailing `@query` replaced by `@name ` (or unchanged without a mention).
pub fn complete(text: &str, name: &str) -> String {
    match query(text) {
        Some(q) => format!("{}@{name} ", &text[..text.len() - q.len() - 1]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(kind: &str, ns: Option<&str>, name: &str) -> Candidate {
        Candidate {
            kind: kind.into(),
            namespace: ns.map(str::to_string),
            name: name.into(),
        }
    }

    #[test]
    fn queries_start_at_a_trailing_at_sign() {
        assert_eq!(query("why does @web"), Some("web"));
        assert_eq!(query("@"), Some(""));
        assert_eq!(query("look at\n@ap"), Some("ap"));
        assert_eq!(query("mail me@example"), None);
        assert_eq!(query("@web is down"), None);
        assert_eq!(query(""), None);
    }

    #[test]
    fn matches_rank_prefixes_first() {
        let candidates = [
            c("Pod", Some("shop"), "checkout-web-0"),
            c("Deployment", Some("shop"), "web"),
            c("Pod", Some("shop"), "web-7d9f-x2kqp"),
            c("Node", None, "worker-1"),
        ];
        assert_eq!(rank(&candidates, "web", 10), [1, 2, 0]);
        assert_eq!(rank(&candidates, "NODE", 10), [3]);
        assert_eq!(rank(&candidates, "", 2).len(), 2);
        assert_eq!(candidates[0].label(), "Pod shop/checkout-web-0");
        assert_eq!(candidates[3].label(), "Node worker-1");
    }

    #[test]
    fn completing_replaces_the_query() {
        assert_eq!(complete("why does @we", "web-0"), "why does @web-0 ");
        assert_eq!(complete("@", "web-0"), "@web-0 ");
        assert_eq!(complete("no mention", "web-0"), "no mention");
    }
}
