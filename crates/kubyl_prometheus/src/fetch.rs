//! Reads of the Prometheus HTTP API. Everything here is async and runs on the Tokio runtime
//! (`kubyl_core::spawn_kube`), never on the UI thread.

use kubyl_metrics::prometheus::{PromClient, PromError};

use crate::complete::Names;
use crate::model::{self, Overview, QueryResult, RuleGroup, Targets};

/// Build info, runtime info and TSDB status, read at once. Each can fail alone: Thanos Query
/// has no TSDB status, VictoriaMetrics no runtime info.
pub async fn overview(prom: &PromClient) -> Overview {
    let (build, runtime, tsdb) = futures::join!(
        prom.api("/api/v1/status/buildinfo", &[]),
        prom.api("/api/v1/status/runtimeinfo", &[]),
        prom.api("/api/v1/status/tsdb", &[]),
    );
    Overview {
        build: Some(build.and_then(|b| model::parse_build_info(&b))),
        runtime: Some(runtime.and_then(|b| model::parse_runtime_info(&b))),
        tsdb: Some(tsdb.and_then(|b| model::parse_tsdb(&b))),
    }
}

/// Active and dropped targets with their discovered labels.
pub async fn targets(prom: &PromClient) -> Result<Targets, PromError> {
    let body = prom
        .api("/api/v1/targets", &[("state", "any".to_string())])
        .await?;
    model::parse_targets(&body)
}

pub async fn rules(prom: &PromClient) -> Result<Vec<RuleGroup>, PromError> {
    let body = prom.api("/api/v1/rules", &[]).await?;
    model::parse_rules(&body)
}

/// How a query is evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Evaluation {
    /// At `time` (unix seconds), now when `None`.
    Instant {
        time: Option<f64>,
    },
    Range {
        start: f64,
        end: f64,
        step: f64,
    },
}

pub async fn query(
    prom: &PromClient,
    expression: &str,
    evaluation: Evaluation,
) -> Result<QueryResult, PromError> {
    let body = match evaluation {
        Evaluation::Instant { time } => {
            let mut params = vec![("query", expression.to_string())];
            if let Some(time) = time {
                params.push(("time", format!("{time:.3}")));
            }
            prom.api("/api/v1/query", &params).await?
        }
        Evaluation::Range { start, end, step } => {
            prom.api(
                "/api/v1/query_range",
                &[
                    ("query", expression.to_string()),
                    ("start", format!("{start:.3}")),
                    ("end", format!("{end:.3}")),
                    ("step", format!("{step}")),
                ],
            )
            .await?
        }
    };
    model::parse_query(&body)
}

/// How far back the suggestions look for series, like the Prometheus UI's default.
const LOOKBACK: f64 = 12.0 * 3600.0;

fn scoped(selector: Option<&str>) -> Vec<(&'static str, String)> {
    let now = kubyl_metrics::service::now();
    let mut params = vec![
        ("start", format!("{:.0}", now - LOOKBACK)),
        ("end", format!("{now:.0}")),
    ];
    if let Some(selector) = selector {
        params.push(("match[]", selector.to_string()));
    }
    params
}

/// Metrics asked for metadata up to, like the Prometheus UI.
const MAX_METADATA_METRICS: usize = 10_000;

/// `{"status": "success", "data": {"metric": [{"type": "counter", "help": "…"}]}}`.
pub fn parse_metadata(
    body: &serde_json::Value,
) -> std::collections::HashMap<String, (String, String)> {
    body["data"]
        .as_object()
        .map(|metrics| {
            metrics
                .iter()
                .filter_map(|(name, entries)| {
                    let first = entries.get(0)?;
                    Some((
                        name.clone(),
                        (
                            first["type"].as_str().unwrap_or_default().to_string(),
                            first["help"].as_str().unwrap_or_default().to_string(),
                        ),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The values of `label` among the series `selector` matches (all series without one).
pub async fn label_values(
    prom: &PromClient,
    label: &str,
    selector: Option<&str>,
) -> Result<Vec<String>, PromError> {
    // The label goes into the URL path: names only.
    if label.is_empty()
        || !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    {
        return Err(PromError::Invalid(format!("not a label name: {label:?}")));
    }
    let body = prom
        .api(&format!("/api/v1/label/{label}/values"), &scoped(selector))
        .await?;
    kubyl_metrics::prometheus::parse_names(&body)
}

/// The label names of the series `selector` matches.
pub async fn labels_of(prom: &PromClient, selector: &str) -> Result<Vec<String>, PromError> {
    let body = prom.api("/api/v1/labels", &scoped(Some(selector))).await?;
    kubyl_metrics::prometheus::parse_names(&body)
}

/// Metric and label names for the query box's suggestions (index lookups on the server). The
/// metric names are required, the label names are not (some servers have no labels API).
pub async fn names(prom: &PromClient) -> Result<Names, PromError> {
    let (metrics, labels) = futures::join!(prom.metric_names(), prom.api("/api/v1/labels", &[]));
    let metrics = metrics?;
    // Type and help, unless the server has so many metrics that this is megabytes.
    let metadata = if metrics.len() <= MAX_METADATA_METRICS {
        prom.api("/api/v1/metadata", &[])
            .await
            .map(|body| parse_metadata(&body))
            .unwrap_or_default()
    } else {
        Default::default()
    };
    Ok(Names {
        metrics,
        labels: labels
            .and_then(|body| kubyl_metrics::prometheus::parse_names(&body))
            .unwrap_or_default(),
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_takes_the_first_entry_per_metric() {
        let body = json!({"status": "success", "data": {
            "up": [{"type": "gauge", "help": "Target is up.", "unit": ""},
                   {"type": "counter", "help": "Another", "unit": ""}],
            "odd": [],
            "bare": [{"type": "counter"}]
        }});
        let metadata = parse_metadata(&body);
        assert_eq!(metadata["up"], ("gauge".into(), "Target is up.".into()));
        assert_eq!(metadata["bare"], ("counter".into(), String::new()));
        assert!(!metadata.contains_key("odd"));
    }
}
