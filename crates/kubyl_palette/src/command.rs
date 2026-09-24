//! Palette modes (chosen by the first character typed) and k9s-style inline commands
//! (`:pods payments`, `:deploy -A`, `:ctx staging`, `:ns kube-system`, `:q`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the palette searches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// No prefix: everything, grouped.
    #[default]
    All,
    /// `:` resource kinds and inline commands.
    Resources,
    /// `@` kubeconfig contexts.
    Contexts,
    /// `#` namespaces of the active cluster.
    Namespaces,
    /// `>` actions, scoped to what was focused.
    Actions,
    /// `*` favorites.
    Favorites,
    /// `/` filter the focused list.
    Filter,
    /// Go to object: names in the loaded caches of every connected cluster (`⌘P`).
    Objects,
    /// Objects the selection refers to (owner, node, secrets, selected pods…).
    References,
}

impl Mode {
    /// Modes with a prefix character, in chip order.
    pub const PREFIXED: [Mode; 6] = [
        Mode::Resources,
        Mode::Contexts,
        Mode::Namespaces,
        Mode::Actions,
        Mode::Favorites,
        Mode::Filter,
    ];

    pub fn prefix(self) -> Option<char> {
        Some(match self {
            Mode::Resources => ':',
            Mode::Contexts => '@',
            Mode::Namespaces => '#',
            Mode::Actions => '>',
            Mode::Favorites => '*',
            Mode::Filter => '/',
            Mode::All | Mode::Objects | Mode::References => return None,
        })
    }

    pub fn from_prefix(c: char) -> Option<Mode> {
        Self::PREFIXED.into_iter().find(|m| m.prefix() == Some(c))
    }

    /// Chip label.
    pub fn label(self) -> &'static str {
        match self {
            Mode::All => "everything",
            Mode::Resources => "resources",
            Mode::Contexts => "contexts",
            Mode::Namespaces => "namespaces",
            Mode::Actions => "actions",
            Mode::Favorites => "favorites",
            Mode::Filter => "filter",
            Mode::Objects => "objects",
            Mode::References => "references",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Mode::All => "Search everything, or type : @ # > * /",
            Mode::Resources => "Kind, or: po <ns> · deploy -A · ctx <name>",
            Mode::Contexts => "Switch to context…",
            Mode::Namespaces => "Switch to namespace…",
            Mode::Actions => "Run an action…",
            Mode::Favorites => "Open a favorite…",
            Mode::Filter => "Filter the list: text, label=value, status…",
            Mode::Objects => "Go to object by name…",
            Mode::References => "Go to a referenced object…",
        }
    }

    /// Splits a leading prefix character off `text` (`:cert` → `Resources`, `cert`).
    pub fn split(text: &str) -> Option<(Mode, &str)> {
        let first = text.chars().next()?;
        Some((Mode::from_prefix(first)?, &text[first.len_utf8()..]))
    }
}

/// Which namespace an opened list shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// The active namespace.
    #[default]
    Active,
    /// All namespaces (`-A`, `⇥`).
    All,
    Named(String),
}

/// A `:` query that is more than a kind name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    /// `pods payments`, `deploy -A`, `po -n kube-system`.
    Kind { kind: String, scope: Scope },
    /// `ctx` alone lists contexts; `ctx <name>` switches.
    Context(Option<String>),
    /// `ns` alone lists namespaces; `ns <name>` switches.
    Namespace(Option<String>),
    /// `xray <kind>` (a later phase).
    Xray(String),
    /// `q`, `q!`, `quit`.
    Quit,
}

/// Parses a `:` query. `None`: a plain kind search (`cert`, `pods`).
pub fn parse_inline(query: &str) -> Option<Inline> {
    let mut words = query.split_whitespace();
    let head = words.next()?.to_lowercase();
    let rest: Vec<&str> = words.collect();
    let arg = rest.first().map(|s| s.to_string());
    match head.as_str() {
        "q" | "q!" | "quit" | "qa" if rest.is_empty() => return Some(Inline::Quit),
        "ctx" | "context" | "contexts" => return Some(Inline::Context(arg)),
        "ns" | "namespace" | "namespaces" if rest.len() <= 1 => {
            return Some(Inline::Namespace(arg));
        }
        "xray" | "x" => return Some(Inline::Xray(arg.unwrap_or_default())),
        _ => {}
    }
    if rest.is_empty() {
        return None;
    }
    let mut scope = Scope::Active;
    let mut args = rest.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "-A" | "--all-namespaces" | "all" => scope = Scope::All,
            "-n" | "--namespace" => {
                if let Some(ns) = args.next() {
                    scope = Scope::Named(ns.to_string());
                }
            }
            other => {
                if let Some(ns) = other.strip_prefix("--namespace=") {
                    scope = Scope::Named(ns.to_string());
                } else if !other.starts_with('-') {
                    scope = Scope::Named(other.to_string());
                }
            }
        }
    }
    Some(Inline::Kind { kind: head, scope })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_pick_modes() {
        assert_eq!(Mode::split(":cert"), Some((Mode::Resources, "cert")));
        assert_eq!(Mode::split("@staging"), Some((Mode::Contexts, "staging")));
        assert_eq!(Mode::split("#"), Some((Mode::Namespaces, "")));
        assert_eq!(Mode::split(">delete"), Some((Mode::Actions, "delete")));
        assert_eq!(Mode::split("*pay"), Some((Mode::Favorites, "pay")));
        assert_eq!(Mode::split("/app=web"), Some((Mode::Filter, "app=web")));
        assert_eq!(Mode::split("pods"), None);
        assert_eq!(Mode::split(""), None);
    }

    #[test]
    fn parses_k9s_commands() {
        assert_eq!(parse_inline("cert"), None);
        assert_eq!(
            parse_inline("pods payments"),
            Some(Inline::Kind {
                kind: "pods".into(),
                scope: Scope::Named("payments".into())
            })
        );
        assert_eq!(
            parse_inline("deploy -A"),
            Some(Inline::Kind {
                kind: "deploy".into(),
                scope: Scope::All
            })
        );
        assert_eq!(
            parse_inline("po -n kube-system"),
            Some(Inline::Kind {
                kind: "po".into(),
                scope: Scope::Named("kube-system".into())
            })
        );
        assert_eq!(
            parse_inline("ctx staging-eu-west-1"),
            Some(Inline::Context(Some("staging-eu-west-1".into())))
        );
        assert_eq!(parse_inline("ctx"), Some(Inline::Context(None)));
        assert_eq!(
            parse_inline("ns kube-system"),
            Some(Inline::Namespace(Some("kube-system".into())))
        );
        assert_eq!(
            parse_inline("xray deploy"),
            Some(Inline::Xray("deploy".into()))
        );
        assert_eq!(parse_inline("q"), Some(Inline::Quit));
        assert_eq!(parse_inline("q!"), Some(Inline::Quit));
    }
}
