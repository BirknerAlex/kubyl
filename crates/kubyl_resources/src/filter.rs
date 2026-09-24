//! The list filter: `checkout`, `!checkout`, `/^api-\d+$/`, `app=web`, `tier!=db`,
//! `app in (a, b)`, `env notin (dev)`, `status.phase=Running`.
//!
//! Terms are separated by spaces or commas and must all match:
//! - a word matches the name or namespace (case-insensitive substring); `!word` negates it;
//! - `/regex/` matches the name; `!/regex/` negates it;
//! - `key=value`, `key==value`, `key!=value`, `key in (…)`, `key notin (…)` match labels;
//! - keys starting with `metadata.`, `spec.` or `status.` are field selectors on the object
//!   (`status.phase=Running`, `spec.nodeName!=node-1`).

use regex::Regex;
use serde_json::Value;

#[derive(Clone, Debug)]
enum Term {
    Text {
        needle: String,
        negate: bool,
    },
    Regex {
        regex: Regex,
        negate: bool,
    },
    Label {
        key: String,
        values: Vec<String>,
        negate: bool,
    },
    Field {
        pointer: String,
        value: String,
        negate: bool,
    },
}

/// A parsed filter. The empty filter matches everything.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    terms: Vec<Term>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Parses the filter input. Errors describe the first invalid term.
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut terms = Vec::new();
        let set = Regex::new(r"([A-Za-z0-9./_-]+)\s+(in|notin)\s*\(([^)]*)\)").expect("valid");
        let mut rest = String::new();
        let mut last = 0;
        for capture in set.captures_iter(input) {
            let whole = capture.get(0).expect("match");
            rest.push_str(&input[last..whole.start()]);
            rest.push(' ');
            last = whole.end();
            let values = capture[3]
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            terms.push(Term::Label {
                key: capture[1].to_string(),
                values,
                negate: &capture[2] == "notin",
            });
        }
        rest.push_str(&input[last..]);

        for token in rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
        {
            terms.push(parse_term(token)?);
        }
        Ok(Self { terms })
    }

    /// Whether an object matches every term.
    pub fn matches(&self, object: &Value) -> bool {
        let meta = &object["metadata"];
        let name = meta["name"].as_str().unwrap_or_default();
        let namespace = meta["namespace"].as_str().unwrap_or_default();
        self.terms.iter().all(|term| match term {
            Term::Text { needle, negate } => {
                let found =
                    contains_ignore_case(name, needle) || contains_ignore_case(namespace, needle);
                found != *negate
            }
            Term::Regex { regex, negate } => regex.is_match(name) != *negate,
            Term::Label {
                key,
                values,
                negate,
            } => {
                let label = meta["labels"][key.as_str()].as_str();
                let found = label.is_some_and(|l| values.iter().any(|v| v == l));
                found != *negate
            }
            Term::Field {
                pointer,
                value,
                negate,
            } => {
                let actual = match object.pointer(pointer) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                (actual == *value) != *negate
            }
        })
    }
}

fn contains_ignore_case(haystack: &str, lowercase_needle: &str) -> bool {
    if haystack.is_ascii() {
        haystack
            .as_bytes()
            .windows(lowercase_needle.len().max(1))
            .any(|w| w.eq_ignore_ascii_case(lowercase_needle.as_bytes()))
            || lowercase_needle.is_empty()
    } else {
        haystack.to_lowercase().contains(lowercase_needle)
    }
}

fn is_field(key: &str) -> bool {
    ["metadata.", "spec.", "status."]
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

fn parse_term(token: &str) -> Result<Term, String> {
    let (negate, body) = match token.strip_prefix('!') {
        Some(rest) if !rest.is_empty() && !rest.starts_with('=') => (true, rest),
        _ => (false, token),
    };
    if body.len() > 2 && body.starts_with('/') && body.ends_with('/') {
        let regex = Regex::new(&body[1..body.len() - 1])
            .map_err(|err| format!("invalid regex {body}: {err}"))?;
        return Ok(Term::Regex { regex, negate });
    }
    let comparison = token
        .find("!=")
        .map(|ix| (ix, 2, true))
        .or_else(|| token.find("==").map(|ix| (ix, 2, false)))
        .or_else(|| token.find('=').map(|ix| (ix, 1, false)));
    if let Some((ix, len, not_equal)) = comparison {
        let key = &token[..ix];
        let value = &token[ix + len..];
        if key.is_empty() {
            return Err(format!("missing key in {token}"));
        }
        return Ok(if is_field(key) {
            Term::Field {
                pointer: format!("/{}", key.replace('.', "/")),
                value: value.to_string(),
                negate: not_equal,
            }
        } else {
            Term::Label {
                key: key.to_string(),
                values: vec![value.to_string()],
                negate: not_equal,
            }
        });
    }
    Ok(Term::Text {
        needle: body.to_lowercase(),
        negate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(name: &str, app: &str, phase: &str) -> Value {
        json!({"metadata": {"name": name, "namespace": "payments", "labels": {"app": app}},
               "status": {"phase": phase}})
    }

    fn names(filter: &str) -> Vec<&'static str> {
        let pods = [
            ("checkout-api-1", "checkout-api", "Running"),
            ("payment-gateway-1", "payment-gateway", "Pending"),
            ("ledger-writer-0", "ledger-writer", "Running"),
        ];
        let filter = Filter::parse(filter).unwrap();
        pods.iter()
            .filter(|(n, a, p)| filter.matches(&pod(n, a, p)))
            .map(|(n, _, _)| *n)
            .collect()
    }

    #[test]
    fn text_regex_and_negation() {
        assert_eq!(
            names(""),
            ["checkout-api-1", "payment-gateway-1", "ledger-writer-0"]
        );
        assert_eq!(names("CHECKOUT"), ["checkout-api-1"]);
        assert_eq!(names("!checkout"), ["payment-gateway-1", "ledger-writer-0"]);
        assert_eq!(
            names(r"/-\d$/ !ledger"),
            ["checkout-api-1", "payment-gateway-1"]
        );
        assert_eq!(names("payments").len(), 3);
    }

    #[test]
    fn labels_and_sets() {
        assert_eq!(names("app=ledger-writer"), ["ledger-writer-0"]);
        assert_eq!(names("app!=ledger-writer").len(), 2);
        assert_eq!(
            names("app in (checkout-api, payment-gateway)"),
            ["checkout-api-1", "payment-gateway-1"]
        );
        assert_eq!(
            names("app notin (checkout-api), !ledger"),
            ["payment-gateway-1"]
        );
    }

    #[test]
    fn field_selectors() {
        assert_eq!(names("status.phase=Pending"), ["payment-gateway-1"]);
        assert_eq!(names("status.phase!=Pending checkout"), ["checkout-api-1"]);
        assert_eq!(names("metadata.namespace=payments").len(), 3);
    }

    #[test]
    fn invalid_regex_is_an_error() {
        assert!(Filter::parse("/(/").is_err());
        assert!(Filter::parse("=x").is_err());
    }
}
