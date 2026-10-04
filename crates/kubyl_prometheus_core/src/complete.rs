//! Completion for the query box, like the Prometheus UI's: metric names, PromQL functions,
//! aggregations and keywords in an expression, label names inside `{…}`. Nothing inside strings
//! or range selectors (`[5m]`).

/// What a suggestion is, shown next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Metric,
    Function,
    Keyword,
    Label,
    /// A label value.
    Value,
    /// `==`, `=~`, `+`…
    Operator,
    /// A duration unit.
    Unit,
    /// A call with its usual arguments.
    Snippet,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Metric => "metric",
            Kind::Function => "function",
            Kind::Keyword => "keyword",
            Kind::Label => "label",
            Kind::Value => "value",
            Kind::Operator => "operator",
            Kind::Unit => "unit",
            Kind::Snippet => "snippet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// What replaces the word being typed (`rate(` for a function).
    pub insert: String,
    pub kind: Kind,
    /// Type and help of a metric.
    pub detail: Option<String>,
    /// Where the cursor lands in `insert` (the end when `None`): inside a snippet's brackets.
    pub cursor: Option<usize>,
}

/// Marks where the cursor goes in a snippet template.
const CURSOR: char = '\u{1}';

impl Item {
    fn new(label: impl Into<String>, insert: impl Into<String>, kind: Kind) -> Self {
        Self {
            label: label.into(),
            insert: insert.into(),
            kind,
            detail: None,
            cursor: None,
        }
    }

    /// A snippet: `template` holds a [`CURSOR`] marker.
    fn snippet(label: &str, template: &str) -> Self {
        let cursor = template.find(CURSOR);
        Self {
            label: label.to_string(),
            insert: template.replace(CURSOR, ""),
            kind: Kind::Snippet,
            detail: None,
            cursor,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// Byte offset in the text where the word being completed starts.
    pub start: usize,
    pub items: Vec<Item>,
}

/// Metric and label names of a server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Names {
    pub metrics: Vec<String>,
    pub labels: Vec<String>,
    /// Type and help per metric, when the server has few enough metrics to ask for them.
    pub metadata: std::collections::HashMap<String, (String, String)>,
}

const FUNCTIONS: &[&str] = &[
    "abs",
    "absent",
    "absent_over_time",
    "acos",
    "asin",
    "atan",
    "avg_over_time",
    "ceil",
    "changes",
    "clamp",
    "clamp_max",
    "clamp_min",
    "cos",
    "count_over_time",
    "day_of_month",
    "day_of_week",
    "day_of_year",
    "days_in_month",
    "deg",
    "delta",
    "deriv",
    "exp",
    "floor",
    "histogram_avg",
    "histogram_count",
    "histogram_fraction",
    "histogram_quantile",
    "histogram_stddev",
    "histogram_sum",
    "holt_winters",
    "hour",
    "idelta",
    "increase",
    "irate",
    "label_join",
    "label_replace",
    "last_over_time",
    "ln",
    "log10",
    "log2",
    "mad_over_time",
    "max_over_time",
    "min_over_time",
    "minute",
    "month",
    "pi",
    "predict_linear",
    "present_over_time",
    "quantile_over_time",
    "rad",
    "rate",
    "resets",
    "round",
    "scalar",
    "sgn",
    "sin",
    "sort",
    "sort_by_label",
    "sort_desc",
    "sqrt",
    "stddev_over_time",
    "stdvar_over_time",
    "sum_over_time",
    "tan",
    "time",
    "timestamp",
    "vector",
    "year",
    // Aggregations.
    "avg",
    "bottomk",
    "count",
    "count_values",
    "group",
    "limit_ratio",
    "limitk",
    "max",
    "min",
    "quantile",
    "stddev",
    "stdvar",
    "sum",
    "topk",
];

/// Keywords that take a label list next: `by (`.
const GROUPING: &[&str] = &[
    "by",
    "without",
    "on",
    "ignoring",
    "group_left",
    "group_right",
];
const KEYWORDS: &[&str] = &[
    "by",
    "without",
    "on",
    "ignoring",
    "group_left",
    "group_right",
    "bool",
    "offset",
    "and",
    "or",
    "unless",
    "atan2",
];

#[derive(Debug, PartialEq, Eq)]
enum Position {
    Expression,
    LabelName,
    Nothing,
}

/// Where a word that starts after `before` sits: strings and range selectors take no
/// completion, `{` and `,` inside braces start a label name.
fn position(before: &str) -> Position {
    let mut stack: Vec<char> = Vec::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in before.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && q != '`' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '[' | '(' => stack.push(c),
            '}' | ']' | ')' => {
                stack.pop();
            }
            _ => {}
        }
    }
    if quote.is_some() {
        return Position::Nothing;
    }
    match stack.last() {
        Some('[') => Position::Nothing,
        // `by (`, `without (`, `on (`…: label names.
        Some('(') if after_grouping_keyword(before) => Position::LabelName,
        Some('{') => match before.trim_end().chars().last() {
            Some('{' | ',') => Position::LabelName,
            _ => Position::Nothing,
        },
        _ => Position::Expression,
    }
}

/// The word that ends `s`.
fn word_before(s: &str) -> String {
    s.trim_end()
        .chars()
        .rev()
        .take_while(|c| is_word(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// The unclosed `(` of `before` when it follows `by`, `without`, `on`, `ignoring` or `group_*`.
fn grouping_paren(before: &str) -> Option<usize> {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut quote: Option<char> = None;
    for (i, c) in before.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '[' | '(' => stack.push((c, i)),
            '}' | ']' | ')' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let (bracket, open) = *stack.last()?;
    (bracket == '(' && GROUPING.contains(&word_before(&before[..open]).as_str())).then_some(open)
}

fn after_grouping_keyword(before: &str) -> bool {
    grouping_paren(before).is_some()
}

/// Whether `before` is the first argument of a call that takes a number there (`topk(`).
fn in_number_argument(before: &str) -> bool {
    let mut stack: Vec<(char, usize, bool)> = Vec::new();
    let mut quote: Option<char> = None;
    for (i, c) in before.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '[' | '(' => stack.push((c, i, false)),
            '}' | ']' | ')' => {
                stack.pop();
            }
            ',' => {
                if let Some(top) = stack.last_mut() {
                    top.2 = true;
                }
            }
            _ => {}
        }
    }
    let Some(&('(', open, false)) = stack.last() else {
        return false;
    };
    NUMBER_CALLS.contains(&word_before(&before[..open]).as_str())
}

/// Whether an expression may start after `before`: at the start, after `(` or `,`, or after an
/// operator. After a complete operand an operator comes next, which isn't offered.
fn expression_starts(before: &str) -> bool {
    let trimmed = before.trim_end();
    match trimmed.chars().last() {
        None => true,
        Some('(' | ',' | '+' | '-' | '*' | '/' | '%' | '^' | '=' | '<' | '>' | '~') => true,
        _ => {
            let word: String = trimmed
                .chars()
                .rev()
                .take_while(|c| is_word(*c))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            matches!(word.as_str(), "and" | "or" | "unless" | "bool" | "atan2")
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == ':'
}

/// Suggestions for the word at the end of `text`, at most `limit`: names that start with it
/// first, then names that contain it.
pub fn complete(text: &str, names: &Names, limit: usize) -> Option<Completion> {
    complete_with(text, names, limit, false)
}

/// [`complete`]; `explicit` (Ctrl+Space) also completes where nothing is being typed and
/// offers the most things: after a closed `)` too.
pub fn complete_with(
    text: &str,
    names: &Names,
    limit: usize,
    explicit: bool,
) -> Option<Completion> {
    if let Some(completion) = quoted_metric(text, names, limit) {
        return Some(completion);
    }
    if let Some(completion) = match_operators(text) {
        return Some(completion);
    }
    if innermost(text) == Some('[') {
        return durations(text);
    }
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(text.len(), |(i, _)| i);
    let word = &text[start..];
    if word.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let before = &text[..start];
    if !brackets(before).1 && before.trim_end().ends_with('@') {
        return at_modifier(word, start);
    }
    let position = position(before);
    // Like the Prometheus UI, an empty word still completes where a name may start.
    if word.is_empty() {
        match position {
            Position::Nothing => return None,
            Position::LabelName => {}
            Position::Expression if expression_starts(before) => {}
            Position::Expression => return after_operand(text, explicit),
        }
    }
    // `topk(` takes a number first, not a series.
    let numbers_only = position == Position::Expression && in_number_argument(before);
    if numbers_only && word.is_empty() {
        return None;
    }
    let lower = word.to_lowercase();
    // One letter matches too much to be a search: names that start with it only.
    let contains = lower.len() > 1;
    let mut found: Vec<(u8, usize, &str, Kind)> = Vec::new();
    let mut lists: Vec<(&[String], Kind)> = Vec::new();
    match position {
        Position::Nothing => return None,
        Position::LabelName => lists.push((&names.labels, Kind::Label)),
        Position::Expression if !numbers_only => lists.push((&names.metrics, Kind::Metric)),
        Position::Expression => {}
    }
    for (list, kind) in lists {
        for name in list {
            if let Some(rank) = rank(name, &lower, contains) {
                found.push((rank, name.len(), name, kind));
            }
        }
    }
    if position == Position::Expression {
        let mut fixed: Vec<(&[&'static str], Kind)> = Vec::new();
        if !numbers_only {
            fixed.push((FUNCTIONS, Kind::Function));
            fixed.push((KEYWORDS, Kind::Keyword));
        }
        if numbers_only || expression_starts(before) {
            fixed.push((&NUMBERS, Kind::Keyword));
        }
        for (list, kind) in fixed {
            for name in list {
                if let Some(rank) = rank(name, &lower, contains) {
                    found.push((rank, name.len(), name, kind));
                }
            }
        }
    }
    found.sort_unstable_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    found.dedup_by(|a, b| a.2 == b.2);
    found.truncate(limit);
    if found.is_empty() || (found.len() == 1 && found[0].2 == word) {
        return None;
    }
    let mut items = Vec::new();
    for (_, _, name, kind) in found {
        let insert = match kind {
            Kind::Function => format!("{name}("),
            Kind::Keyword if GROUPING.contains(&name) => format!("{name} ("),
            Kind::Keyword => format!("{name} "),
            _ => name.to_string(),
        };
        let mut item = Item::new(name, insert, kind);
        if kind == Kind::Metric {
            item.detail = metric_detail(names, name);
        }
        items.push(item);
        if kind == Kind::Function {
            for (function, label, template) in SNIPPETS {
                if *function == name {
                    items.push(Item::snippet(label, template));
                }
            }
        }
    }
    Some(Completion { start, items })
}

fn metric_detail(names: &Names, metric: &str) -> Option<String> {
    let (kind, help) = names.metadata.get(metric)?;
    Some(if help.is_empty() {
        kind.clone()
    } else {
        format!("{kind} · {help}")
    })
}

/// The usual ways to call a function, with the cursor where the series goes.
const SNIPPETS: &[(&str, &str, &str)] = &[
    ("rate", "rate(…[5m])", "rate(\u{1}[5m])"),
    ("irate", "irate(…[5m])", "irate(\u{1}[5m])"),
    ("increase", "increase(…[5m])", "increase(\u{1}[5m])"),
    ("delta", "delta(…[5m])", "delta(\u{1}[5m])"),
    (
        "histogram_quantile",
        "histogram_quantile(0.95, sum by (le) (rate(…[5m])))",
        "histogram_quantile(0.95, sum by (le) (rate(\u{1}[5m])))",
    ),
    ("sum", "sum by (…) (…)", "sum by (\u{1}) ()"),
    ("avg", "avg by (…) (…)", "avg by (\u{1}) ()"),
    ("max", "max by (…) (…)", "max by (\u{1}) ()"),
    ("min", "min by (…) (…)", "min by (\u{1}) ()"),
    ("count", "count by (…) (…)", "count by (\u{1}) ()"),
    ("topk", "topk(10, …)", "topk(10, \u{1})"),
    (
        "label_replace",
        "label_replace(…)",
        "label_replace(\u{1}, \"dst\", \"$1\", \"src\", \"(.*)\")",
    ),
];

const NUMBERS: [&str; 2] = ["NaN", "Inf"];
/// Calls whose first argument is a number (or a label name), not a series.
const NUMBER_CALLS: [&str; 5] = ["topk", "bottomk", "limitk", "limit_ratio", "count_values"];

/// `start()` and `end()` after `@`.
fn at_modifier(word: &str, start: usize) -> Option<Completion> {
    let lower = word.to_lowercase();
    let items: Vec<Item> = ["start()", "end()"]
        .into_iter()
        .filter(|f| f.starts_with(&lower))
        .map(|f| Item::new(f, f, Kind::Function))
        .collect();
    (!items.is_empty()).then_some(Completion { start, items })
}

/// Metric names inside quotes at the start of a matcher: `{"http.requests"`.
fn quoted_metric(text: &str, names: &Names, limit: usize) -> Option<Completion> {
    let brace = scan_braces(text)?;
    let quote = brace.quote?;
    if !text[quote..].starts_with('"') {
        return None;
    }
    let matcher_start = brace.commas.last().map_or(brace.at, |c| *c) + 1;
    if !text[matcher_start..quote].trim().is_empty() {
        return None;
    }
    let lower = text[quote + 1..].to_lowercase();
    let mut found: Vec<(u8, usize, &String)> = names
        .metrics
        .iter()
        .filter_map(|m| Some((rank(m, &lower, !lower.is_empty())?, m.len(), m)))
        .collect();
    found.sort_unstable_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    found.truncate(limit);
    if found.is_empty() {
        return None;
    }
    Some(Completion {
        start: quote + 1,
        items: found
            .into_iter()
            .map(|(_, _, m)| {
                let mut item = Item::new(m.clone(), format!("{m}\""), Kind::Metric);
                item.detail = metric_detail(names, m);
                item
            })
            .collect(),
    })
}

const MATCH_OPERATORS: [&str; 4] = ["=", "!=", "=~", "!~"];
const UNITS: [&str; 7] = ["y", "w", "d", "h", "m", "s", "ms"];
const BINARY_OPERATORS: [&str; 12] = [
    "+", "-", "*", "/", "%", "^", "==", "!=", ">", "<", ">=", "<=",
];
const AGGREGATIONS: [&str; 14] = [
    "sum",
    "min",
    "max",
    "avg",
    "count",
    "group",
    "stddev",
    "stdvar",
    "topk",
    "bottomk",
    "quantile",
    "count_values",
    "limitk",
    "limit_ratio",
];

/// The innermost unclosed bracket of `text` and whether a string is open.
fn brackets(text: &str) -> (Option<char>, bool) {
    let mut stack: Vec<char> = Vec::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in text.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && q != '`' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '[' | '(' => stack.push(c),
            '}' | ']' | ')' => {
                stack.pop();
            }
            _ => {}
        }
    }
    (stack.last().copied(), quote.is_some())
}

/// The innermost unclosed bracket, `None` inside a string.
fn innermost(text: &str) -> Option<char> {
    match brackets(text) {
        (bracket, false) => bracket,
        (_, true) => None,
    }
}

/// The matcher being typed in `{…}`: what follows the `{` or the last `,`.
fn current_matcher(text: &str) -> Option<&str> {
    if innermost(text) != Some('{') {
        return None;
    }
    let mut depth: Vec<(char, usize)> = Vec::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && q != '`' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '[' | '(' => depth.push((c, i + 1)),
            '}' | ']' | ')' => {
                depth.pop();
            }
            ',' => {
                if let Some(top) = depth.last_mut() {
                    top.1 = i + 1;
                }
            }
            _ => {}
        }
    }
    depth.last().map(|(_, start)| &text[*start..])
}

/// `=`, `!=`, `=~`, `!~` after a label name in `{…}`.
fn match_operators(text: &str) -> Option<Completion> {
    let matcher = current_matcher(text)?.trim_start();
    let name_len = matcher.chars().take_while(|c| is_word(*c)).count();
    if name_len == 0 {
        return None;
    }
    let rest = &matcher[name_len..];
    let typed = rest.trim_start();
    if !typed.chars().all(|c| matches!(c, '=' | '!' | '~')) {
        return None;
    }
    // Still typing the name: a space or an operator character ends it.
    if rest.is_empty() {
        return None;
    }
    let items: Vec<Item> = MATCH_OPERATORS
        .iter()
        .filter(|op| op.starts_with(typed) && **op != typed)
        .map(|op| Item::new(*op, *op, Kind::Operator))
        .collect();
    (!items.is_empty()).then_some(Completion {
        start: text.len() - typed.len(),
        items,
    })
}

/// Units after a number in `[5`: `5m`, `5s`… Compound durations (`1h30`) complete their last part.
fn durations(text: &str) -> Option<Completion> {
    let unit_len = text
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    let (head, unit) = text.split_at(text.len() - unit_len);
    let number: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if number.is_empty() {
        return None;
    }
    let matching: Vec<&str> = UNITS
        .iter()
        .copied()
        .filter(|u| u.starts_with(unit))
        .collect();
    if matching.is_empty() || matching == [unit] {
        return None;
    }
    Some(Completion {
        start: text.len() - unit.len(),
        items: matching
            .into_iter()
            .map(|u| Item::new(format!("{number}{u}"), u, Kind::Unit))
            .collect(),
    })
}

/// The word an operand ends with, and whether it is a call's closing `)` of an aggregation.
fn aggregation_before(trimmed: &str) -> bool {
    let word_before = |s: &str| -> String {
        s.trim_end()
            .chars()
            .rev()
            .take_while(|c| is_word(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    if AGGREGATIONS.contains(&word_before(trimmed).as_str()) {
        return true;
    }
    if !trimmed.ends_with(')') {
        return false;
    }
    // The call's own `(`.
    let mut depth = 0;
    for (i, c) in trimmed.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    let head = &trimmed[..i];
                    // Already grouped: `sum by (job) (x)` has a `by (...)` before.
                    return AGGREGATIONS.contains(&word_before(head).as_str());
                }
            }
            _ => {}
        }
    }
    false
}

/// What follows a finished operand: binary operators, `offset`, and `by`/`without` after an
/// aggregation. Only after a space, or when asked for.
fn after_operand(text: &str, explicit: bool) -> Option<Completion> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() || (trimmed.len() == text.len() && !explicit) {
        return None;
    }
    let mut items: Vec<Item> = Vec::new();
    let mut add = |label: &str, insert: String, kind: Kind| {
        items.push(Item::new(label, insert, kind));
    };
    if aggregation_before(trimmed) {
        add("by", "by (".into(), Kind::Keyword);
        add("without", "without (".into(), Kind::Keyword);
    }
    add("offset", "offset ".into(), Kind::Keyword);
    for op in BINARY_OPERATORS {
        add(op, format!("{op} "), Kind::Operator);
    }
    for op in ["and", "or", "unless"] {
        add(op, format!("{op} "), Kind::Keyword);
    }
    Some(Completion {
        start: text.len(),
        items,
    })
}

/// `rqt` is a subsequence of `http_requests_total`: the Prometheus UI matches fuzzily.
fn is_subsequence(word: &[u8], name: &[u8]) -> bool {
    let mut chars = name.iter();
    word.iter()
        .all(|w| chars.any(|c| c.eq_ignore_ascii_case(w)))
}

/// What the server has to be asked before the word at the end of a text can be completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Values of `label` (`job="` completes them).
    Values {
        label: String,
        selector: Option<String>,
    },
    /// Label names of the series a selector matches (`metric{` completes them).
    Labels { selector: String },
}

impl Request {
    /// A cache key for the answer.
    pub fn key(&self) -> String {
        match self {
            Request::Values { label, selector } => {
                format!("values:{label}:{}", selector.as_deref().unwrap_or_default())
            }
            Request::Labels { selector } => format!("labels:{selector}"),
        }
    }
}

/// The request whose answer completes the word at the end of `text`, and where that word starts:
/// a label value inside quotes in `{…}`, or a label name in `{…}` of a selector with a metric or
/// other matchers. The series are scoped like the Prometheus UI does, by the metric name and
/// the complete matchers before the current one.
pub fn request(text: &str) -> Option<(Request, usize)> {
    if !brackets(text).1
        && let Some(open) = grouping_paren(text)
    {
        return grouping_request(text, open);
    }
    let brace = scan_braces(text)?;
    // Where the current matcher starts.
    let matcher_start = brace.commas.last().map_or(brace.at, |c| *c) + 1;
    let metric: String = {
        let before = text[..brace.at].trim_end();
        let word: String = before
            .chars()
            .rev()
            .take_while(|c| is_word(*c))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        // `sum by (job) {`? a word that isn't a metric name isn't one.
        if word.is_empty()
            || word.starts_with(|c: char| c.is_ascii_digit())
            || FUNCTIONS.contains(&word.as_str())
            || KEYWORDS.contains(&word.as_str())
        {
            String::new()
        } else {
            word
        }
    };
    // The complete matchers before the current one.
    let mut bounds = vec![brace.at];
    bounds.extend(&brace.commas);
    let matchers: Vec<&str> = bounds
        .windows(2)
        .map(|w| text[w[0] + 1..w[1]].trim())
        .filter(|m| is_complete_matcher(m))
        .collect();
    let selector = if metric.is_empty() && matchers.is_empty() {
        None
    } else {
        Some(format!("{metric}{{{}}}", matchers.join(",")))
    };
    if let Some(q) = brace.quote {
        if !text[q..].starts_with('"') {
            return None;
        }
        // A value: `label op "partial`.
        let head = text[matcher_start..q].trim_end_matches(['=', '!', '~', ' ']);
        let label = head.trim();
        if label.is_empty()
            || !label.chars().all(is_word)
            || label == "__name__" && selector.is_none()
        {
            return None;
        }
        // A selector without a metric and matchers can still ask for the values of the label.
        return Some((
            Request::Values {
                label: label.to_string(),
                selector,
            },
            q + 1,
        ));
    }
    // A label name: right after `{` or `,`, scoped only when something scopes it.
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(text.len(), |(i, _)| i);
    if !matches!(text[..start].trim_end().chars().last(), Some('{' | ',')) {
        return None;
    }
    selector.map(|selector| (Request::Labels { selector }, start))
}

/// Label values that complete the partial value at `start`, as `value"` with the escapes a
/// PromQL string needs.
pub fn complete_values(
    text: &str,
    start: usize,
    values: &[String],
    limit: usize,
) -> Option<Completion> {
    // The partial is PromQL string text: match what it says, not its escapes.
    let partial = text
        .get(start..)?
        .replace("\\\"", "\"")
        .replace("\\\\", "\\");
    let lower = partial.to_lowercase();
    let mut found: Vec<(u8, usize, &String)> = values
        .iter()
        .filter_map(|v| Some((rank(v, &lower, !lower.is_empty())?, v.len(), v)))
        .collect();
    found.sort_unstable_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    found.truncate(limit);
    if found.is_empty() {
        return None;
    }
    Some(Completion {
        start,
        items: found
            .into_iter()
            .map(|(_, _, v)| {
                Item::new(
                    v.clone(),
                    format!("{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
                    Kind::Value,
                )
            })
            .collect(),
    })
}

/// The innermost unclosed `{` of a text: where it is, the commas that split its matchers and
/// the string that is open in it.
struct BraceScan {
    at: usize,
    commas: Vec<usize>,
    quote: Option<usize>,
}

fn scan_braces(text: &str) -> Option<BraceScan> {
    let mut stack: Vec<(char, Option<BraceScan>)> = Vec::new();
    let mut quote: Option<(char, usize)> = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if let Some((q, _)) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' && q != '`' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some((c, i)),
            '{' => stack.push((
                c,
                Some(BraceScan {
                    at: i,
                    commas: Vec::new(),
                    quote: None,
                }),
            )),
            '[' | '(' => stack.push((c, None)),
            '}' | ']' | ')' => {
                stack.pop();
            }
            ',' => {
                if let Some((_, Some(brace))) = stack.last_mut() {
                    brace.commas.push(i);
                }
            }
            _ => {}
        }
    }
    let (_, brace) = stack.pop()?;
    let mut brace = brace?;
    brace.quote = quote.map(|(_, i)| i);
    Some(brace)
}

/// `name op "value"`: anything else would make the selector invalid for the server.
fn is_complete_matcher(matcher: &str) -> bool {
    let name_len = matcher.chars().take_while(|c| is_word(*c)).count();
    if name_len == 0 {
        return false;
    }
    let rest = matcher[name_len..].trim_start();
    let value = rest.trim_start_matches(['=', '!', '~']);
    let op = &rest[..rest.len() - value.len()];
    MATCH_OPERATORS.contains(&op)
        && value.trim_start().len() >= 2
        && value.trim_start().starts_with('"')
        && value.ends_with('"')
}

/// `s` without its `by (…)` and `without (…)` lists, which hold label names, not series.
fn strip_groupings(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = ["by", "without"]
        .iter()
        .filter_map(|kw| find_keyword(rest, kw))
        .min()
    {
        let after = rest[i..]
            .trim_start_matches(|c: char| is_word(c))
            .trim_start();
        out.push_str(&rest[..i]);
        if let Some(list) = after.strip_prefix('(') {
            // Skip to the matching `)`.
            let mut depth = 1;
            let mut end = list.len();
            for (j, c) in list.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = j + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &list[end.min(list.len())..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Where the whole word `keyword` starts in `s`.
fn find_keyword(s: &str, keyword: &str) -> Option<usize> {
    s.match_indices(keyword).map(|(i, _)| i).find(|&i| {
        let before = s[..i].chars().next_back();
        let after = s[i + keyword.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// The first metric name inside the call that `s` ends with (its closing `)`).
fn aggregate_metric(s: &str) -> Option<String> {
    let mut depth = 0;
    let mut open = None;
    for (i, c) in s.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    open = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = strip_groupings(&s[open? + 1..s.len() - 1]);
    let inner = inner.as_str();
    let mut quote: Option<char> = None;
    let mut nest = 0;
    let mut token = String::new();
    let mut found = None;
    let chars: Vec<char> = inner.chars().chain([' ']).collect();
    for (i, c) in chars.iter().copied().enumerate() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if nest == 0 && is_word(c) {
            token.push(c);
            continue;
        }
        if !token.is_empty() {
            let next = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
            if !token.starts_with(|c: char| c.is_ascii_digit())
                && !FUNCTIONS.contains(&token.as_str())
                && !KEYWORDS.contains(&token.as_str())
                && next != Some('(')
            {
                found = Some(std::mem::take(&mut token));
                break;
            }
            token.clear();
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '[' => nest += 1,
            '}' | ']' => nest -= 1,
            _ => {}
        }
    }
    found
}

/// Label names for `sum(metric) by (`: those of the aggregated series.
fn grouping_request(text: &str, open: usize) -> Option<(Request, usize)> {
    let keyword = word_before(&text[..open]);
    if keyword != "by" && keyword != "without" {
        return None;
    }
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(text.len(), |(i, _)| i);
    if !matches!(text[..start].trim_end().chars().last(), Some('(' | ',')) {
        return None;
    }
    let before = text[..open]
        .trim_end()
        .strip_suffix(keyword.as_str())?
        .trim_end();
    if !before.ends_with(')') {
        return None;
    }
    let metric = aggregate_metric(before)?;
    Some((
        Request::Labels {
            selector: format!("{metric}{{}}"),
        },
        start,
    ))
}

/// How well `name` matches the lower-case word: 0 prefix, 1 contains, 2 subsequence. Compares
/// bytes without case (no allocation: this runs over every metric name on every keystroke).
fn rank(name: &str, lower_word: &str, contains: bool) -> Option<u8> {
    let (name, word) = (name.as_bytes(), lower_word.as_bytes());
    if name.len() >= word.len() && name[..word.len()].eq_ignore_ascii_case(word) {
        Some(0)
    } else if contains
        && name
            .windows(word.len())
            .any(|w| w.eq_ignore_ascii_case(word))
    {
        Some(1)
    } else if contains && is_subsequence(word, name) {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            metrics: [
                "up",
                "http_requests_total",
                "http_request_duration_seconds_bucket",
                "node_cpu_seconds_total",
            ]
            .map(String::from)
            .to_vec(),
            labels: ["job", "instance", "__name__"].map(String::from).to_vec(),
            metadata: [(
                "http_requests_total".to_string(),
                ("counter".to_string(), "Requests served.".to_string()),
            )]
            .into(),
        }
    }

    fn labels(text: &str) -> Vec<String> {
        complete(text, &names(), 10)
            .map(|c| c.items.into_iter().map(|i| i.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn prefix_before_contains() {
        assert_eq!(
            labels("http_req"),
            [
                "http_requests_total",
                "http_request_duration_seconds_bucket"
            ]
        );
        let found = labels("seconds");
        assert!(found.contains(&"node_cpu_seconds_total".to_string()));
    }

    #[test]
    fn functions_and_keywords_complete_with_their_syntax() {
        let completion = complete("sum(rat", &names(), 10).unwrap();
        assert_eq!(completion.start, 4);
        assert_eq!(completion.items[0].label, "rate");
        assert_eq!(completion.items[0].insert, "rate(");
        let by = complete("sum by", &names(), 10).unwrap();
        assert_eq!(
            by.items.iter().find(|i| i.label == "by").unwrap().insert,
            "by ("
        );
    }

    #[test]
    fn one_letter_only_matches_prefixes() {
        let found = labels("u");
        assert!(found.contains(&"up".to_string()));
        assert!(!found.contains(&"http_requests_total".to_string()));
    }

    #[test]
    fn label_names_inside_braces() {
        assert_eq!(labels(r#"up{jo"#), ["job"]);
        assert_eq!(labels(r#"up{job="a", ins"#), ["instance"]);
        assert!(
            labels(r#"up{job=ins"#).is_empty(),
            "after `=` is a value, not a name"
        );
    }

    #[test]
    fn nothing_in_strings_ranges_or_numbers() {
        assert!(labels(r#"up{job="ht"#).is_empty());
        assert!(labels("rate(up[5s").is_empty());
        assert!(labels("up > 10").is_empty());
    }

    #[test]
    fn empty_words_complete_where_a_name_may_start() {
        let starts = |text: &str| complete(text, &names(), 10).is_some();
        assert!(starts(""), "an empty box");
        assert!(starts("sum("));
        assert!(starts("rate(up[5m]) + "));
        assert!(starts("a_total, "));
        assert!(starts("up and "));
        assert!(starts("up "), "after an operand come operators");
        assert!(!starts("sum(up)"));
        assert!(!starts("up{job=\""), "no values yet");
        assert!(!starts("up[")); // no number yet
    }

    #[test]
    fn empty_words_in_braces_and_groupings_offer_label_names() {
        assert_eq!(labels("up{"), ["job", "__name__", "instance"]);
        assert_eq!(labels("sum by ("), labels("up{"));
        assert_eq!(labels("sum(up) without (job, "), labels("up{"));
    }

    #[test]
    fn matching_is_fuzzy_after_prefix_and_contains() {
        let found = labels("hrt");
        assert!(found.contains(&"http_requests_total".to_string()));
    }

    #[test]
    fn values_are_scoped_by_the_metric_and_the_other_matchers() {
        let (request, start) = request(r#"apiserver_request_total{job=""#).unwrap();
        assert_eq!(
            request,
            Request::Values {
                label: "job".into(),
                selector: Some("apiserver_request_total{}".into())
            }
        );
        assert_eq!(start, r#"apiserver_request_total{job=""#.len());
        let text = r#"sum(up{namespace="a", job=~"ap"#;
        let (request, start) = self::request(text).unwrap();
        assert_eq!(
            request,
            Request::Values {
                label: "job".into(),
                selector: Some(r#"up{namespace="a"}"#.into())
            }
        );
        assert_eq!(&text[start..], "ap");
        let (request, _) = self::request(r#"{job=""#).unwrap();
        assert_eq!(
            request,
            Request::Values {
                label: "job".into(),
                selector: None
            },
            "no metric, nothing to scope by"
        );
    }

    #[test]
    fn label_names_are_scoped_when_something_scopes_them() {
        let (request, start) = request("up{jo").unwrap();
        assert_eq!(
            request,
            Request::Labels {
                selector: "up{}".into()
            }
        );
        assert_eq!(start, 3);
        assert!(
            request_none("{jo"),
            "unscoped names come from the global list"
        );
        assert!(request_none("sum by (jo"));
    }

    fn request_none(text: &str) -> bool {
        request(text).is_none()
    }

    #[test]
    fn values_complete_with_their_escapes() {
        let values = ["apiserver", "kube-apiserver", "we\"ird"]
            .map(String::from)
            .to_vec();
        let text = r#"up{job=""#;
        let completion = complete_values(text, text.len(), &values, 10).unwrap();
        assert_eq!(completion.items.len(), 3);
        let text = r#"up{job="api"#;
        let completion = complete_values(text, text.len() - 3, &values, 10).unwrap();
        let labels: Vec<_> = completion.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["apiserver", "kube-apiserver"], "prefix first");
        assert_eq!(completion.items[0].insert, "apiserver\"");
        let text = r#"up{job=""#;
        let weird = complete_values(text, text.len(), &values, 10).unwrap();
        let weird = weird
            .items
            .iter()
            .find(|i| i.label.contains("ird"))
            .unwrap();
        assert_eq!(weird.insert, r#"we\"ird""#);
    }

    #[test]
    fn match_operators_follow_a_label_name() {
        let ops = |text: &str| labels(text);
        assert_eq!(ops("up{job "), ["=", "!=", "=~", "!~"]);
        assert_eq!(ops("up{job!"), ["!=", "!~"]);
        assert_eq!(ops("up{a=\"x\", job="), ["=~"]);
        assert!(ops("up{job=~").is_empty());
        assert!(
            ops("up{job").is_empty() || ops("up{job") != ["="],
            "still typing the name"
        );
        let completion = complete("up{job !", &names(), 10).unwrap();
        assert_eq!(completion.start, 7);
    }

    #[test]
    fn durations_take_units_after_a_number() {
        let units = |text: &str| labels(text);
        assert_eq!(
            units("rate(up[5"),
            ["5y", "5w", "5d", "5h", "5m", "5s", "5ms"]
        );
        assert_eq!(units("rate(up[5m"), ["5m", "5ms"]);
        assert!(units("rate(up[5s").is_empty(), "a finished unit");
        assert!(units("rate(up[").is_empty(), "no number yet");
        let completion = complete("rate(up[1h3", &names(), 10).unwrap();
        assert_eq!(completion.start, 11);
        assert_eq!(completion.items[0].insert, "y");
        assert_eq!(completion.items[0].label, "3y");
    }

    #[test]
    fn operators_follow_an_operand() {
        let after = |text: &str| labels(text);
        let found = after("up ");
        assert_eq!(found[0], "offset");
        assert!(found.contains(&"==".to_string()) && found.contains(&"unless".to_string()));
        assert!(!found.contains(&"by".to_string()));
        assert_eq!(&after("sum ")[..2], ["by", "without"]);
        assert_eq!(&after("sum(up) ")[..2], ["by", "without"]);
        assert_eq!(
            after("rate(up[5m]) ")[0],
            "offset",
            "rate isn't an aggregation"
        );
        assert!(after("sum(up)").is_empty(), "not after a space");
        let explicit = complete_with("sum(up)", &names(), 10, true).unwrap();
        assert_eq!(explicit.items[0].label, "by");
        assert_eq!(explicit.start, 7);
    }

    #[test]
    fn at_modifier_offers_start_and_end() {
        assert_eq!(labels("up @ "), ["start()", "end()"]);
        assert_eq!(labels("up @ st"), ["start()"]);
        assert!(labels(r#"up{job="@ "#).is_empty(), "not inside a string");
    }

    #[test]
    fn quoted_metric_names_start_a_matcher() {
        let completion = complete(r#"{"http_req"#, &names(), 10).unwrap();
        assert_eq!(completion.start, 2);
        assert_eq!(completion.items[0].label, "http_requests_total");
        assert_eq!(completion.items[0].insert, "http_requests_total\"");
        assert!(
            complete(r#"up{job=""#, &names(), 10).is_none(),
            "a value isn't a metric"
        );
        assert!(complete(r#"{job="a", "up"#, &names(), 10).is_some());
    }

    #[test]
    fn number_arguments_and_nan() {
        assert!(
            labels("topk(").is_empty(),
            "a number comes first, not a series"
        );
        assert_eq!(labels("topk(na"), ["NaN"]);
        assert!(
            labels("topk(3, up").contains(&"up".to_string()),
            "after the comma it's a series"
        );
        assert!(labels("sum(n").contains(&"NaN".to_string()));
    }

    #[test]
    fn functions_come_with_snippets() {
        let completion = complete("rat", &names(), 10).unwrap();
        let labels: Vec<_> = completion.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(&labels[..2], ["rate", "rate(…[5m])"]);
        let snippet = &completion.items[1];
        assert_eq!(snippet.insert, "rate([5m])");
        assert_eq!(
            snippet.cursor,
            Some(5),
            "inside the brackets, where the series goes"
        );
        assert_eq!(snippet.kind, Kind::Snippet);
        let sum = complete("su", &names(), 10).unwrap();
        let by = sum.items.iter().find(|i| i.kind == Kind::Snippet).unwrap();
        assert_eq!(by.insert, "sum by () ()");
        assert_eq!(by.cursor, Some(8));
    }

    #[test]
    fn metrics_carry_their_help() {
        let completion = complete("http_requests", &names(), 10).unwrap();
        assert_eq!(
            completion.items[0].detail.as_deref(),
            Some("counter · Requests served.")
        );
        assert_eq!(complete("up", &names(), 10).unwrap().items[0].detail, None);
    }

    #[test]
    fn groupings_are_scoped_to_the_aggregated_metric() {
        let scope = |text: &str| request(text).map(|(r, _)| r);
        let labels_of = |metric: &str| Request::Labels {
            selector: format!("{metric}{{}}"),
        };
        assert_eq!(scope("sum(up) by ("), Some(labels_of("up")));
        assert_eq!(
            scope("sum(rate(http_requests_total[5m])) without (a, "),
            Some(labels_of("http_requests_total"))
        );
        assert_eq!(scope(r#"sum(up{job="a"}) by ("#), Some(labels_of("up")));
        assert_eq!(scope("sum by ("), None, "nothing to scope by yet");
        assert_eq!(scope("up / on ("), None, "matching labels aren't scoped");
    }

    #[test]
    fn nested_aggregations_scope_to_the_series_not_a_label() {
        let scope = |text: &str| request(text).map(|(r, _)| r);
        assert_eq!(
            scope("sum(sum by (job) (up)) by ("),
            Some(Request::Labels {
                selector: "up{}".into()
            })
        );
    }

    #[test]
    fn half_typed_matchers_never_reach_the_server() {
        let selector = |text: &str| match request(text) {
            Some((Request::Values { selector, .. }, _)) => selector,
            other => panic!("{other:?}"),
        };
        assert_eq!(selector(r#"up{job=, instance=""#).as_deref(), Some("up{}"));
        assert_eq!(
            selector(r#"up{job="a", instance=""#).as_deref(),
            Some(r#"up{job="a"}"#)
        );
        assert_eq!(selector(r#"{job=~, x=""#), None);
        assert!(
            request("up{job='ap").is_none(),
            "single quotes hold no values"
        );
    }

    #[test]
    fn values_match_what_the_partial_says() {
        let values = vec!["we\"ird".to_string()];
        let text = r#"up{job="we\"i"#;
        let start = text.len() - r#"we\"i"#.len();
        assert!(complete_values(text, start, &values, 10).is_some());
    }

    #[test]
    fn an_exact_match_alone_is_done() {
        assert!(complete("http_requests_total", &names(), 10).is_none());
    }

    #[test]
    fn works_after_an_operator() {
        assert_eq!(
            labels("up > http"),
            [
                "http_requests_total",
                "http_request_duration_seconds_bucket"
            ]
        );
        assert!(complete("ünï", &names(), 10).is_none());
    }
}
