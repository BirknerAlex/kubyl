//! One list from Alertmanager's alerts and Prometheus' (pending ones, values), plus the
//! heartbeat.
//!
//! A Prometheus alert belongs to the Alertmanager alert with the same name whose labels include
//! all of the Prometheus alert's labels (Prometheus adds its external labels when it sends).
//! Without Alertmanager, "firing since" is `activeAt` plus the rule's `for`, marked approximate.

use std::collections::BTreeMap;
use std::hash::{Hash as _, Hasher as _};
use std::time::Duration;

use jiff::Timestamp;

use crate::model::{Alert, AlertState, ParseOptions, PromAlert, RuleGroup, target_of};

/// A heartbeat received within this long means the pipeline works.
pub const HEARTBEAT_FRESH: Duration = Duration::from_secs(300);

/// Whether alerts get from Prometheus to Alertmanager (the `Watchdog` alert always fires).
#[derive(Clone, Debug, PartialEq)]
pub enum Heartbeat {
    /// No heartbeat alert and no rule for one: nothing to say.
    Unknown,
    /// Received at…
    Ok(Timestamp),
    /// Received, but not within [`HEARTBEAT_FRESH`].
    Stale(Timestamp),
    /// Prometheus has the rule but Alertmanager doesn't have the alert.
    NotDelivered,
    /// …and Prometheus lists no Alertmanager at all.
    NoAlertmanagerConfigured,
    /// Only Prometheus: the heartbeat fires there (delivery can't be checked).
    FiringInPrometheus,
}

impl Heartbeat {
    pub fn problem(&self) -> bool {
        matches!(
            self,
            Heartbeat::Stale(_) | Heartbeat::NotDelivered | Heartbeat::NoAlertmanagerConfigured
        )
    }
}

/// What the merge needs besides the alerts.
pub struct MergeOptions<'a> {
    pub heartbeat_alerts: &'a [String],
    pub hidden_alerts: &'a [String],
    pub parse: &'a ParseOptions<'a>,
}

/// The merged list and the heartbeat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Merged {
    pub alerts: Vec<Alert>,
    pub heartbeat: Option<Heartbeat>,
}

/// A stable id for a Prometheus-only alert (Alertmanager's fingerprints are hashes of labels
/// too; these are prefixed so they never collide).
pub fn fingerprint_of(labels: &BTreeMap<String, String>) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    labels.hash(&mut hasher);
    format!("p{:016x}", hasher.finish())
}

fn includes(outer: &BTreeMap<String, String>, inner: &BTreeMap<String, String>) -> bool {
    inner.iter().all(|(k, v)| outer.get(k) == Some(v))
}

/// Merges `am` (`None`: no Alertmanager) and `prom` (`None`: no rules source).
/// `alertmanagers`: how many Alertmanagers Prometheus sends to, when known.
pub fn merge(
    am: Option<&[Alert]>,
    prom: Option<&[PromAlert]>,
    rules: &[RuleGroup],
    alertmanagers: Option<usize>,
    options: &MergeOptions,
    now: Timestamp,
) -> Merged {
    let mut alerts: Vec<Alert> = am.map(<[Alert]>::to_vec).unwrap_or_default();
    let rule_for = |name: &str| {
        rules
            .iter()
            .flat_map(|g| &g.rules)
            .find(|r| r.name == name)
            .map(|r| r.duration)
    };
    for p in prom.unwrap_or_default() {
        let Some(name) = p.labels.get("alertname").cloned() else {
            continue;
        };
        let pending = p.state == "pending";
        if !pending
            && let Some(existing) = alerts
                .iter_mut()
                .find(|a| a.name == name && includes(&a.labels, &p.labels))
        {
            existing.value = p.value.clone();
            existing.active_at = existing.active_at.or(p.active_at);
            continue;
        }
        let (starts_at, approx) = match (pending, p.active_at) {
            (false, Some(active)) => {
                let wait = rule_for(&name).unwrap_or(0.0).max(0.0);
                let starts = active
                    .checked_add(jiff::SignedDuration::from_secs_f64(wait))
                    .unwrap_or(active);
                (Some(starts), true)
            }
            _ => (None, false),
        };
        alerts.push(Alert {
            fingerprint: fingerprint_of(&p.labels),
            target: target_of(&name, &p.labels, options.parse.node_of),
            severity: options_severity(options.parse, &p.labels),
            annotations: p.annotations.clone(),
            state: if pending {
                AlertState::Pending
            } else {
                AlertState::Firing
            },
            silenced_by: Vec::new(),
            inhibited_by: Vec::new(),
            starts_at,
            starts_approx: approx,
            active_at: p.active_at,
            updated_at: None,
            ends_at: None,
            receivers: Vec::new(),
            generator_url: None,
            value: p.value.clone(),
            source: "Prometheus".into(),
            labels: p.labels.clone(),
            name,
        });
    }

    let heartbeat = heartbeat(
        am,
        prom,
        rules,
        alertmanagers,
        options.heartbeat_alerts,
        now,
    );
    alerts.retain(|a| {
        !options.heartbeat_alerts.contains(&a.name) && !options.hidden_alerts.contains(&a.name)
    });
    sort(&mut alerts);
    Merged { alerts, heartbeat }
}

fn options_severity(
    parse: &ParseOptions,
    labels: &BTreeMap<String, String>,
) -> crate::model::Severity {
    crate::model::Severity::from_value(
        labels
            .get(parse.severity_label)
            .map(String::as_str)
            .unwrap_or_default(),
        parse.severities,
    )
}

/// Severity first, then the oldest.
pub fn sort(alerts: &mut [Alert]) {
    alerts.sort_by(|a, b| {
        a.severity
            .rank()
            .cmp(&b.severity.rank())
            .then_with(|| a.since().cmp(&b.since()))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
}

fn heartbeat(
    am: Option<&[Alert]>,
    prom: Option<&[PromAlert]>,
    rules: &[RuleGroup],
    alertmanagers: Option<usize>,
    names: &[String],
    now: Timestamp,
) -> Option<Heartbeat> {
    if names.is_empty() {
        return None;
    }
    let is_heartbeat = |name: Option<&String>| name.is_some_and(|n| names.contains(n));
    let has_rule = rules
        .iter()
        .flat_map(|g| &g.rules)
        .any(|r| names.contains(&r.name));
    match am {
        Some(am) => {
            let received = am
                .iter()
                .filter(|a| is_heartbeat(Some(&a.name)))
                .filter_map(|a| a.updated_at.or(a.starts_at))
                .max();
            Some(match received {
                Some(at)
                    if now.duration_since(at).as_secs() <= HEARTBEAT_FRESH.as_secs() as i64 =>
                {
                    Heartbeat::Ok(at)
                }
                Some(at) => Heartbeat::Stale(at),
                None if has_rule && alertmanagers == Some(0) => Heartbeat::NoAlertmanagerConfigured,
                None if has_rule => Heartbeat::NotDelivered,
                None => Heartbeat::Unknown,
            })
        }
        None => {
            let firing = prom
                .unwrap_or_default()
                .iter()
                .any(|p| p.state == "firing" && is_heartbeat(p.labels.get("alertname")));
            firing.then_some(Heartbeat::FiringInPrometheus)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::options;
    use crate::model::{Rule, Severity};

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn am_alert(name: &str, extra: &[(&str, &str)], starts: &str) -> Alert {
        let mut l = labels(extra);
        l.insert("alertname".into(), name.into());
        Alert {
            fingerprint: format!("f-{name}"),
            name: name.into(),
            severity: Severity::from_value(
                l.get("severity").map(String::as_str).unwrap_or(""),
                &BTreeMap::new(),
            ),
            labels: l,
            annotations: BTreeMap::new(),
            state: AlertState::Firing,
            silenced_by: vec![],
            inhibited_by: vec![],
            starts_at: Some(ts(starts)),
            starts_approx: false,
            active_at: None,
            updated_at: Some(ts(starts)),
            ends_at: None,
            receivers: vec![],
            generator_url: None,
            value: None,
            source: "am".into(),
            target: None,
        }
    }

    fn prom_alert(name: &str, extra: &[(&str, &str)], state: &str, active: &str) -> PromAlert {
        let mut l = labels(extra);
        l.insert("alertname".into(), name.into());
        PromAlert {
            labels: l,
            annotations: BTreeMap::new(),
            state: state.into(),
            active_at: Some(ts(active)),
            value: Some("1".into()),
        }
    }

    fn rules(pairs: &[(&str, f64)]) -> Vec<RuleGroup> {
        vec![RuleGroup {
            name: "g".into(),
            file: "f".into(),
            rules: pairs
                .iter()
                .map(|(name, duration)| Rule {
                    name: name.to_string(),
                    group: "g".into(),
                    file: "f".into(),
                    query: "x".into(),
                    duration: *duration,
                    keep_firing_for: 0.0,
                    labels: BTreeMap::new(),
                    annotations: BTreeMap::new(),
                    health: "ok".into(),
                    last_error: None,
                    last_evaluation: None,
                    evaluation_time: 0.0,
                    state: "inactive".into(),
                })
                .collect(),
        }]
    }

    fn opts<'a>(
        parse: &'a ParseOptions<'a>,
        heartbeat: &'a [String],
        hidden: &'a [String],
    ) -> MergeOptions<'a> {
        MergeOptions {
            heartbeat_alerts: heartbeat,
            hidden_alerts: hidden,
            parse,
        }
    }

    #[test]
    fn prometheus_alerts_join_their_alertmanager_alert() {
        let parse = options();
        let (hb, hidden) = (
            vec!["Watchdog".to_string()],
            vec!["InfoInhibitor".to_string()],
        );
        let now = ts("2026-09-26T09:00:00Z");
        // Alertmanager has the external label `cluster`, Prometheus doesn't.
        let am = vec![
            am_alert(
                "KubePodCrashLooping",
                &[("severity", "warning"), ("pod", "a"), ("cluster", "eu")],
                "2026-09-26T08:00:00Z",
            ),
            am_alert(
                "KubeNodeNotReady",
                &[("severity", "critical"), ("node", "n")],
                "2026-09-26T08:30:00Z",
            ),
            am_alert("Watchdog", &[("severity", "none")], "2026-09-26T08:58:00Z"),
            am_alert(
                "InfoInhibitor",
                &[("severity", "none")],
                "2026-09-26T08:00:00Z",
            ),
        ];
        let prom = vec![
            prom_alert(
                "KubePodCrashLooping",
                &[("severity", "warning"), ("pod", "a")],
                "firing",
                "2026-09-26T07:45:00Z",
            ),
            prom_alert(
                "KubeHpaMaxedOut",
                &[("severity", "warning")],
                "pending",
                "2026-09-26T08:50:00Z",
            ),
        ];
        let merged = merge(
            Some(&am),
            Some(&prom),
            &rules(&[("Watchdog", 0.0)]),
            Some(1),
            &opts(&parse, &hb, &hidden),
            now,
        );
        let names: Vec<&str> = merged.alerts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            ["KubeNodeNotReady", "KubePodCrashLooping", "KubeHpaMaxedOut"]
        );
        let crash = &merged.alerts[1];
        assert_eq!(crash.value.as_deref(), Some("1"));
        assert_eq!(
            crash.starts_at,
            Some(ts("2026-09-26T08:00:00Z")),
            "firing since is Alertmanager's startsAt"
        );
        let pending = &merged.alerts[2];
        assert_eq!(pending.state, AlertState::Pending);
        assert_eq!(
            pending.since(),
            Some(ts("2026-09-26T08:50:00Z")),
            "pending since activeAt"
        );
        assert_eq!(
            merged.heartbeat,
            Some(Heartbeat::Ok(ts("2026-09-26T08:58:00Z")))
        );
    }

    #[test]
    fn without_alertmanager_firing_since_is_approximate() {
        let parse = options();
        let hb = vec!["Watchdog".to_string()];
        let prom = vec![
            prom_alert(
                "KubeJobFailed",
                &[("severity", "warning")],
                "firing",
                "2026-09-26T08:00:00Z",
            ),
            prom_alert("Watchdog", &[], "firing", "2026-09-26T07:00:00Z"),
        ];
        let merged = merge(
            None,
            Some(&prom),
            &rules(&[("KubeJobFailed", 900.0)]),
            None,
            &opts(&parse, &hb, &[]),
            ts("2026-09-26T09:00:00Z"),
        );
        assert_eq!(merged.alerts.len(), 1);
        assert_eq!(merged.alerts[0].starts_at, Some(ts("2026-09-26T08:15:00Z")));
        assert!(merged.alerts[0].starts_approx);
        assert_eq!(merged.heartbeat, Some(Heartbeat::FiringInPrometheus));
    }

    #[test]
    fn heartbeat_explains_a_broken_pipeline() {
        let parse = options();
        let hb = vec!["Watchdog".to_string()];
        let now = ts("2026-09-26T09:00:00Z");
        let o = opts(&parse, &hb, &[]);
        let r = rules(&[("Watchdog", 0.0)]);
        assert_eq!(
            merge(Some(&[]), None, &r, Some(1), &o, now).heartbeat,
            Some(Heartbeat::NotDelivered)
        );
        assert_eq!(
            merge(Some(&[]), None, &r, Some(0), &o, now).heartbeat,
            Some(Heartbeat::NoAlertmanagerConfigured)
        );
        assert_eq!(
            merge(Some(&[]), None, &[], None, &o, now).heartbeat,
            Some(Heartbeat::Unknown)
        );
        let old = vec![am_alert("Watchdog", &[], "2026-09-26T08:00:00Z")];
        let stale = merge(Some(&old), None, &r, None, &o, now)
            .heartbeat
            .unwrap();
        assert!(stale.problem());
    }
}
