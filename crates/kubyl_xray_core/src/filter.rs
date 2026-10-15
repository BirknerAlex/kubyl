//! The XRay filter, as in k9s: a case-insensitive regex tested against the path of every leaf
//! (`!` in front keeps what doesn't match), `-f text` for a fuzzy match, `-l selector` for a
//! label selector on the objects the tree starts from. A matching leaf keeps its ancestors; a
//! branch without a match disappears.

use regex::{Regex, RegexBuilder};
use serde_json::Value;

use crate::Node;

#[derive(Clone, Debug)]
enum Matcher {
    Regex { regex: Regex, inverse: bool },
    Fuzzy(String),
}

/// A parsed filter. The default filter keeps everything.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    matcher: Option<Matcher>,
    labels: Option<String>,
}

impl Filter {
    /// Parses what was typed. `Err` says why a regex is invalid.
    pub fn parse(text: &str) -> Result<Filter, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Filter::default());
        }
        if let Some(selector) = text.strip_prefix("-l") {
            let selector = selector.trim();
            return Ok(Filter {
                matcher: None,
                labels: (!selector.is_empty()).then(|| selector.to_string()),
            });
        }
        if let Some(fuzzy) = text.strip_prefix("-f") {
            let fuzzy = fuzzy.trim().to_lowercase();
            return Ok(Filter {
                matcher: (!fuzzy.is_empty()).then_some(Matcher::Fuzzy(fuzzy)),
                labels: None,
            });
        }
        let (inverse, pattern) = match text.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, text),
        };
        if pattern.is_empty() {
            return Ok(Filter::default());
        }
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map_err(|err| format!("invalid regex: {err}"))?;
        Ok(Filter {
            matcher: Some(Matcher::Regex { regex, inverse }),
            labels: None,
        })
    }

    /// Whether anything is filtered by text (the label selector applies when building).
    pub fn is_active(&self) -> bool {
        self.matcher.is_some()
    }

    /// The label selector for the objects the tree starts from.
    pub fn labels(&self) -> Option<&str> {
        self.labels.as_deref()
    }

    /// `tokens`: the names on the way from the root to the leaf; the last one carries the
    /// leaf's status text.
    fn matches(&self, tokens: &[String]) -> bool {
        match &self.matcher {
            None => true,
            Some(Matcher::Regex { regex, inverse }) => {
                tokens.iter().any(|t| regex.is_match(t)) != *inverse
            }
            Some(Matcher::Fuzzy(needle)) => {
                let hay = tokens.join("/").to_lowercase();
                let mut chars = hay.chars();
                needle.chars().all(|n| chars.any(|h| h == n))
            }
        }
    }

    /// The tree with only the leaves that match, and their ancestors.
    pub fn prune(&self, roots: &[Node]) -> Vec<Node> {
        if !self.is_active() {
            return roots.to_vec();
        }
        roots
            .iter()
            .filter_map(|n| self.prune_node(n, &mut Vec::new()))
            .collect()
    }

    fn prune_node(&self, node: &Node, path: &mut Vec<String>) -> Option<Node> {
        if node.children.is_empty() {
            let mut tokens = path.clone();
            tokens.push(format!("{}{}", node.name, node.info));
            return self.matches(&tokens).then(|| node.clone());
        }
        path.push(node.name.clone());
        let children: Vec<Node> = node
            .children
            .iter()
            .filter_map(|c| self.prune_node(c, path))
            .collect();
        path.pop();
        (!children.is_empty()).then(|| Node {
            children,
            ..node.clone()
        })
    }
}

/// Whether `labels` satisfy a comma-separated selector: `a=b`, `a==b`, `a!=b`, `a`, `!a`,
/// `a in (x,y)`, `a notin (x,y)`.
pub fn selector_matches(selector: &str, labels: Option<&Value>) -> bool {
    let label = |k: &str| labels.and_then(|l| l.get(k)).and_then(Value::as_str);
    split_terms(selector).iter().all(|term| {
        let term = term.trim();
        if term.is_empty() {
            return true;
        }
        if let Some((key, rest)) = term.split_once(" notin ") {
            let values = set(rest);
            return label(key.trim()).is_none_or(|v| !values.contains(&v));
        }
        if let Some((key, rest)) = term.split_once(" in ") {
            let values = set(rest);
            return label(key.trim()).is_some_and(|v| values.contains(&v));
        }
        if let Some((key, value)) = term.split_once("!=") {
            return label(key.trim()) != Some(value.trim());
        }
        if let Some((key, value)) = term.split_once("==").or_else(|| term.split_once('=')) {
            return label(key.trim()) == Some(value.trim());
        }
        match term.strip_prefix('!') {
            Some(key) => label(key.trim()).is_none(),
            None => label(term).is_some(),
        }
    })
}

/// Splits on commas outside parentheses.
fn split_terms(selector: &str) -> Vec<&str> {
    let (mut terms, mut depth, mut start) = (Vec::new(), 0, 0);
    for (i, c) in selector.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                terms.push(&selector[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    terms.push(&selector[start..]);
    terms
}

fn set(text: &str) -> Vec<&str> {
    text.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(str::trim)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::Health;

    fn node(name: &str, children: Vec<Node>) -> Node {
        Node {
            kind: "X",
            group: "",
            resource: "",
            namespace: None,
            name: name.into(),
            health: Health::Ok,
            info: String::new(),
            children,
        }
    }

    fn tree() -> Vec<Node> {
        vec![node(
            "ns",
            vec![
                node("web", vec![node("web-1", vec![node("app", vec![])])]),
                node("db", vec![node("db-1", vec![node("pg", vec![])])]),
            ],
        )]
    }

    #[test]
    fn regex_keeps_matching_leaves_and_ancestors() {
        let pruned = Filter::parse("PG").unwrap().prune(&tree());
        assert_eq!(pruned[0].children.len(), 1);
        assert_eq!(pruned[0].children[0].name, "db");
        // A match on an ancestor's name keeps the leaves below it.
        let pruned = Filter::parse("^web$").unwrap().prune(&tree());
        assert_eq!(pruned[0].children[0].name, "web");
        assert!(Filter::parse("(").is_err());
    }

    #[test]
    fn inverse_and_fuzzy() {
        let pruned = Filter::parse("!db").unwrap().prune(&tree());
        assert_eq!(pruned[0].children.len(), 1);
        assert_eq!(pruned[0].children[0].name, "web");
        let pruned = Filter::parse("-f wb1").unwrap().prune(&tree());
        assert_eq!(pruned[0].children[0].name, "web");
        assert!(Filter::parse("zzz").unwrap().prune(&tree()).is_empty());
    }

    #[test]
    fn label_selectors() {
        let l = json!({"app": "web", "tier": "fe"});
        assert!(selector_matches("app=web,tier!=db", Some(&l)));
        assert!(selector_matches("app in (web, api)", Some(&l)));
        assert!(!selector_matches("tier notin (fe)", Some(&l)));
        assert!(selector_matches("!canary", Some(&l)));
        assert!(!selector_matches("app", None));
        assert_eq!(
            Filter::parse("-l app=web").unwrap().labels(),
            Some("app=web")
        );
    }
}
