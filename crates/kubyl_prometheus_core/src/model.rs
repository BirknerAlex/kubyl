//! What the Prometheus HTTP API returns, parsed leniently: Thanos Query, VictoriaMetrics and
//! older Prometheus versions leave fields out, so every field is optional.

use std::collections::BTreeMap;

use jiff::Timestamp;
use kubyl_metrics_core::prometheus::PromError;
use serde_json::Value;

pub type Labels = BTreeMap<String, String>;

/// The `data` of a successful answer.
pub fn data(body: &Value) -> Result<&Value, PromError> {
    match body.get("status").and_then(Value::as_str) {
        Some("success") => Ok(&body["data"]),
        Some("error") => Err(PromError::Query(
            body.get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
        )),
        _ => Err(PromError::Invalid("missing status".into())),
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn labels(value: &Value) -> Labels {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn time(value: &Value, key: &str) -> Option<Timestamp> {
    let raw = value.get(key)?.as_str()?;
    // Prometheus writes `0001-01-01T00:00:00Z` for "never".
    raw.parse::<Timestamp>().ok().filter(|t| t.as_second() > 0)
}

/// A number the API may send as a JSON number or a string.
fn number(value: &Value, key: &str) -> Option<f64> {
    let v = value.get(key)?;
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

// ----- Overview -----

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildInfo {
    pub version: Option<String>,
    pub revision: Option<String>,
    pub branch: Option<String>,
    pub build_date: Option<String>,
    pub go_version: Option<String>,
}

pub fn parse_build_info(body: &Value) -> Result<BuildInfo, PromError> {
    let d = data(body)?;
    Ok(BuildInfo {
        version: text(d, "version"),
        revision: text(d, "revision"),
        branch: text(d, "branch"),
        build_date: text(d, "buildDate"),
        go_version: text(d, "goVersion"),
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimeInfo {
    pub start_time: Option<Timestamp>,
    pub last_config_time: Option<Timestamp>,
    pub reload_config_success: Option<bool>,
    pub corruption_count: Option<i64>,
    pub goroutines: Option<i64>,
    pub storage_retention: Option<String>,
    pub cwd: Option<String>,
}

pub fn parse_runtime_info(body: &Value) -> Result<RuntimeInfo, PromError> {
    let d = data(body)?;
    Ok(RuntimeInfo {
        start_time: time(d, "startTime"),
        last_config_time: time(d, "lastConfigTime"),
        reload_config_success: d.get("reloadConfigSuccess").and_then(Value::as_bool),
        corruption_count: number(d, "corruptionCount").map(|n| n as i64),
        goroutines: number(d, "goroutineCount").map(|n| n as i64),
        storage_retention: text(d, "storageRetention"),
        cwd: text(d, "CWD"),
    })
}

/// A `name → count` row of the TSDB status.
#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub name: String,
    pub value: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tsdb {
    pub series: Option<u64>,
    pub label_pairs: Option<u64>,
    pub chunks: Option<u64>,
    /// Unix milliseconds of the oldest and newest sample in the head block.
    pub min_time: Option<i64>,
    pub max_time: Option<i64>,
    pub series_by_metric: Vec<Stat>,
    pub label_value_counts: Vec<Stat>,
    pub memory_by_label: Vec<Stat>,
    pub series_by_pair: Vec<Stat>,
}

fn stats(value: &Value, key: &str) -> Vec<Stat> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(Stat {
                        name: row.get("name")?.as_str()?.to_string(),
                        value: number(row, "value")? as u64,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_tsdb(body: &Value) -> Result<Tsdb, PromError> {
    let d = data(body)?;
    let head = &d["headStats"];
    Ok(Tsdb {
        series: number(head, "numSeries").map(|n| n as u64),
        label_pairs: number(head, "numLabelPairs").map(|n| n as u64),
        chunks: number(head, "chunkCount").map(|n| n as u64),
        min_time: number(head, "minTime").map(|n| n as i64),
        max_time: number(head, "maxTime").map(|n| n as i64),
        series_by_metric: stats(d, "seriesCountByMetricName"),
        label_value_counts: stats(d, "labelValueCountByLabelName"),
        memory_by_label: stats(d, "memoryInBytesByLabelName"),
        series_by_pair: stats(d, "seriesCountByLabelValuePair"),
    })
}

/// What the Overview tab shows. Each part fails on its own (Thanos Query has no TSDB status).
#[derive(Clone, Debug, Default)]
pub struct Overview {
    pub build: Option<Result<BuildInfo, PromError>>,
    pub runtime: Option<Result<RuntimeInfo, PromError>>,
    pub tsdb: Option<Result<Tsdb, PromError>>,
}

// ----- Targets and service discovery -----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Health {
    Up,
    Down,
    Unknown,
}

impl Health {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "up" | "ok" => Health::Up,
            // Rules report `err`, targets `down`.
            "down" | "err" => Health::Down,
            _ => Health::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Health::Up => "up",
            Health::Down => "down",
            Health::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub pool: String,
    pub url: String,
    pub health: Health,
    pub labels: Labels,
    pub discovered: Labels,
    pub last_error: Option<String>,
    pub last_scrape: Option<Timestamp>,
    /// Seconds.
    pub scrape_duration: Option<f64>,
}

impl Target {
    /// `instance` label, else the URL's host.
    pub fn instance(&self) -> &str {
        self.labels
            .get("instance")
            .map(String::as_str)
            .unwrap_or(&self.url)
    }
}

/// A target that relabeling dropped: only what discovery knew.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub pool: String,
    pub discovered: Labels,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    pub active: Vec<Target>,
    pub dropped: Vec<Dropped>,
    /// Dropped targets per pool, as the server counts them (the list may be shorter).
    pub dropped_counts: BTreeMap<String, usize>,
}

pub fn parse_targets(body: &Value) -> Result<Targets, PromError> {
    let d = data(body)?;
    let mut out = Targets::default();
    for item in d["activeTargets"].as_array().into_iter().flatten() {
        out.active.push(Target {
            pool: text(item, "scrapePool").unwrap_or_default(),
            url: text(item, "scrapeUrl").unwrap_or_default(),
            health: Health::parse(item["health"].as_str().unwrap_or_default()),
            labels: labels(&item["labels"]),
            discovered: labels(&item["discoveredLabels"]),
            last_error: text(item, "lastError"),
            last_scrape: time(item, "lastScrape"),
            scrape_duration: number(item, "lastScrapeDuration"),
        });
    }
    for item in d["droppedTargets"].as_array().into_iter().flatten() {
        out.dropped.push(Dropped {
            pool: text(item, "scrapePool").unwrap_or_default(),
            discovered: labels(&item["discoveredLabels"]),
        });
    }
    if let Some(counts) = d["droppedTargetCounts"].as_object() {
        for (pool, count) in counts {
            out.dropped_counts
                .insert(pool.clone(), count.as_u64().unwrap_or_default() as usize);
        }
    } else {
        for dropped in &out.dropped {
            *out.dropped_counts.entry(dropped.pool.clone()).or_default() += 1;
        }
    }
    out.active
        .sort_by(|a, b| (&a.pool, &a.url).cmp(&(&b.pool, &b.url)));
    Ok(out)
}

/// One scrape pool: its targets and the dropped count.
#[derive(Clone, Debug, PartialEq)]
pub struct Pool {
    pub name: String,
    pub up: usize,
    pub down: usize,
    pub unknown: usize,
    pub dropped: usize,
}

impl Pool {
    pub fn total(&self) -> usize {
        self.up + self.down + self.unknown
    }
}

impl Targets {
    /// The pools in name order (pools with only dropped targets too).
    pub fn pools(&self) -> Vec<Pool> {
        let mut pools: BTreeMap<&str, Pool> = BTreeMap::new();
        for target in &self.active {
            let entry = pools.entry(&target.pool).or_insert_with(|| Pool {
                name: target.pool.clone(),
                up: 0,
                down: 0,
                unknown: 0,
                dropped: 0,
            });
            match target.health {
                Health::Up => entry.up += 1,
                Health::Down => entry.down += 1,
                Health::Unknown => entry.unknown += 1,
            }
        }
        for (name, count) in &self.dropped_counts {
            pools
                .entry(name)
                .or_insert_with(|| Pool {
                    name: name.clone(),
                    up: 0,
                    down: 0,
                    unknown: 0,
                    dropped: 0,
                })
                .dropped = *count;
        }
        pools.into_values().collect()
    }

    pub fn count(&self, health: Health) -> usize {
        self.active.iter().filter(|t| t.health == health).count()
    }
}

// ----- Rules -----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleKind {
    Alerting,
    Recording,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleAlert {
    pub labels: Labels,
    pub state: String,
    pub active_at: Option<Timestamp>,
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub name: String,
    pub kind: RuleKind,
    pub query: String,
    /// The `for` duration in seconds.
    pub duration: f64,
    pub labels: Labels,
    pub annotations: Labels,
    pub health: Health,
    pub last_error: Option<String>,
    pub evaluation_time: Option<f64>,
    pub last_evaluation: Option<Timestamp>,
    /// `inactive`, `pending` or `firing` (alerting rules).
    pub state: Option<String>,
    pub alerts: Vec<RuleAlert>,
}

impl Rule {
    pub fn firing(&self) -> bool {
        self.state.as_deref() == Some("firing")
    }

    pub fn pending(&self) -> bool {
        self.state.as_deref() == Some("pending")
    }

    pub fn failing(&self) -> bool {
        self.health == Health::Down
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleGroup {
    pub name: String,
    pub file: String,
    pub interval: f64,
    pub evaluation_time: Option<f64>,
    pub last_evaluation: Option<Timestamp>,
    pub rules: Vec<Rule>,
}

pub fn parse_rules(body: &Value) -> Result<Vec<RuleGroup>, PromError> {
    let d = data(body)?;
    let mut groups = Vec::new();
    for group in d["groups"].as_array().into_iter().flatten() {
        let mut rules = Vec::new();
        for rule in group["rules"].as_array().into_iter().flatten() {
            let kind = match rule["type"].as_str() {
                Some("recording") => RuleKind::Recording,
                Some(_) => RuleKind::Alerting,
                // Old servers: only alerting rules have `alerts`.
                None if rule.get("alerts").is_some() => RuleKind::Alerting,
                None => RuleKind::Recording,
            };
            let alerts = rule["alerts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| RuleAlert {
                    labels: labels(&a["labels"]),
                    state: text(a, "state").unwrap_or_default(),
                    active_at: time(a, "activeAt"),
                    value: text(a, "value"),
                })
                .collect();
            rules.push(Rule {
                name: text(rule, "name").unwrap_or_default(),
                kind,
                query: text(rule, "query").unwrap_or_default(),
                duration: number(rule, "duration").unwrap_or_default(),
                labels: labels(&rule["labels"]),
                annotations: labels(&rule["annotations"]),
                health: Health::parse(rule["health"].as_str().unwrap_or_default()),
                last_error: text(rule, "lastError"),
                evaluation_time: number(rule, "evaluationTime"),
                last_evaluation: time(rule, "lastEvaluation"),
                state: text(rule, "state"),
                alerts,
            });
        }
        groups.push(RuleGroup {
            name: text(group, "name").unwrap_or_default(),
            file: text(group, "file").unwrap_or_default(),
            interval: number(group, "interval").unwrap_or_default(),
            evaluation_time: number(group, "evaluationTime"),
            last_evaluation: time(group, "lastEvaluation"),
            rules,
        });
    }
    groups.sort_by(|a, b| (&a.file, &a.name).cmp(&(&b.file, &b.name)));
    Ok(groups)
}

// ----- Query results -----

/// One row of an instant query.
#[derive(Clone, Debug, PartialEq)]
pub struct InstantSample {
    pub labels: Labels,
    /// The value as the server wrote it (`NaN`, `+Inf` and 17 digits included).
    pub value: String,
}

/// One series of a range query.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeSeries {
    pub labels: Labels,
    pub points: Vec<(f64, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueryResult {
    Vector(Vec<InstantSample>),
    Matrix(Vec<RangeSeries>),
    Scalar(String),
    String(String),
}

impl QueryResult {
    pub fn len(&self) -> usize {
        match self {
            QueryResult::Vector(rows) => rows.len(),
            QueryResult::Matrix(series) => series.len(),
            QueryResult::Scalar(_) | QueryResult::String(_) => 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn sample_value(pair: &Value) -> Option<String> {
    match pair.get(1) {
        Some(Value::String(s)) => Some(s.clone()),
        // Native histograms: `[t, {"count": …, "sum": …}]`.
        Some(Value::Object(h)) => Some(format!(
            "histogram (count {}, sum {})",
            h.get("count").and_then(Value::as_str).unwrap_or("?"),
            h.get("sum").and_then(Value::as_str).unwrap_or("?")
        )),
        _ => None,
    }
}

pub fn parse_query(body: &Value) -> Result<QueryResult, PromError> {
    let d = data(body)?;
    let result = &d["result"];
    match d["resultType"].as_str().unwrap_or_default() {
        "vector" => Ok(QueryResult::Vector(
            result
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    let value = sample_value(item.get("value").or_else(|| item.get("histogram"))?)?;
                    Some(InstantSample {
                        labels: labels(&item["metric"]),
                        value,
                    })
                })
                .collect(),
        )),
        "matrix" => Ok(QueryResult::Matrix(
            result
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| RangeSeries {
                    labels: labels(&item["metric"]),
                    points: item["values"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|p| {
                            let t = p.get(0)?.as_f64()?;
                            let v: f64 = p.get(1)?.as_str()?.parse().ok()?;
                            v.is_finite().then_some((t, v))
                        })
                        .collect(),
                })
                .collect(),
        )),
        "scalar" => Ok(QueryResult::Scalar(
            sample_value(result).unwrap_or_default(),
        )),
        "string" => Ok(QueryResult::String(
            sample_value(result).unwrap_or_default(),
        )),
        other => Err(PromError::Invalid(format!("unknown result type {other:?}"))),
    }
}

/// The series name as the Prometheus UI writes it: `metric{a="b", c="d"}`.
pub fn series_name(labels: &Labels) -> String {
    let name = labels
        .get("__name__")
        .map(String::as_str)
        .unwrap_or_default();
    let rest: Vec<String> = labels
        .iter()
        .filter(|(k, _)| k.as_str() != "__name__")
        .map(|(k, v)| format!("{k}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    if rest.is_empty() {
        if name.is_empty() {
            "{}".into()
        } else {
            name.into()
        }
    } else {
        format!("{name}{{{}}}", rest.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn errors_come_from_the_status() {
        let err = data(&json!({"status": "error", "error": "parse error"})).unwrap_err();
        assert_eq!(err, PromError::Query("parse error".into()));
        assert!(data(&json!({"hello": 1})).is_err());
    }

    #[test]
    fn runtime_and_tsdb() {
        let runtime = parse_runtime_info(&json!({"status": "success", "data": {
            "startTime": "2026-09-01T10:00:00.5Z", "reloadConfigSuccess": true,
            "corruptionCount": 0, "goroutineCount": 87, "storageRetention": "15d",
            "lastConfigTime": "0001-01-01T00:00:00Z"
        }}))
        .unwrap();
        assert!(runtime.start_time.is_some());
        assert_eq!(runtime.last_config_time, None, "year 1 means never");
        assert_eq!(runtime.goroutines, Some(87));
        let tsdb = parse_tsdb(&json!({"status": "success", "data": {
            "headStats": {"numSeries": 1200, "numLabelPairs": 99, "chunkCount": 3000,
                          "minTime": 1000, "maxTime": 9000},
            "seriesCountByMetricName": [{"name": "up", "value": 40}, {"value": 1}]
        }}))
        .unwrap();
        assert_eq!(tsdb.series, Some(1200));
        assert_eq!(
            tsdb.series_by_metric.len(),
            1,
            "rows without a name are skipped"
        );
    }

    fn targets_body() -> Value {
        json!({"status": "success", "data": {
            "activeTargets": [
                {"scrapePool": "serviceMonitor/a", "scrapeUrl": "http://10.0.0.2:9100/metrics",
                 "health": "up", "labels": {"instance": "10.0.0.2:9100", "job": "node"},
                 "discoveredLabels": {"__address__": "10.0.0.2:9100"},
                 "lastError": "", "lastScrape": "2026-09-30T10:00:00Z",
                 "lastScrapeDuration": 0.012},
                {"scrapePool": "serviceMonitor/a", "scrapeUrl": "http://10.0.0.3:9100/metrics",
                 "health": "down", "labels": {}, "discoveredLabels": {},
                 "lastError": "connection refused"},
                {"scrapePool": "serviceMonitor/b", "scrapeUrl": "http://x/metrics",
                 "health": "weird", "labels": {}, "discoveredLabels": {}}
            ],
            "droppedTargets": [
                {"scrapePool": "serviceMonitor/a", "discoveredLabels": {"__address__": "z"}},
                {"scrapePool": "serviceMonitor/c", "discoveredLabels": {}}
            ]
        }})
    }

    #[test]
    fn targets_group_into_pools() {
        let targets = parse_targets(&targets_body()).unwrap();
        assert_eq!(targets.active.len(), 3);
        assert_eq!(
            targets.active[1].last_error.as_deref(),
            Some("connection refused")
        );
        assert_eq!(targets.active[0].instance(), "10.0.0.2:9100");
        assert_eq!(targets.active[1].instance(), "http://10.0.0.3:9100/metrics");
        assert_eq!(targets.count(Health::Unknown), 1);
        let pools = targets.pools();
        let names: Vec<_> = pools.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["serviceMonitor/a", "serviceMonitor/b", "serviceMonitor/c"]
        );
        assert_eq!((pools[0].up, pools[0].down, pools[0].dropped), (1, 1, 1));
        assert_eq!((pools[2].total(), pools[2].dropped), (0, 1));
    }

    #[test]
    fn server_side_dropped_counts_win() {
        let body = json!({"status": "success", "data": {
            "activeTargets": [], "droppedTargets": [],
            "droppedTargetCounts": {"p": 4200}
        }});
        let targets = parse_targets(&body).unwrap();
        assert_eq!(targets.pools()[0].dropped, 4200);
    }

    #[test]
    fn rules() {
        let body = json!({"status": "success", "data": {"groups": [{
        "name": "g", "file": "/etc/rules.yaml", "interval": 30, "evaluationTime": 0.002,
        "rules": [
            {"name": "HighErrors", "type": "alerting", "query": "rate(x[5m]) > 1",
             "duration": 300, "health": "ok", "state": "firing",
             "labels": {"severity": "critical"}, "annotations": {"summary": "s"},
             "alerts": [{"labels": {"a": "b"}, "state": "firing",
                         "activeAt": "2026-09-30T09:00:00Z", "value": "1e+00"}]},
            {"name": "job:x:rate5m", "type": "recording", "query": "rate(x[5m])",
             "health": "err", "lastError": "boom"}
        ]}]}});
        let groups = parse_rules(&body).unwrap();
        let rules = &groups[0].rules;
        assert_eq!(rules[0].kind, RuleKind::Alerting);
        assert!(rules[0].firing() && !rules[0].failing());
        assert_eq!(rules[0].alerts[0].value.as_deref(), Some("1e+00"));
        assert_eq!(rules[1].kind, RuleKind::Recording);
        assert!(rules[1].failing(), "a rule's `err` is a failing rule");
        assert_eq!(rules[1].last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn query_results() {
        let vector = parse_query(&json!({"status": "success", "data": {
            "resultType": "vector",
            "result": [{"metric": {"__name__": "up", "job": "a"}, "value": [1.0, "1"]}]
        }}))
        .unwrap();
        let QueryResult::Vector(rows) = vector else {
            panic!("vector")
        };
        assert_eq!(series_name(&rows[0].labels), r#"up{job="a"}"#);
        let matrix = parse_query(&json!({"status": "success", "data": {
            "resultType": "matrix",
            "result": [{"metric": {}, "values": [[1.0, "1"], [2.0, "NaN"], [3.0, "2"]]}]
        }}))
        .unwrap();
        let QueryResult::Matrix(series) = matrix else {
            panic!("matrix")
        };
        assert_eq!(series[0].points, [(1.0, 1.0), (3.0, 2.0)], "NaN is a gap");
        assert_eq!(series_name(&series[0].labels), "{}");
        assert_eq!(
            parse_query(&json!({"status": "success", "data": {
                "resultType": "scalar", "result": [1.0, "3"]}}))
            .unwrap(),
            QueryResult::Scalar("3".into())
        );
        assert!(parse_query(&json!({"status": "success", "data": {"resultType": "x"}})).is_err());
    }

    #[test]
    fn label_values_are_escaped() {
        let labels: Labels = [("a".to_string(), "x\"y".to_string())].into();
        assert_eq!(series_name(&labels), r#"{a="x\"y"}"#);
    }
}
