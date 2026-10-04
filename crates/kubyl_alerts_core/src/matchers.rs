//! Label matchers: the filter input of the Alerts view and silences share them.
//!
//! `alertname=~"Kube.*", namespace="payments"` (braces optional, values quoted or not). Regex
//! matchers are anchored like Alertmanager's (`^(?:value)$`, RE2 syntax through `regex`), and a
//! missing label matches like an empty one.

use std::collections::BTreeMap;
use std::fmt;

use regex::Regex;
use serde_json::{Value, json};

/// `=`, `!=`, `=~`, `!~`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchOp {
    Equal,
    NotEqual,
    Regex,
    NotRegex,
}

impl MatchOp {
    pub const ALL: [MatchOp; 4] = [
        MatchOp::Equal,
        MatchOp::NotEqual,
        MatchOp::Regex,
        MatchOp::NotRegex,
    ];

    pub fn symbol(self) -> &'static str {
        match self {
            MatchOp::Equal => "=",
            MatchOp::NotEqual => "!=",
            MatchOp::Regex => "=~",
            MatchOp::NotRegex => "!~",
        }
    }

    fn is_regex(self) -> bool {
        matches!(self, MatchOp::Regex | MatchOp::NotRegex)
    }

    fn is_equal(self) -> bool {
        matches!(self, MatchOp::Equal | MatchOp::Regex)
    }
}

/// One matcher.
#[derive(Clone, Debug)]
pub struct Matcher {
    pub name: String,
    pub op: MatchOp,
    pub value: String,
    regex: Option<Regex>,
}

impl PartialEq for Matcher {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.op == other.op && self.value == other.value
    }
}

impl Eq for Matcher {}

impl fmt::Display for Matcher {
    /// `name="value"` (the value quoted and escaped).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}\"{}\"",
            self.name,
            self.op.symbol(),
            self.value.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

/// Anchored like Alertmanager: the whole value must match.
fn anchored(value: &str) -> Result<Regex, String> {
    Regex::new(&format!("^(?:{value})$")).map_err(|e| e.to_string())
}

impl Matcher {
    /// A matcher; an invalid regex never matches (see [`Matcher::check`]).
    pub fn new(name: &str, op: MatchOp, value: &str) -> Self {
        let regex = op.is_regex().then(|| anchored(value).ok()).flatten();
        Self {
            name: name.to_string(),
            op,
            value: value.to_string(),
            regex,
        }
    }

    /// An Alertmanager API matcher (`isRegex`, `isEqual`, which defaults to true).
    pub fn from_json(value: &Value) -> Option<Self> {
        let name = value["name"].as_str()?;
        let regex = value["isRegex"].as_bool().unwrap_or(false);
        let equal = value["isEqual"].as_bool().unwrap_or(true);
        let op = match (regex, equal) {
            (false, true) => MatchOp::Equal,
            (false, false) => MatchOp::NotEqual,
            (true, true) => MatchOp::Regex,
            (true, false) => MatchOp::NotRegex,
        };
        Some(Self::new(
            name,
            op,
            value["value"].as_str().unwrap_or_default(),
        ))
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "value": self.value,
            "isRegex": self.op.is_regex(),
            "isEqual": self.op.is_equal(),
        })
    }

    /// The regex is valid (always true for `=`/`!=`).
    pub fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a matcher needs a label name".into());
        }
        if self.op.is_regex() {
            anchored(&self.value).map(|_| ())
        } else {
            Ok(())
        }
    }

    pub fn matches_value(&self, value: &str) -> bool {
        match self.op {
            MatchOp::Equal => value == self.value,
            MatchOp::NotEqual => value != self.value,
            MatchOp::Regex => self.regex.as_ref().is_some_and(|r| r.is_match(value)),
            MatchOp::NotRegex => self.regex.as_ref().is_some_and(|r| !r.is_match(value)),
        }
    }

    /// Matches a label set (a missing label counts as empty).
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.matches_value(labels.get(&self.name).map(String::as_str).unwrap_or(""))
    }

    /// Matches every label set, even one without this label: Alertmanager refuses silences
    /// made only of such matchers.
    pub fn matches_empty(&self) -> bool {
        self.matches_value("")
    }
}

/// Parses `a="x", b=~"y.*"`, with or without braces and quotes.
pub fn parse(text: &str) -> Result<Vec<Matcher>, String> {
    let text = text.trim();
    let text = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(text);
    let mut matchers = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let name_end = rest
            .find(['=', '!'])
            .ok_or_else(|| format!("expected an operator after {rest:?}"))?;
        let name = rest[..name_end].trim();
        if name.is_empty() {
            return Err("a matcher needs a label name".into());
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-')
        {
            return Err(format!("invalid label name {name:?}"));
        }
        rest = &rest[name_end..];
        let op = [
            ("=~", MatchOp::Regex),
            ("!~", MatchOp::NotRegex),
            ("!=", MatchOp::NotEqual),
            ("=", MatchOp::Equal),
        ]
        .into_iter()
        .find(|(symbol, _)| rest.starts_with(symbol))
        .ok_or_else(|| format!("expected =, !=, =~ or !~ after {name}"))?;
        rest = rest[op.0.len()..].trim_start();
        let (value, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = None;
            while let Some((ix, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, next)) = chars.next() {
                            value.push(next);
                        }
                    }
                    '"' => {
                        end = Some(ix);
                        break;
                    }
                    c => value.push(c),
                }
            }
            let end = end.ok_or("unterminated quote")?;
            (value, &quoted[end + 1..])
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            (rest[..end].trim().to_string(), &rest[end..])
        };
        let matcher = Matcher::new(name, op.1, &value);
        matcher.check()?;
        matchers.push(matcher);
        rest = after.trim_start();
        if let Some(next) = rest.strip_prefix(',') {
            rest = next.trim_start();
        } else if !rest.is_empty() {
            return Err(format!("expected , before {rest:?}"));
        }
    }
    Ok(matchers)
}

/// Why Alertmanager would refuse a silence with these matchers, if it would.
pub fn validate_silence(matchers: &[Matcher]) -> Result<(), String> {
    if matchers.is_empty() {
        return Err("A silence needs at least one matcher.".into());
    }
    for matcher in matchers {
        matcher
            .check()
            .map_err(|e| format!("{}: {e}", matcher.name))?;
    }
    if matchers.iter().all(Matcher::matches_empty) {
        return Err(
            "At least one matcher must not match an empty value (it would silence everything)."
                .into(),
        );
    }
    Ok(())
}

/// `a="x", b=~"y"`.
pub fn format(matchers: &[Matcher]) -> String {
    matchers
        .iter()
        .map(Matcher::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `amtool alert query 'a="x"' 'b="y"'` style arguments, shell-quoted.
pub fn amtool_args(matchers: &[Matcher]) -> String {
    matchers
        .iter()
        .map(|m| format!("'{}'", m.to_string().replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parses_matchers() {
        let ms =
            parse(r#"{alertname=~"Kube.*", namespace="payments", severity!=info, pod!~"x-\"q\""}"#)
                .unwrap();
        assert_eq!(ms.len(), 4);
        assert_eq!(ms[0].op, MatchOp::Regex);
        assert_eq!(ms[2].value, "info");
        assert_eq!(ms[3].value, "x-\"q\"");
        assert_eq!(
            format(&ms[..2]),
            r#"alertname=~"Kube.*", namespace="payments""#
        );
        assert!(parse("a").is_err());
        assert!(parse(r#"a="x" b="y""#).is_err());
        assert!(parse(r#"a=~"(""#).is_err(), "invalid regex");
        assert!(parse(r#"a="unterminated"#).is_err());
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn regexes_are_anchored_like_alertmanager() {
        let m = Matcher::new("alertname", MatchOp::Regex, "Kube.*");
        assert!(m.matches(&labels(&[("alertname", "KubePodCrashLooping")])));
        assert!(
            !m.matches(&labels(&[("alertname", "NotKube")])),
            "anchored at the start"
        );
        let m = Matcher::new("namespace", MatchOp::Regex, "pay");
        assert!(
            !m.matches(&labels(&[("namespace", "payments")])),
            "anchored at the end"
        );
        let m = Matcher::new("team", MatchOp::NotEqual, "a");
        assert!(m.matches(&labels(&[])), "missing counts as empty");
        let m = Matcher::new("x", MatchOp::NotRegex, "a|b");
        assert!(m.matches(&labels(&[("x", "c")])));
        assert!(!m.matches(&labels(&[("x", "a")])));
    }

    #[test]
    fn silences_need_a_matcher_that_does_not_match_empty() {
        assert!(validate_silence(&[]).is_err());
        let everything = [Matcher::new("alertname", MatchOp::Regex, ".*")];
        assert!(validate_silence(&everything).is_err());
        let not = [Matcher::new("alertname", MatchOp::NotEqual, "X")];
        assert!(validate_silence(&not).is_err(), "!= matches empty");
        let ok = [
            Matcher::new("alertname", MatchOp::Equal, "KubePodCrashLooping"),
            Matcher::new("pod", MatchOp::Regex, ".*"),
        ];
        assert!(validate_silence(&ok).is_ok());
        let broken = [Matcher::new("a", MatchOp::Regex, "(")];
        assert!(validate_silence(&broken).is_err());
    }

    #[test]
    fn api_round_trip_and_amtool() {
        let m = Matcher::new("pod", MatchOp::NotRegex, "gw-.*");
        let json = m.to_json();
        assert_eq!(json["isRegex"], true);
        assert_eq!(json["isEqual"], false);
        assert_eq!(Matcher::from_json(&json).unwrap(), m);
        let old = serde_json::json!({"name": "a", "value": "b", "isRegex": false});
        assert_eq!(Matcher::from_json(&old).unwrap().op, MatchOp::Equal);
        assert_eq!(
            amtool_args(&[Matcher::new("alertname", MatchOp::Equal, "It's")]),
            r#"'alertname="It'\''s"'"#
        );
    }
}
