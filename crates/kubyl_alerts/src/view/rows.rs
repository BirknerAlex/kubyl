//! The Alerts table's rows: filters, grouping and the order (pure, tested).

use std::collections::{BTreeSet, HashMap, HashSet};

use jiff::Timestamp;

use crate::matchers::{self, Matcher};
use crate::model::{Alert, AlertState, Severity, TargetKind};
use crate::settings::GroupBy;

/// The filter input: matchers (`alertname=~"Kube.*"`) or plain text.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Query {
    #[default]
    None,
    Text(String),
    Matchers(Vec<Matcher>),
    /// Looks like matchers but doesn't parse (the input shows the error; nothing is hidden).
    Invalid(String),
}

impl Query {
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        if text.is_empty() {
            return Query::None;
        }
        let looks_like_matchers = text.starts_with('{')
            || text.split(',').next().is_some_and(|first| {
                let name_end = first.find(['=', '!']);
                name_end.is_some_and(|end| {
                    let name = first[..end].trim();
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '/')
                })
            });
        if !looks_like_matchers {
            return Query::Text(text.to_lowercase());
        }
        match matchers::parse(text) {
            Ok(matchers) => Query::Matchers(matchers),
            Err(err) => Query::Invalid(err),
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Query::Invalid(err) => Some(err),
            _ => None,
        }
    }

    fn matches(&self, alert: &Alert) -> bool {
        match self {
            Query::None | Query::Invalid(_) => true,
            Query::Matchers(matchers) => matchers.iter().all(|m| m.matches(&alert.labels)),
            Query::Text(text) => {
                alert.name.to_lowercase().contains(text)
                    || alert.summary().to_lowercase().contains(text)
                    || alert
                        .target
                        .as_ref()
                        .is_some_and(|t| t.label().to_lowercase().contains(text))
                    || alert
                        .labels
                        .values()
                        .any(|v| v.to_lowercase().contains(text))
            }
        }
    }
}

/// The severity chips.
pub const SEVERITY_CHIPS: [&str; 4] = ["critical", "warning", "info", "none"];
/// The state chips.
pub const STATE_CHIPS: [&str; 4] = ["firing", "pending", "silenced", "inhibited"];

/// Chip key of a severity: custom values count as `none`.
pub fn severity_key(severity: &Severity) -> &'static str {
    match severity {
        Severity::Critical => "critical",
        Severity::Warning => "warning",
        Severity::Info => "info",
        _ => "none",
    }
}

/// Chip key of a state (`unprocessed` counts as firing).
pub fn state_key(state: AlertState) -> &'static str {
    match state {
        AlertState::Firing | AlertState::Unprocessed => "firing",
        AlertState::Pending => "pending",
        AlertState::Silenced => "silenced",
        AlertState::Inhibited => "inhibited",
        AlertState::Resolved => "resolved",
    }
}

/// The alerts of one object ("Show Alerts for Selection", the details section): alerts whose
/// target is it, a workload's pods (`<name>-…`), or anything in a namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectFilter {
    pub kind: TargetKind,
    pub namespace: Option<String>,
    pub name: String,
}

impl ObjectFilter {
    /// The filter for a resource (`pods`, `deployments`…), if alerts can be about it.
    pub fn for_resource(resource: &str, namespace: Option<&str>, name: &str) -> Option<Self> {
        let kind = match resource {
            "pods" => TargetKind::Pod,
            "deployments" => TargetKind::Deployment,
            "statefulsets" => TargetKind::StatefulSet,
            "daemonsets" => TargetKind::DaemonSet,
            "replicasets" => TargetKind::ReplicaSet,
            "jobs" => TargetKind::Job,
            "cronjobs" => TargetKind::CronJob,
            "horizontalpodautoscalers" => TargetKind::HorizontalPodAutoscaler,
            "persistentvolumeclaims" => TargetKind::PersistentVolumeClaim,
            "nodes" => TargetKind::Node,
            "services" => TargetKind::Service,
            "namespaces" => TargetKind::Namespace,
            "clusteroperators" => TargetKind::ClusterOperator,
            _ => return None,
        };
        Some(Self {
            kind,
            namespace: namespace.map(str::to_string).filter(|_| kind.namespaced()),
            name: name.to_string(),
        })
    }

    /// `deployment/payment-gateway`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.kind.short(), self.name)
    }

    pub fn matches(&self, alert: &Alert) -> bool {
        if self.kind == TargetKind::Namespace {
            return alert.namespace() == Some(self.name.as_str());
        }
        let Some(target) = &alert.target else {
            return false;
        };
        if self.kind.namespaced() && target.namespace != self.namespace {
            return false;
        }
        if target.kind == self.kind && target.name == self.name {
            return true;
        }
        let owns_pods = matches!(
            self.kind,
            TargetKind::Deployment
                | TargetKind::StatefulSet
                | TargetKind::DaemonSet
                | TargetKind::ReplicaSet
                | TargetKind::Job
                | TargetKind::CronJob
        );
        // A workload's pods and ReplicaSets (`<name>-<hash>`).
        owns_pods
            && matches!(
                target.kind,
                TargetKind::Pod | TargetKind::ReplicaSet | TargetKind::Job
            )
            && target
                .name
                .strip_prefix(self.name.as_str())
                .is_some_and(|rest| rest.starts_with('-'))
    }
}

/// What the table shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub query: Query,
    /// Only the alerts of one object.
    pub object: Option<ObjectFilter>,
    /// Severity chips turned on (empty: all).
    pub severities: BTreeSet<String>,
    /// State chips turned on (empty: firing and pending, plus silenced and inhibited with
    /// `show_suppressed`).
    pub states: BTreeSet<String>,
    /// `Some("")`: alerts without a namespace ("Cluster").
    pub namespace: Option<String>,
    pub receiver: Option<String>,
    pub show_suppressed: bool,
}

impl Filters {
    /// Everything but the state (the severity chips' counts and the suppressed row use it).
    fn matches_scope(&self, alert: &Alert) -> bool {
        if let Some(object) = &self.object
            && !object.matches(alert)
        {
            return false;
        }
        if let Some(ns) = &self.namespace
            && alert.namespace().unwrap_or_default() != ns
        {
            return false;
        }
        if let Some(receiver) = &self.receiver
            && !alert.receivers.iter().any(|r| r == receiver)
        {
            return false;
        }
        self.query.matches(alert)
    }

    fn matches_severity(&self, alert: &Alert) -> bool {
        self.severities.is_empty() || self.severities.contains(severity_key(&alert.severity))
    }

    fn matches_state(&self, alert: &Alert) -> bool {
        let key = state_key(alert.state);
        if self.states.is_empty() {
            match alert.state {
                AlertState::Silenced | AlertState::Inhibited => self.show_suppressed,
                _ => true,
            }
        } else {
            self.states.contains(key)
        }
    }

    pub fn matches(&self, alert: &Alert) -> bool {
        self.matches_scope(alert) && self.matches_severity(alert) && self.matches_state(alert)
    }

    /// Nothing narrows the list (the "all clear" state can show).
    pub fn is_empty(&self) -> bool {
        matches!(self.query, Query::None)
            && self.object.is_none()
            && self.severities.is_empty()
            && self.states.is_empty()
            && self.namespace.is_none()
            && self.receiver.is_none()
    }
}

/// Counts for the chips, within the scope (namespace, receiver, query).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChipCounts {
    pub severities: HashMap<&'static str, usize>,
    pub states: HashMap<&'static str, usize>,
}

pub fn chip_counts(alerts: &[Alert], filters: &Filters) -> ChipCounts {
    let mut counts = ChipCounts::default();
    for alert in alerts.iter().filter(|a| filters.matches_scope(a)) {
        if filters.matches_state(alert) {
            *counts
                .severities
                .entry(severity_key(&alert.severity))
                .or_default() += 1;
        }
        if filters.matches_severity(alert) {
            *counts.states.entry(state_key(alert.state)).or_default() += 1;
        }
    }
    counts
}

/// One row of the table.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A group of alerts (only for 2 or more).
    Group {
        key: String,
        label: String,
        count: usize,
        severity: Severity,
        oldest: Option<Timestamp>,
        collapsed: bool,
    },
    /// An alert: an index into the active alerts, or into the resolved ones.
    Alert {
        index: usize,
        resolved: bool,
        nested: bool,
    },
    /// "5 silenced · 1 inhibited · Show" (hidden by default).
    Suppressed { silenced: usize, inhibited: usize },
    /// "Resolved in the last 15 min".
    ResolvedHeader { count: usize, collapsed: bool },
}

/// The key an alert is grouped under, and the group's label.
pub fn group_key(alert: &Alert, by: GroupBy) -> Option<(String, String)> {
    match by {
        GroupBy::None => None,
        GroupBy::Name => Some((alert.name.clone(), alert.name.clone())),
        GroupBy::Namespace => {
            let ns = alert.namespace().unwrap_or_default().to_string();
            let label = if ns.is_empty() {
                "Cluster".into()
            } else {
                ns.clone()
            };
            Some((ns, label))
        }
        GroupBy::Severity => {
            let label = alert.severity.label().to_string();
            Some((label.clone(), label))
        }
        GroupBy::Receiver => {
            let receivers = if alert.receivers.is_empty() {
                "no receiver".to_string()
            } else {
                alert.receivers.join(", ")
            };
            Some((receivers.clone(), receivers))
        }
        GroupBy::Target => {
            let target = alert
                .target
                .as_ref()
                .map(|t| t.label())
                .unwrap_or_else(|| "no target".into());
            Some((target.clone(), target))
        }
    }
}

/// Groups collapse by default when their worst alert is info or less.
fn collapsed_by_default(severity: &Severity) -> bool {
    severity.rank() >= Severity::Info.rank()
}

/// The rows for `alerts` (sorted: severity, then age) and `resolved`.
/// `toggled`: groups the user opened or closed (against their default).
pub fn build(
    alerts: &[Alert],
    resolved: &[Alert],
    filters: &Filters,
    by: GroupBy,
    toggled: &HashSet<String>,
    resolved_open: bool,
) -> Vec<Row> {
    let shown: Vec<usize> = (0..alerts.len())
        .filter(|&i| filters.matches(&alerts[i]))
        .collect();
    let mut rows = Vec::with_capacity(shown.len() + 8);
    match by {
        GroupBy::None => rows.extend(shown.iter().map(|&index| Row::Alert {
            index,
            resolved: false,
            nested: false,
        })),
        by => {
            // Groups in the order of their first (most severe, oldest) alert.
            let mut order: Vec<String> = Vec::new();
            let mut groups: HashMap<String, (String, Vec<usize>)> = HashMap::new();
            for &index in &shown {
                let Some((key, label)) = group_key(&alerts[index], by) else {
                    continue;
                };
                let entry = groups.entry(key.clone()).or_insert_with(|| {
                    order.push(key.clone());
                    (label, Vec::new())
                });
                entry.1.push(index);
            }
            for key in order {
                let (label, members) = &groups[&key];
                if members.len() == 1 {
                    rows.push(Row::Alert {
                        index: members[0],
                        resolved: false,
                        nested: false,
                    });
                    continue;
                }
                let severity = members
                    .iter()
                    .map(|&i| &alerts[i].severity)
                    .min_by_key(|s| s.rank())
                    .cloned()
                    .unwrap_or(Severity::None);
                let oldest = members.iter().filter_map(|&i| alerts[i].since()).min();
                let collapsed = collapsed_by_default(&severity) != toggled.contains(&key);
                rows.push(Row::Group {
                    key: key.clone(),
                    label: label.clone(),
                    count: members.len(),
                    severity,
                    oldest,
                    collapsed,
                });
                if !collapsed {
                    rows.extend(members.iter().map(|&index| Row::Alert {
                        index,
                        resolved: false,
                        nested: true,
                    }));
                }
            }
        }
    }
    if filters.states.is_empty() && !filters.show_suppressed {
        let hidden = alerts
            .iter()
            .filter(|a| a.state.suppressed() && filters.matches_scope(a))
            .filter(|a| {
                filters.severities.is_empty()
                    || filters.severities.contains(severity_key(&a.severity))
            });
        let (mut silenced, mut inhibited) = (0, 0);
        for alert in hidden {
            if alert.state == AlertState::Silenced {
                silenced += 1;
            } else {
                inhibited += 1;
            }
        }
        if silenced + inhibited > 0 {
            rows.push(Row::Suppressed {
                silenced,
                inhibited,
            });
        }
    }
    let resolved_shown: Vec<usize> = (0..resolved.len())
        .filter(|&i| {
            let alert = &resolved[i];
            filters.matches_scope(alert)
                && filters.matches_severity(alert)
                && (filters.states.is_empty() || filters.states.contains("resolved"))
        })
        .collect();
    if !resolved_shown.is_empty() {
        rows.push(Row::ResolvedHeader {
            count: resolved_shown.len(),
            collapsed: !resolved_open,
        });
        if resolved_open {
            rows.extend(resolved_shown.into_iter().map(|index| Row::Alert {
                index,
                resolved: true,
                nested: false,
            }));
        }
    }
    rows
}

/// The alert a row shows.
pub fn alert_of<'a>(row: &Row, alerts: &'a [Alert], resolved: &'a [Alert]) -> Option<&'a Alert> {
    match row {
        Row::Alert {
            index,
            resolved: false,
            ..
        } => alerts.get(*index),
        Row::Alert {
            index,
            resolved: true,
            ..
        } => resolved.get(*index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Target, TargetKind};

    pub(crate) fn alert(name: &str, severity: Severity, state: AlertState, ns: &str) -> Alert {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("alertname".to_string(), name.to_string());
        if !ns.is_empty() {
            labels.insert("namespace".to_string(), ns.to_string());
        }
        Alert {
            fingerprint: format!("{name}-{ns}-{}", state_key(state)),
            name: name.into(),
            labels,
            annotations: Default::default(),
            severity,
            state,
            silenced_by: Vec::new(),
            inhibited_by: Vec::new(),
            starts_at: None,
            starts_approx: false,
            active_at: None,
            updated_at: None,
            ends_at: None,
            receivers: vec!["pagerduty".into()],
            generator_url: None,
            value: None,
            source: String::new(),
            target: Some(Target {
                kind: TargetKind::Namespace,
                namespace: None,
                name: ns.into(),
                container: None,
            }),
        }
    }

    fn sample() -> Vec<Alert> {
        vec![
            alert(
                "KubePodCrashLooping",
                Severity::Critical,
                AlertState::Firing,
                "payments",
            ),
            alert(
                "KubePodCrashLooping",
                Severity::Critical,
                AlertState::Firing,
                "checkout",
            ),
            alert(
                "KubeNodeNotReady",
                Severity::Critical,
                AlertState::Firing,
                "",
            ),
            alert(
                "KubeHpaMaxedOut",
                Severity::Warning,
                AlertState::Pending,
                "payments",
            ),
            alert(
                "CPUThrottlingHigh",
                Severity::Info,
                AlertState::Firing,
                "payments",
            ),
            alert(
                "CPUThrottlingHigh",
                Severity::Info,
                AlertState::Firing,
                "checkout",
            ),
            alert(
                "KubeJobFailed",
                Severity::Warning,
                AlertState::Silenced,
                "payments",
            ),
            alert(
                "KubeJobFailed",
                Severity::Warning,
                AlertState::Inhibited,
                "checkout",
            ),
        ]
    }

    #[test]
    fn groups_of_two_or_more_with_suppressed_hidden() {
        let alerts = sample();
        let rows = build(
            &alerts,
            &[],
            &Filters::default(),
            GroupBy::Name,
            &HashSet::new(),
            false,
        );
        let kinds: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Group {
                    label,
                    count,
                    collapsed,
                    ..
                } => format!("group {label} x{count} {collapsed}"),
                Row::Alert { index, nested, .. } => {
                    format!("alert {} {nested}", alerts[*index].name)
                }
                Row::Suppressed {
                    silenced,
                    inhibited,
                } => format!("suppressed {silenced}/{inhibited}"),
                Row::ResolvedHeader { count, .. } => format!("resolved {count}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "group KubePodCrashLooping x2 false",
                "alert KubePodCrashLooping true",
                "alert KubePodCrashLooping true",
                "alert KubeNodeNotReady false",
                "alert KubeHpaMaxedOut false",
                // Info groups start collapsed.
                "group CPUThrottlingHigh x2 true",
                "suppressed 1/1",
            ]
        );
        let toggled = HashSet::from(["CPUThrottlingHigh".to_string()]);
        let rows = build(
            &alerts,
            &[],
            &Filters::default(),
            GroupBy::Name,
            &toggled,
            false,
        );
        assert!(rows.iter().any(|r| matches!(r, Row::Group { collapsed: false, label, .. } if label == "CPUThrottlingHigh")));
    }

    #[test]
    fn filters_by_matchers_text_namespace_and_chips() {
        let alerts = sample();
        let mut filters = Filters {
            query: Query::parse(r#"alertname=~"Kube.*", namespace="payments""#),
            ..Default::default()
        };
        let shown: Vec<&str> = alerts
            .iter()
            .filter(|a| filters.matches(a))
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(shown, ["KubePodCrashLooping", "KubeHpaMaxedOut"]);
        filters.query = Query::parse("crash");
        assert_eq!(alerts.iter().filter(|a| filters.matches(a)).count(), 2);
        filters.query = Query::None;
        filters.namespace = Some(String::new());
        let shown: Vec<&str> = alerts
            .iter()
            .filter(|a| filters.matches(a))
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(
            shown,
            ["KubeNodeNotReady"],
            "Cluster: alerts without a namespace"
        );
        filters.namespace = None;
        filters.states = BTreeSet::from(["silenced".to_string()]);
        assert_eq!(alerts.iter().filter(|a| filters.matches(a)).count(), 1);
        let counts = chip_counts(&alerts, &Filters::default());
        assert_eq!(counts.severities["critical"], 3);
        assert_eq!(counts.states["silenced"], 1);
        assert!(Query::parse(r#"a=~"(""#).error().is_some());
        assert_eq!(Query::parse("  "), Query::None);
    }

    #[test]
    fn objects_include_their_pods() {
        let mut pod = alert(
            "KubePodCrashLooping",
            Severity::Critical,
            AlertState::Firing,
            "payments",
        );
        pod.target = Some(Target {
            kind: TargetKind::Pod,
            namespace: Some("payments".into()),
            name: "payment-gateway-5c8b7f9d4-hl2vp".into(),
            container: None,
        });
        let deployment =
            ObjectFilter::for_resource("deployments", Some("payments"), "payment-gateway").unwrap();
        assert!(deployment.matches(&pod));
        let elsewhere =
            ObjectFilter::for_resource("deployments", Some("checkout"), "payment-gateway").unwrap();
        assert!(!elsewhere.matches(&pod));
        let namespace = ObjectFilter::for_resource("namespaces", None, "payments").unwrap();
        assert!(namespace.matches(&pod));
        assert!(ObjectFilter::for_resource("configmaps", None, "x").is_none());
    }

    #[test]
    fn resolved_rows_follow() {
        let alerts = sample();
        let mut gone = alert(
            "TargetDown",
            Severity::Warning,
            AlertState::Resolved,
            "payments",
        );
        gone.ends_at = Some(Timestamp::now());
        let rows = build(
            &alerts,
            &[gone],
            &Filters::default(),
            GroupBy::None,
            &HashSet::new(),
            true,
        );
        assert!(matches!(
            rows[rows.len() - 2],
            Row::ResolvedHeader {
                count: 1,
                collapsed: false
            }
        ));
        assert!(matches!(
            rows.last(),
            Some(Row::Alert { resolved: true, .. })
        ));
    }
}
