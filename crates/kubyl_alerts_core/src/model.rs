//! Alerts, silences and rules, and the parsers of Alertmanager's API v2 and Prometheus' rules
//! API. Parsers accept unknown fields and states. Nothing here keeps Alertmanager's
//! configuration (`/api/v2/status` `config.original` can hold receiver secrets).

use std::collections::BTreeMap;
use std::fmt;

use jiff::Timestamp;
use serde_json::Value;

use crate::matchers::{MatchOp, Matcher};

/// How bad an alert is. Unknown values sort after info.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Severity {
    Critical,
    Warning,
    Info,
    Other(String),
    None,
}

impl Severity {
    /// Maps a label value: `critical`, `warning`, `info`, `none` (or empty), else the
    /// `alerts.severities` map (case-insensitive), else [`Severity::Other`].
    pub fn from_value(value: &str, custom: &BTreeMap<String, String>) -> Self {
        let lower = value.trim().to_lowercase();
        let known = |v: &str| match v {
            "critical" => Some(Severity::Critical),
            "warning" => Some(Severity::Warning),
            "info" => Some(Severity::Info),
            "none" | "" => Some(Severity::None),
            _ => None,
        };
        if let Some(severity) = known(&lower) {
            return severity;
        }
        if let Some(mapped) = custom
            .iter()
            .find(|(k, _)| k.to_lowercase() == lower)
            .and_then(|(_, v)| known(&v.to_lowercase()))
        {
            return mapped;
        }
        Severity::Other(value.trim().to_string())
    }

    /// Sort key: critical first.
    pub fn rank(&self) -> u8 {
        match self {
            Severity::Critical => 0,
            Severity::Warning => 1,
            Severity::Info => 2,
            Severity::Other(_) => 3,
            Severity::None => 4,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Severity::Critical => "critical",
            Severity::Warning => "warning",
            Severity::Info => "info",
            Severity::Other(value) => value,
            Severity::None => "none",
        }
    }

    /// At least as bad as `other`.
    pub fn at_least(&self, other: &Severity) -> bool {
        self.rank() <= other.rank()
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Where an alert stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AlertState {
    Firing,
    /// Its rule's condition holds, but not for `for` yet (Prometheus only).
    Pending,
    Silenced,
    Inhibited,
    /// Alertmanager got it but hasn't routed it yet.
    Unprocessed,
    /// It stopped firing (kept for a while in memory).
    Resolved,
}

impl AlertState {
    pub fn label(self) -> &'static str {
        match self {
            AlertState::Firing => "firing",
            AlertState::Pending => "pending",
            AlertState::Silenced => "silenced",
            AlertState::Inhibited => "inhibited",
            AlertState::Unprocessed => "unprocessed",
            AlertState::Resolved => "resolved",
        }
    }

    /// Firing, but silenced or inhibited.
    pub fn suppressed(self) -> bool {
        matches!(self, AlertState::Silenced | AlertState::Inhibited)
    }
}

/// A kind of object an alert is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetKind {
    Pod,
    Deployment,
    StatefulSet,
    DaemonSet,
    ReplicaSet,
    Job,
    CronJob,
    HorizontalPodAutoscaler,
    PersistentVolumeClaim,
    Node,
    ClusterOperator,
    Service,
    Namespace,
}

impl TargetKind {
    /// `(group, version, plural)`.
    pub fn gvr(self) -> (&'static str, &'static str, &'static str) {
        match self {
            TargetKind::Pod => ("", "v1", "pods"),
            TargetKind::Deployment => ("apps", "v1", "deployments"),
            TargetKind::StatefulSet => ("apps", "v1", "statefulsets"),
            TargetKind::DaemonSet => ("apps", "v1", "daemonsets"),
            TargetKind::ReplicaSet => ("apps", "v1", "replicasets"),
            TargetKind::Job => ("batch", "v1", "jobs"),
            TargetKind::CronJob => ("batch", "v1", "cronjobs"),
            TargetKind::HorizontalPodAutoscaler => {
                ("autoscaling", "v2", "horizontalpodautoscalers")
            }
            TargetKind::PersistentVolumeClaim => ("", "v1", "persistentvolumeclaims"),
            TargetKind::Node => ("", "v1", "nodes"),
            TargetKind::ClusterOperator => ("config.openshift.io", "v1", "clusteroperators"),
            TargetKind::Service => ("", "v1", "services"),
            TargetKind::Namespace => ("", "v1", "namespaces"),
        }
    }

    /// `pod`, `deployment`…
    pub fn short(self) -> &'static str {
        match self {
            TargetKind::Pod => "pod",
            TargetKind::Deployment => "deployment",
            TargetKind::StatefulSet => "statefulset",
            TargetKind::DaemonSet => "daemonset",
            TargetKind::ReplicaSet => "replicaset",
            TargetKind::Job => "job",
            TargetKind::CronJob => "cronjob",
            TargetKind::HorizontalPodAutoscaler => "hpa",
            TargetKind::PersistentVolumeClaim => "pvc",
            TargetKind::Node => "node",
            TargetKind::ClusterOperator => "clusteroperator",
            TargetKind::Service => "service",
            TargetKind::Namespace => "namespace",
        }
    }

    pub fn namespaced(self) -> bool {
        !matches!(
            self,
            TargetKind::Node | TargetKind::ClusterOperator | TargetKind::Namespace
        )
    }
}

/// The object an alert is about, from its labels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub kind: TargetKind,
    pub namespace: Option<String>,
    pub name: String,
    pub container: Option<String>,
}

impl Target {
    /// `pod/checkout-7d9f-x2kqp`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.kind.short(), self.name)
    }
}

/// The object an alert is about, most specific label first: `exported_namespace`/`exported_pod`
/// (metrics relabelled by an exporter), `pod` (with `container`), the workload labels,
/// `job_name` (not `job`, the scrape job), `cronjob`, `horizontalpodautoscaler`,
/// `persistentvolumeclaim`, `node`, `instance` when it names a node (`node_of` maps a node name
/// or `<InternalIP>:<port>`), the ClusterOperator of `ClusterOperator*` alerts, `service` only
/// for alerts not derived from kube-state-metrics, and finally `namespace`.
pub fn target_of(
    alert_name: &str,
    labels: &BTreeMap<String, String>,
    node_of: &dyn Fn(&str) -> Option<String>,
) -> Option<Target> {
    let get = |key: &str| labels.get(key).filter(|v| !v.is_empty()).cloned();
    let namespace = get("exported_namespace").or_else(|| get("namespace"));
    let namespaced = |kind: TargetKind, name: String| Target {
        kind,
        namespace: namespace.clone(),
        name,
        container: None,
    };
    if let Some(pod) = get("exported_pod") {
        return Some(Target {
            container: get("exported_container").or_else(|| get("container")),
            ..namespaced(TargetKind::Pod, pod)
        });
    }
    if let Some(pod) = get("pod") {
        return Some(Target {
            container: get("container"),
            ..namespaced(TargetKind::Pod, pod)
        });
    }
    for (label, kind) in [
        ("deployment", TargetKind::Deployment),
        ("statefulset", TargetKind::StatefulSet),
        ("daemonset", TargetKind::DaemonSet),
        ("replicaset", TargetKind::ReplicaSet),
        ("job_name", TargetKind::Job),
        ("cronjob", TargetKind::CronJob),
        (
            "horizontalpodautoscaler",
            TargetKind::HorizontalPodAutoscaler,
        ),
        ("persistentvolumeclaim", TargetKind::PersistentVolumeClaim),
    ] {
        if let Some(name) = get(label) {
            return Some(namespaced(kind, name));
        }
    }
    let node = |name: String| Target {
        kind: TargetKind::Node,
        namespace: None,
        name,
        container: None,
    };
    if let Some(name) = get("node") {
        return Some(node(name));
    }
    if let Some(name) = get("instance").and_then(|i| node_of(&i)) {
        return Some(node(name));
    }
    if alert_name.starts_with("ClusterOperator")
        && let Some(name) = get("name")
    {
        return Some(Target {
            kind: TargetKind::ClusterOperator,
            namespace: None,
            name,
            container: None,
        });
    }
    if let Some(service) = get("service")
        && get("job").as_deref() != Some("kube-state-metrics")
        && namespace.is_some()
    {
        return Some(namespaced(TargetKind::Service, service));
    }
    namespace.map(|ns| Target {
        kind: TargetKind::Namespace,
        namespace: None,
        name: ns,
        container: None,
    })
}

/// One alert, from Alertmanager, Prometheus or both.
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    /// Alertmanager's fingerprint, or a hash of the labels for Prometheus-only alerts.
    pub fingerprint: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub severity: Severity,
    pub state: AlertState,
    pub silenced_by: Vec<String>,
    pub inhibited_by: Vec<String>,
    /// Firing since (Alertmanager's `startsAt`).
    pub starts_at: Option<Timestamp>,
    /// `starts_at` was computed from Prometheus' `activeAt` plus the rule's `for`.
    pub starts_approx: bool,
    /// Pending since (Prometheus' `activeAt`).
    pub active_at: Option<Timestamp>,
    /// Last received by Alertmanager.
    pub updated_at: Option<Timestamp>,
    /// When it resolved (in-memory history), or Alertmanager's `endsAt`.
    pub ends_at: Option<Timestamp>,
    pub receivers: Vec<String>,
    pub generator_url: Option<String>,
    /// The expression's value (Prometheus).
    pub value: Option<String>,
    /// `monitoring/alertmanager-operated` or `Prometheus`.
    pub source: String,
    pub target: Option<Target>,
}

impl Alert {
    pub fn namespace(&self) -> Option<&str> {
        self.labels
            .get("namespace")
            .or_else(|| self.labels.get("exported_namespace"))
            .map(String::as_str)
            .filter(|n| !n.is_empty())
    }

    /// `summary`, else `message`, else `description`.
    pub fn summary(&self) -> &str {
        ["summary", "message", "description"]
            .iter()
            .find_map(|k| self.annotations.get(*k).filter(|v| !v.is_empty()))
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub fn description(&self) -> Option<&str> {
        self.annotations
            .get("description")
            .or_else(|| self.annotations.get("message"))
            .map(String::as_str)
            .filter(|d| !d.is_empty() && *d != self.summary())
    }

    pub fn runbook_url(&self) -> Option<&str> {
        self.annotations
            .get("runbook_url")
            .map(String::as_str)
            .filter(|u| is_web_url(u))
    }

    /// Matchers selecting exactly this alert (its labels).
    pub fn matchers(&self) -> Vec<Matcher> {
        self.labels
            .iter()
            .map(|(name, value)| Matcher::new(name, MatchOp::Equal, value))
            .collect()
    }

    /// Since when it's in its state: firing since, else pending since.
    pub fn since(&self) -> Option<Timestamp> {
        match self.state {
            AlertState::Pending => self.active_at,
            _ => self.starts_at.or(self.active_at),
        }
    }
}

/// `http`/`https` only: never `javascript:` or `file:` from an annotation.
pub fn is_web_url(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

fn timestamp(value: &Value) -> Option<Timestamp> {
    let text = value.as_str()?;
    // `0001-01-01T00:00:00Z` means "not set" in Alertmanager.
    let ts: Timestamp = text.parse().ok()?;
    (ts.as_second() > 0).then_some(ts)
}

fn string_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// How to read labels while parsing.
pub struct ParseOptions<'a> {
    pub severity_label: &'a str,
    pub severities: &'a BTreeMap<String, String>,
    pub node_of: &'a dyn Fn(&str) -> Option<String>,
}

impl ParseOptions<'_> {
    fn severity(&self, labels: &BTreeMap<String, String>) -> Severity {
        Severity::from_value(
            labels
                .get(self.severity_label)
                .map(String::as_str)
                .unwrap_or_default(),
            self.severities,
        )
    }
}

/// `GET /api/v2/alerts` (active, silenced, inhibited and unprocessed alerts).
pub fn parse_am_alerts(body: &Value, source: &str, options: &ParseOptions) -> Vec<Alert> {
    let Some(items) = body.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let labels = string_map(&item["labels"]);
            let name = labels.get("alertname")?.clone();
            let status = &item["status"];
            let silenced_by = strings(&status["silencedBy"]);
            let inhibited_by = strings(&status["inhibitedBy"]);
            let state = match status["state"].as_str().unwrap_or_default() {
                "active" => AlertState::Firing,
                "unprocessed" => AlertState::Unprocessed,
                // "suppressed", or an unknown state with suppressors.
                _ if !silenced_by.is_empty() => AlertState::Silenced,
                _ if !inhibited_by.is_empty() => AlertState::Inhibited,
                "suppressed" => AlertState::Silenced,
                _ => AlertState::Firing,
            };
            let receivers = item["receivers"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .filter_map(|r| r["name"].as_str().or(r.as_str()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(Alert {
                fingerprint: item["fingerprint"].as_str().unwrap_or_default().to_string(),
                target: target_of(&name, &labels, options.node_of),
                severity: options.severity(&labels),
                annotations: string_map(&item["annotations"]),
                state,
                silenced_by,
                inhibited_by,
                starts_at: timestamp(&item["startsAt"]),
                starts_approx: false,
                active_at: None,
                updated_at: timestamp(&item["updatedAt"]),
                ends_at: timestamp(&item["endsAt"]),
                receivers,
                generator_url: item["generatorURL"]
                    .as_str()
                    .filter(|u| is_web_url(u))
                    .map(str::to_string),
                value: None,
                source: source.to_string(),
                name,
                labels,
            })
        })
        .collect()
}

/// An alert from Prometheus' `/api/v1/alerts`.
#[derive(Clone, Debug, PartialEq)]
pub struct PromAlert {
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    /// `firing`, `pending` (others are kept as they come).
    pub state: String,
    pub active_at: Option<Timestamp>,
    pub value: Option<String>,
}

/// `GET /api/v1/alerts` (Prometheus 2.x/3.x and Thanos).
pub fn parse_prom_alerts(body: &Value) -> Result<Vec<PromAlert>, String> {
    check_status(body)?;
    let items = body["data"]["alerts"]
        .as_array()
        .ok_or("expected data.alerts")?;
    Ok(items
        .iter()
        .map(|item| PromAlert {
            labels: string_map(&item["labels"]),
            annotations: string_map(&item["annotations"]),
            state: item["state"].as_str().unwrap_or_default().to_string(),
            active_at: timestamp(&item["activeAt"]),
            value: item["value"].as_str().map(str::to_string),
        })
        .collect())
}

fn check_status(body: &Value) -> Result<(), String> {
    match body["status"].as_str() {
        Some("success") => Ok(()),
        Some("error") => Err(body["error"]
            .as_str()
            .unwrap_or("unknown error")
            .to_string()),
        _ => Err("not a Prometheus API answer".into()),
    }
}

/// An alerting rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub name: String,
    pub group: String,
    pub file: String,
    pub query: String,
    /// `for`, in seconds.
    pub duration: f64,
    pub keep_firing_for: f64,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    /// `ok`, `err`, `unknown`.
    pub health: String,
    pub last_error: Option<String>,
    pub last_evaluation: Option<Timestamp>,
    /// Seconds.
    pub evaluation_time: f64,
    /// `inactive`, `pending`, `firing`.
    pub state: String,
}

impl Rule {
    pub fn failing(&self) -> bool {
        self.health == "err" || self.last_error.is_some()
    }

    /// Firing, pending or failing.
    pub fn problem(&self) -> bool {
        self.failing() || matches!(self.state.as_str(), "firing" | "pending")
    }
}

/// A rule group with its alerting rules.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleGroup {
    pub name: String,
    pub file: String,
    pub rules: Vec<Rule>,
}

/// `GET /api/v1/rules?type=alert` (older servers ignore the filter: recording rules are
/// skipped here).
pub fn parse_rules(body: &Value) -> Result<Vec<RuleGroup>, String> {
    check_status(body)?;
    let groups = body["data"]["groups"]
        .as_array()
        .ok_or("expected data.groups")?;
    Ok(groups
        .iter()
        .map(|group| {
            let name = group["name"].as_str().unwrap_or_default().to_string();
            let file = group["file"].as_str().unwrap_or_default().to_string();
            let rules = group["rules"]
                .as_array()
                .map(|rules| {
                    rules
                        .iter()
                        .filter(|r| match r["type"].as_str() {
                            Some(kind) => kind == "alerting",
                            // Servers without `type`: alerting rules have a `for`/alerts.
                            None => r.get("duration").is_some() || r.get("alerts").is_some(),
                        })
                        .map(|r| Rule {
                            name: r["name"].as_str().unwrap_or_default().to_string(),
                            group: name.clone(),
                            file: file.clone(),
                            query: r["query"].as_str().unwrap_or_default().to_string(),
                            duration: r["duration"].as_f64().unwrap_or_default(),
                            keep_firing_for: r["keepFiringFor"].as_f64().unwrap_or_default(),
                            labels: string_map(&r["labels"]),
                            annotations: string_map(&r["annotations"]),
                            health: r["health"].as_str().unwrap_or("unknown").to_string(),
                            last_error: r["lastError"]
                                .as_str()
                                .filter(|e| !e.is_empty())
                                .map(str::to_string),
                            last_evaluation: timestamp(&r["lastEvaluation"]),
                            evaluation_time: r["evaluationTime"].as_f64().unwrap_or_default(),
                            state: r["state"].as_str().unwrap_or("inactive").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            RuleGroup { name, file, rules }
        })
        .filter(|g: &RuleGroup| !g.rules.is_empty())
        .collect())
}

/// `GET /api/v1/alertmanagers`: the URLs Prometheus sends alerts to (`activeAlertmanagers`).
pub fn parse_alertmanagers(body: &Value) -> Result<Vec<String>, String> {
    check_status(body)?;
    Ok(body["data"]["activeAlertmanagers"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i["url"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// A silence.
#[derive(Clone, Debug, PartialEq)]
pub struct Silence {
    pub id: String,
    pub matchers: Vec<Matcher>,
    pub starts_at: Option<Timestamp>,
    pub ends_at: Option<Timestamp>,
    pub created_by: String,
    pub comment: String,
    /// `active`, `pending`, `expired`.
    pub state: String,
    /// The Alertmanager it's on.
    pub source: String,
}

impl Silence {
    /// Matches a set of labels (the Alertmanager semantics of every matcher).
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        self.matchers.iter().all(|m| m.matches(labels))
    }
}

/// `GET /api/v2/silences`.
pub fn parse_silences(body: &Value, source: &str) -> Vec<Silence> {
    let Some(items) = body.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| Silence {
            id: item["id"].as_str().unwrap_or_default().to_string(),
            matchers: item["matchers"]
                .as_array()
                .map(|ms| ms.iter().filter_map(Matcher::from_json).collect())
                .unwrap_or_default(),
            starts_at: timestamp(&item["startsAt"]),
            ends_at: timestamp(&item["endsAt"]),
            created_by: item["createdBy"].as_str().unwrap_or_default().to_string(),
            comment: item["comment"].as_str().unwrap_or_default().to_string(),
            state: item["status"]["state"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            source: source.to_string(),
        })
        .collect()
}

/// What Kubyl keeps of `/api/v2/status`: version, uptime, cluster status and peers. The
/// configuration (`config.original`, with receiver URLs and secrets) is dropped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmStatus {
    pub version: Option<String>,
    pub uptime: Option<Timestamp>,
    /// `ready`, `settling`, `disabled`.
    pub cluster_status: Option<String>,
    /// The gossip cluster's name: the same Alertmanager behind two Services.
    pub cluster_name: Option<String>,
    pub peers: usize,
}

pub fn parse_status(body: &Value) -> Result<AmStatus, String> {
    if !body.is_object() || body.get("versionInfo").is_none() && body.get("cluster").is_none() {
        return Err("not an Alertmanager API v2 status".into());
    }
    Ok(AmStatus {
        version: body["versionInfo"]["version"].as_str().map(str::to_string),
        uptime: timestamp(&body["uptime"]),
        cluster_status: body["cluster"]["status"].as_str().map(str::to_string),
        cluster_name: body["cluster"]["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_string),
        peers: body["cluster"]["peers"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
    })
}

/// `GET /api/v2/receivers`.
pub fn parse_receivers(body: &Value) -> Vec<String> {
    body.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|r| r["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn options() -> ParseOptions<'static> {
        static SEVERITIES: std::sync::OnceLock<BTreeMap<String, String>> =
            std::sync::OnceLock::new();
        ParseOptions {
            severity_label: "severity",
            severities: SEVERITIES.get_or_init(|| {
                BTreeMap::from([
                    ("page".into(), "critical".into()),
                    ("P2".into(), "warning".into()),
                ])
            }),
            node_of: &|instance: &str| match instance {
                "kind-worker" | "172.18.0.3:9100" => Some("kind-worker".into()),
                _ => None,
            },
        }
    }

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn severities_map_custom_values() {
        let custom = BTreeMap::from([("page".to_string(), "critical".to_string())]);
        assert_eq!(
            Severity::from_value("CRITICAL", &custom),
            Severity::Critical
        );
        assert_eq!(Severity::from_value("Page", &custom), Severity::Critical);
        assert_eq!(Severity::from_value("", &custom), Severity::None);
        let high = Severity::from_value("high", &custom);
        assert_eq!(high, Severity::Other("high".into()));
        let mut all = [
            Severity::None,
            high,
            Severity::Info,
            Severity::Critical,
            Severity::Warning,
        ];
        all.sort_by_key(Severity::rank);
        assert_eq!(all[0], Severity::Critical);
        assert_eq!(all[3], Severity::Other("high".into()));
        assert!(Severity::Critical.at_least(&Severity::Warning));
        assert!(!Severity::Info.at_least(&Severity::Warning));
    }

    #[test]
    fn targets_from_labels_most_specific_first() {
        let node_of = options().node_of;
        let t = |name: &str, pairs: &[(&str, &str)]| target_of(name, &labels(pairs), node_of);
        // kube-state-metrics relabels its own labels as exported_*.
        let pod = t(
            "KubePodCrashLooping",
            &[
                ("namespace", "monitoring"),
                ("exported_namespace", "payments"),
                ("exported_pod", "a-1"),
                ("pod", "ksm-0"),
            ],
        )
        .unwrap();
        assert_eq!(
            (pod.kind, pod.namespace.as_deref(), pod.name.as_str()),
            (TargetKind::Pod, Some("payments"), "a-1")
        );
        let pod = t(
            "KubeContainerWaiting",
            &[("namespace", "p"), ("pod", "b"), ("container", "c")],
        )
        .unwrap();
        assert_eq!(pod.container.as_deref(), Some("c"));
        assert_eq!(
            t(
                "KubeDeploymentReplicasMismatch",
                &[("namespace", "p"), ("deployment", "d")]
            )
            .unwrap()
            .kind,
            TargetKind::Deployment
        );
        // `job` is the scrape job, `job_name` the Job.
        assert_eq!(
            t(
                "KubeJobFailed",
                &[
                    ("namespace", "p"),
                    ("job_name", "j"),
                    ("job", "kube-state-metrics")
                ]
            )
            .unwrap()
            .kind,
            TargetKind::Job
        );
        assert_eq!(
            t(
                "KubePersistentVolumeFillingUp",
                &[("namespace", "p"), ("persistentvolumeclaim", "data")]
            )
            .unwrap()
            .kind,
            TargetKind::PersistentVolumeClaim
        );
        assert_eq!(
            t(
                "NodeFilesystemSpaceFillingUp",
                &[("instance", "172.18.0.3:9100")]
            )
            .unwrap()
            .name,
            "kind-worker"
        );
        assert_eq!(
            t("KubeNodeNotReady", &[("node", "n1")]).unwrap().kind,
            TargetKind::Node
        );
        assert_eq!(
            t(
                "ClusterOperatorDegraded",
                &[
                    ("name", "ingress"),
                    ("namespace", "openshift-cluster-version")
                ]
            )
            .unwrap()
            .kind,
            TargetKind::ClusterOperator
        );
        // `service` of kube-state-metrics alerts is the exporter's own Service.
        assert_eq!(
            t(
                "KubeHpaMaxedOut",
                &[
                    ("namespace", "p"),
                    ("service", "ksm"),
                    ("job", "kube-state-metrics")
                ]
            )
            .unwrap()
            .kind,
            TargetKind::Namespace
        );
        assert_eq!(
            t(
                "TargetDown",
                &[("namespace", "p"), ("service", "web"), ("job", "web")]
            )
            .unwrap()
            .kind,
            TargetKind::Service
        );
        assert_eq!(
            t("KubeQuotaAlmostFull", &[("namespace", "p")])
                .unwrap()
                .label(),
            "namespace/p"
        );
        assert_eq!(t("Watchdog", &[]), None);
        assert_eq!(t("NodeClockSkew", &[("instance", "10.0.0.9:9100")]), None);
    }

    #[test]
    fn parses_alertmanager_v2() {
        let body = json!([
            {"fingerprint": "f1", "labels": {"alertname": "KubePodCrashLooping", "severity": "critical", "namespace": "payments", "pod": "gw-1"},
             "annotations": {"summary": "Pod is crash looping.", "runbook_url": "https://runbooks.example.com/x", "description": "d"},
             "startsAt": "2026-09-26T08:00:00.000Z", "endsAt": "2026-09-26T09:00:00.000Z", "updatedAt": "2026-09-26T08:30:00.000Z",
             "receivers": [{"name": "pagerduty"}], "generatorURL": "http://prometheus:9090/graph?g0.expr=up",
             "status": {"state": "active", "silencedBy": [], "inhibitedBy": [], "mutedBy": []}, "newField": 1},
            {"fingerprint": "f2", "labels": {"alertname": "TargetDown", "severity": "page"},
             "annotations": {"runbook_url": "javascript:alert(1)"}, "startsAt": "2026-09-26T08:00:00Z",
             "receivers": [{"name": "slack"}], "status": {"state": "suppressed", "silencedBy": ["s1"], "inhibitedBy": []}},
            {"fingerprint": "f3", "labels": {"alertname": "X"}, "status": {"state": "suppressed", "silencedBy": [], "inhibitedBy": ["f1"]}},
            {"fingerprint": "f4", "labels": {"alertname": "Y"}, "status": {"state": "someday"}},
            {"fingerprint": "f5", "labels": {"no": "name"}}
        ]);
        let alerts = parse_am_alerts(&body, "monitoring/am", &options());
        assert_eq!(alerts.len(), 4);
        let a = &alerts[0];
        assert_eq!(a.state, AlertState::Firing);
        assert_eq!(a.severity, Severity::Critical);
        assert_eq!(a.receivers, ["pagerduty"]);
        assert_eq!(a.runbook_url(), Some("https://runbooks.example.com/x"));
        assert_eq!(a.summary(), "Pod is crash looping.");
        assert_eq!(a.target.as_ref().unwrap().label(), "pod/gw-1");
        assert_eq!(a.since().unwrap().to_string(), "2026-09-26T08:00:00Z");
        let silenced = &alerts[1];
        assert_eq!(silenced.state, AlertState::Silenced);
        assert_eq!(silenced.severity, Severity::Critical, "custom severity");
        assert_eq!(silenced.runbook_url(), None, "only http(s) links");
        assert_eq!(alerts[2].state, AlertState::Inhibited);
        assert_eq!(
            alerts[3].state,
            AlertState::Firing,
            "unknown states are kept"
        );
    }

    #[test]
    fn parses_status_without_the_config() {
        let body = json!({
            "cluster": {"name": "01H", "peers": [{"name": "a"}, {"name": "b"}], "status": "ready"},
            "config": {"original": "receivers:\n- name: x\n  slack_configs:\n  - api_url: https://hooks.example.com/SECRET"},
            "uptime": "2026-09-25T10:00:00.000Z",
            "versionInfo": {"version": "0.28.1", "revision": "abc"}
        });
        let status = parse_status(&body).unwrap();
        assert_eq!(status.version.as_deref(), Some("0.28.1"));
        assert_eq!(status.peers, 2);
        assert!(!format!("{status:?}").contains("SECRET"));
        assert!(parse_status(&json!({"status": "success"})).is_err());
        assert!(parse_status(&json!("<html>")).is_err());
    }

    /// Prometheus 3.x (with `keepFiringFor`) and Thanos (no `evaluationTime`).
    #[test]
    fn parses_prometheus_rules_and_alerts() {
        let prom3 = json!({"status": "success", "data": {"groups": [
            {"name": "kubernetes-apps", "file": "/etc/prometheus/rules/monitoring-kube-prometheus-stack-kubernetes-apps.yaml", "interval": 30,
             "rules": [
                {"state": "firing", "name": "KubePodCrashLooping", "query": "max_over_time(x[5m]) >= 1", "duration": 900, "keepFiringFor": 0,
                 "labels": {"severity": "warning"}, "annotations": {"summary": "s"}, "alerts": [], "health": "ok",
                 "evaluationTime": 0.004, "lastEvaluation": "2026-09-26T08:30:00.1Z", "type": "alerting"},
                {"name": "up:sum", "query": "sum(up)", "health": "ok", "type": "recording"}
             ]},
            {"name": "recording-only", "file": "f", "rules": [{"name": "r", "type": "recording", "query": "1"}]}
        ]}});
        let groups = parse_rules(&prom3).unwrap();
        assert_eq!(groups.len(), 1);
        let rule = &groups[0].rules[0];
        assert_eq!(
            (rule.duration, rule.state.as_str(), rule.group.as_str()),
            (900.0, "firing", "kubernetes-apps")
        );
        assert!(rule.problem() && !rule.failing());
        let thanos = json!({"status": "success", "data": {"groups": [{"name": "g", "file": "", "rules": [
            {"name": "Broken", "query": "x", "health": "err", "lastError": "vector contains metrics with the same labelset", "state": "inactive", "type": "alerting"}]}]}});
        let rule = &parse_rules(&thanos).unwrap()[0].rules[0];
        assert!(rule.failing());
        assert!(parse_rules(&json!({"status": "error", "error": "forbidden"})).is_err());

        let alerts = json!({"status": "success", "data": {"alerts": [
            {"labels": {"alertname": "KubeHpaMaxedOut", "namespace": "p", "severity": "warning"}, "annotations": {}, "state": "pending",
             "activeAt": "2026-09-26T08:20:00Z", "value": "1e+00"}]}});
        let alerts = parse_prom_alerts(&alerts).unwrap();
        assert_eq!(alerts[0].state, "pending");
        assert_eq!(alerts[0].value.as_deref(), Some("1e+00"));
        let managers = json!({"status": "success", "data": {"activeAlertmanagers": [{"url": "http://10.244.1.7:9093/api/v2/alerts"}], "droppedAlertmanagers": []}});
        assert_eq!(
            parse_alertmanagers(&managers).unwrap(),
            ["http://10.244.1.7:9093/api/v2/alerts"]
        );
    }

    #[test]
    fn parses_silences() {
        let body = json!([{"id": "s1", "status": {"state": "active"}, "comment": "load test", "createdBy": "alice@example.com",
            "startsAt": "2026-09-26T08:00:00Z", "endsAt": "2026-09-26T12:00:00Z", "updatedAt": "2026-09-26T08:00:00Z",
            "matchers": [{"name": "alertname", "value": "KubeHpaMaxedOut", "isRegex": false, "isEqual": true},
                         {"name": "namespace", "value": "pay.*", "isRegex": true}]}]);
        let silences = parse_silences(&body, "am");
        assert_eq!(silences[0].matchers.len(), 2);
        assert!(silences[0].matches(&labels(&[
            ("alertname", "KubeHpaMaxedOut"),
            ("namespace", "payments")
        ])));
        assert!(!silences[0].matches(&labels(&[
            ("alertname", "KubeHpaMaxedOut"),
            ("namespace", "ops")
        ])));
        assert_eq!(
            parse_receivers(&json!([{"name": "a"}, {"name": "b"}])),
            ["a", "b"]
        );
    }
}
