//! The tree as the flat rows a table shows.

use std::collections::HashSet;

use crate::{Filter, Health, Node};

/// One visible line.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Path from the root (`Deployment/web>ReplicaSet/web-1`): unique, stable across refreshes.
    pub path: String,
    pub depth: usize,
    pub expandable: bool,
    pub expanded: bool,
    pub node: Node,
    /// Worst health of the subtree: a collapsed branch still shows red.
    pub health: Health,
}

/// Flattens `roots`, descending into nodes whose path is in `expanded`. With a text filter only
/// matching leaves and their ancestors stay, and everything is open.
pub fn flatten(roots: &[Node], expanded: &HashSet<String>, filter: &Filter) -> Vec<Row> {
    let pruned = filter.prune(roots);
    let mut out = Vec::new();
    for node in &pruned {
        walk(node, "", 0, expanded, filter.is_active(), &mut out);
    }
    out
}

fn walk(
    node: &Node,
    parent: &str,
    depth: usize,
    expanded: &HashSet<String>,
    all_open: bool,
    out: &mut Vec<Row>,
) {
    let path = if parent.is_empty() {
        node.id()
    } else {
        format!("{parent}>{}", node.id())
    };
    let open = all_open || expanded.contains(&path);
    out.push(Row {
        path: path.clone(),
        depth,
        expandable: !node.children.is_empty(),
        expanded: open && !node.children.is_empty(),
        node: Node {
            children: Vec::new(),
            ..node.clone()
        },
        health: node.worst(),
    });
    if open {
        for child in &node.children {
            walk(child, &path, depth + 1, expanded, all_open, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: &'static str, name: &str, children: Vec<Node>) -> Node {
        Node {
            kind,
            group: "",
            resource: "",
            namespace: None,
            name: name.into(),
            health: Health::Ok,
            info: String::new(),
            children,
        }
    }

    #[test]
    fn collapsed_shows_roots_and_filter_opens_matches() {
        let tree = vec![node(
            "Deployment",
            "web",
            vec![node("Pod", "web-1", vec![node("Container", "app", vec![])])],
        )];
        let none = HashSet::new();
        let all = Filter::default();
        assert_eq!(flatten(&tree, &none, &all).len(), 1);
        let open: HashSet<String> = ["Deployment/web".to_string()].into();
        assert_eq!(flatten(&tree, &open, &all).len(), 2);
        let rows = flatten(&tree, &none, &Filter::parse("app").unwrap());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].depth, 2);
        assert!(flatten(&tree, &none, &Filter::parse("zzz").unwrap()).is_empty());
    }
}
