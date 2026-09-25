//! Turning Event objects (and pod status) into the rows of the events stream.
//!
//! Pure functions, tested without a cluster: [`event_row`] reads `events.k8s.io/v1` and core
//! `v1` Events alike, [`oom_rows`] derives `OOMKilled` warnings from pod status (Kubernetes
//! itself only reports a `BackOff` after the restart), and [`group`] folds repeats into `×N`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::SharedString;
use jiff::Timestamp;
use kubyl_resources::columns::{event_message, event_time};
use kubyl_resources::format::{array_at, format_bytes, int_at, parse_quantity, str_at, timestamp};
use serde_json::Value;

/// One line of the stream (one event, or a group of repeats).
#[derive(Clone, Debug, PartialEq)]
pub struct EventRow {
    /// Identity for grouping and selection.
    pub key: SharedString,
    pub warning: bool,
    pub reason: SharedString,
    /// `Pod`, `Deployment`…
    pub kind: SharedString,
    pub name: SharedString,
    /// `apps/v1` etc. of the object, when the event says.
    pub api_version: SharedString,
    pub namespace: Option<SharedString>,
    pub message: SharedString,
    /// Occurrences (series count / count, summed over grouped events).
    pub count: u64,
    pub last: Option<Timestamp>,
    /// Who reported it (`kubelet`, `deployment-controller`…), or `pod status` for derived rows.
    pub source: SharedString,
    /// Derived from pod status, not an Event object.
    pub derived: bool,
}

impl EventRow {
    /// `pod/name`, as in the mockup.
    pub fn object(&self) -> String {
        format!("{}/{}", self.kind.to_lowercase(), self.name)
    }

    fn matches(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        [
            self.reason.as_ref(),
            self.message.as_ref(),
            self.name.as_ref(),
            self.kind.as_ref(),
            self.source.as_ref(),
            self.namespace.as_deref().unwrap_or_default(),
        ]
        .iter()
        .any(|field| field.to_lowercase().contains(&needle))
    }
}

/// Which rows to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    /// `Some(true)`: warnings only, `Some(false)`: normal only.
    pub warnings: Option<bool>,
    pub search: String,
}

impl Filter {
    pub fn matches(&self, row: &EventRow) -> bool {
        self.warnings.is_none_or(|w| row.warning == w)
            && (self.search.trim().is_empty() || row.matches(self.search.trim()))
    }
}

/// Reads one Event (either API).
pub fn event_row(event: &Value) -> EventRow {
    let object = event
        .get("regarding")
        .or_else(|| event.get("involvedObject"))
        .unwrap_or(&Value::Null);
    let count = event
        .pointer("/series/count")
        .or_else(|| event.pointer("/count"))
        .or_else(|| event.pointer("/deprecatedCount"))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1);
    let source = [
        "/reportingController",
        "/source/component",
        "/deprecatedSource/component",
    ]
    .iter()
    .map(|p| str_at(event, p))
    .find(|s| !s.is_empty())
    .unwrap_or_default();
    let namespace = match str_at(object, "/namespace") {
        "" => match str_at(event, "/metadata/namespace") {
            "" => None,
            ns => Some(ns.to_string().into()),
        },
        ns => Some(ns.to_string().into()),
    };
    EventRow {
        key: str_at(event, "/metadata/uid").to_string().into(),
        warning: str_at(event, "/type") == "Warning",
        reason: str_at(event, "/reason").to_string().into(),
        kind: str_at(object, "/kind").to_string().into(),
        name: str_at(object, "/name").to_string().into(),
        api_version: str_at(object, "/apiVersion").to_string().into(),
        namespace,
        message: event_message(event).trim().replace('\n', " ").into(),
        count,
        last: event_time(event),
        source: source.to_string().into(),
        derived: false,
    }
}

/// `OOMKilled` warnings from pod status: one row per container whose current or last
/// termination was an OOM kill, counted by its restarts.
pub fn oom_rows(pod: &Value) -> Vec<EventRow> {
    let namespace = str_at(pod, "/metadata/namespace");
    let name = str_at(pod, "/metadata/name");
    let containers = array_at(pod, "/spec/containers");
    ["/status/containerStatuses", "/status/initContainerStatuses"]
        .iter()
        .flat_map(|path| array_at(pod, path))
        .filter_map(|status| {
            let terminated = ["/state/terminated", "/lastState/terminated"]
                .iter()
                .filter_map(|p| status.pointer(p))
                .find(|t| str_at(t, "/reason") == "OOMKilled")?;
            let container = str_at(status, "/name");
            let limit = containers
                .iter()
                .find(|c| str_at(c, "/name") == container)
                .and_then(|c| parse_quantity(str_at(c, "/resources/limits/memory")));
            let restarts = int_at(status, "/restartCount").max(0) as u64;
            let message = match limit {
                Some(limit) => format!(
                    "Container {container} was OOM-killed (memory limit {}).",
                    format_bytes(limit)
                ),
                None => format!("Container {container} was OOM-killed."),
            };
            Some(EventRow {
                key: format!("oom:{namespace}/{name}/{container}").into(),
                warning: true,
                reason: "OOMKilled".into(),
                kind: "Pod".into(),
                name: name.to_string().into(),
                api_version: "v1".into(),
                namespace: Some(namespace.to_string().into()),
                message: message.into(),
                count: restarts.max(1),
                last: timestamp(str_at(terminated, "/finishedAt")),
                source: "pod status".into(),
                derived: true,
            })
        })
        .collect()
}

/// Namespace, kind, name, reason and type of a group.
type GroupKey = (
    Option<SharedString>,
    SharedString,
    SharedString,
    SharedString,
    bool,
);

/// Folds events about the same object with the same reason and type into one row (the newest
/// message, summed counts). Rows come back newest first.
pub fn group(rows: Vec<EventRow>, fold: bool) -> Vec<EventRow> {
    let mut rows = if fold {
        let mut groups: HashMap<GroupKey, EventRow> = HashMap::new();
        for row in rows {
            let key = (
                row.namespace.clone(),
                row.kind.clone(),
                row.name.clone(),
                row.reason.clone(),
                row.warning,
            );
            match groups.get_mut(&key) {
                Some(group) => {
                    group.count += row.count;
                    if row.last > group.last {
                        group.last = row.last;
                        group.message = row.message;
                        group.source = row.source;
                    }
                }
                None => {
                    let mut row = row;
                    row.key = format!(
                        "{}/{}/{}/{}/{}",
                        key.0.as_deref().unwrap_or_default(),
                        key.1,
                        key.2,
                        key.3,
                        key.4
                    )
                    .into();
                    groups.insert(key, row);
                }
            }
        }
        groups.into_values().collect()
    } else {
        rows
    };
    rows.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.key.cmp(&b.key)));
    rows
}

/// Builds the stream from Event objects and (optionally) pods.
pub fn rows<'a>(
    events: impl Iterator<Item = &'a Arc<Value>>,
    pods: impl Iterator<Item = &'a Arc<Value>>,
    fold: bool,
) -> Vec<EventRow> {
    let mut rows: Vec<EventRow> = events.map(|e| event_row(e)).collect();
    rows.extend(pods.flat_map(|p| oom_rows(p)));
    group(rows, fold)
}

/// Warning and normal row counts, as the mockup's chips show (folded rows count once).
pub fn counts(rows: &[EventRow]) -> (usize, usize) {
    let warnings = rows.iter().filter(|r| r.warning).count();
    (warnings, rows.len() - warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn core_event(uid: &str, reason: &str, pod: &str, count: u64, last: &str) -> Value {
        json!({
            "metadata": {"uid": uid, "namespace": "payments", "name": format!("{pod}.{uid}")},
            "type": "Warning",
            "reason": reason,
            "message": format!("{reason} happened\nagain"),
            "involvedObject": {"kind": "Pod", "name": pod, "namespace": "payments", "apiVersion": "v1"},
            "count": count,
            "lastTimestamp": last,
            "source": {"component": "kubelet"}
        })
    }

    #[test]
    fn reads_both_event_apis() {
        let row = event_row(&core_event(
            "1",
            "BackOff",
            "web-0",
            14,
            "2026-09-25T10:00:00Z",
        ));
        assert!(row.warning);
        assert_eq!(row.object(), "pod/web-0");
        assert_eq!(row.count, 14);
        assert_eq!(row.message.as_ref(), "BackOff happened again");
        assert_eq!(row.source.as_ref(), "kubelet");

        let modern = json!({
            "metadata": {"uid": "2", "namespace": "payments"},
            "type": "Normal",
            "reason": "ScalingReplicaSet",
            "note": "Scaled up replica set web-7d9 to 2",
            "regarding": {"kind": "Deployment", "name": "web", "namespace": "payments", "apiVersion": "apps/v1"},
            "series": {"count": 3, "lastObservedTime": "2026-09-25T10:01:00.000000Z"},
            "reportingController": "deployment-controller"
        });
        let row = event_row(&modern);
        assert!(!row.warning);
        assert_eq!(row.object(), "deployment/web");
        assert_eq!(row.api_version.as_ref(), "apps/v1");
        assert_eq!(row.count, 3);
        assert_eq!(row.message.as_ref(), "Scaled up replica set web-7d9 to 2");
        assert_eq!(row.source.as_ref(), "deployment-controller");
        assert!(row.last.is_some());
    }

    #[test]
    fn groups_repeats_newest_first() {
        let rows = vec![
            event_row(&core_event(
                "1",
                "BackOff",
                "web-0",
                2,
                "2026-09-25T10:00:00Z",
            )),
            event_row(&core_event(
                "2",
                "BackOff",
                "web-0",
                3,
                "2026-09-25T10:05:00Z",
            )),
            event_row(&core_event(
                "3",
                "Unhealthy",
                "web-0",
                1,
                "2026-09-25T10:02:00Z",
            )),
        ];
        let grouped = group(rows.clone(), true);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].reason.as_ref(), "BackOff");
        assert_eq!(grouped[0].count, 5);
        assert_eq!(grouped[1].reason.as_ref(), "Unhealthy");
        let flat = group(rows, false);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].key.as_ref(), "2");
        assert_eq!(counts(&flat), (3, 0));
    }

    #[test]
    fn derives_oom_kills_from_pod_status() {
        let pod = json!({
            "metadata": {"name": "memory-hog-1", "namespace": "payments"},
            "spec": {"containers": [{"name": "hog", "resources": {"limits": {"memory": "48Mi"}}}]},
            "status": {"containerStatuses": [{
                "name": "hog",
                "restartCount": 4,
                "state": {"waiting": {"reason": "CrashLoopBackOff"}},
                "lastState": {"terminated": {"reason": "OOMKilled", "exitCode": 137, "finishedAt": "2026-09-25T10:00:00Z"}}
            }]}
        });
        let rows = oom_rows(&pod);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reason.as_ref(), "OOMKilled");
        assert_eq!(rows[0].count, 4);
        assert!(rows[0].derived && rows[0].warning);
        assert_eq!(
            rows[0].message.as_ref(),
            "Container hog was OOM-killed (memory limit 48Mi)."
        );
        let healthy = json!({"metadata": {"name": "ok"}, "status": {"containerStatuses": [
            {"name": "a", "lastState": {"terminated": {"reason": "Completed"}}}
        ]}});
        assert!(oom_rows(&healthy).is_empty());
    }

    #[test]
    fn filters() {
        let row = event_row(&core_event(
            "1",
            "BackOff",
            "web-0",
            1,
            "2026-09-25T10:00:00Z",
        ));
        assert!(Filter::default().matches(&row));
        assert!(
            Filter {
                warnings: Some(true),
                search: String::new()
            }
            .matches(&row)
        );
        assert!(
            !Filter {
                warnings: Some(false),
                search: String::new()
            }
            .matches(&row)
        );
        assert!(
            Filter {
                warnings: None,
                search: "WEB-0".into()
            }
            .matches(&row)
        );
        assert!(
            !Filter {
                warnings: None,
                search: "ledger".into()
            }
            .matches(&row)
        );
    }
}
