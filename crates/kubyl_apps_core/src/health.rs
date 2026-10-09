//! How healthy a workload is, read from its status the way `kubectl rollout status` does.

use kubyl_resources_core::format::{array_at, int_at, str_at};
use serde_json::Value;

use crate::kinds::Kind;

/// Worst first, so sorting ascending puts the problems on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Health {
    /// A failed Job, a Deployment that can't make progress, pods that aren't available.
    Degraded,
    /// Replicas still starting or updating, a Job running.
    Progressing,
    Healthy,
    /// Scaled to zero, or a suspended CronJob or Job.
    Suspended,
    /// Nothing that reports health (an application of Services and ConfigMaps only).
    Unknown,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Health::Degraded => "Degraded",
            Health::Progressing => "Progressing",
            Health::Healthy => "Healthy",
            Health::Suspended => "Suspended",
            Health::Unknown => "No workloads",
        }
    }
}

/// A workload's health and a short account of it (`3/3 ready`).
pub fn of(kind: Kind, object: &Value) -> Option<(Health, String)> {
    match kind {
        Kind::Deployment => Some(deployment(object)),
        Kind::StatefulSet => Some(stateful_set(object)),
        Kind::DaemonSet => Some(daemon_set(object)),
        Kind::Job => Some(job(object)),
        Kind::CronJob => Some(cron_job(object)),
        _ => None,
    }
}

fn condition<'a>(object: &'a Value, kind: &str) -> Option<&'a Value> {
    array_at(object, "/status/conditions")
        .iter()
        .find(|c| str_at(c, "/type") == kind)
}

fn desired(object: &Value) -> i64 {
    // An absent `spec.replicas` means one.
    object
        .pointer("/spec/replicas")
        .and_then(Value::as_i64)
        .unwrap_or(1)
}

fn deployment(object: &Value) -> (Health, String) {
    let want = desired(object);
    let ready = int_at(object, "/status/readyReplicas");
    let updated = int_at(object, "/status/updatedReplicas");
    let total = int_at(object, "/status/replicas");
    let text = format!("{ready}/{want} ready");
    if want == 0 {
        return (Health::Suspended, "scaled to 0".into());
    }
    let exceeded = condition(object, "Progressing")
        .is_some_and(|c| str_at(c, "/reason") == "ProgressDeadlineExceeded");
    let failing =
        condition(object, "ReplicaFailure").is_some_and(|c| str_at(c, "/status") == "True");
    let unavailable =
        condition(object, "Available").is_some_and(|c| str_at(c, "/status") == "False");
    if exceeded || failing || unavailable && ready == 0 {
        return (Health::Degraded, text);
    }
    // Old replicas still around, or the update isn't through: the rollout is running.
    if ready < want || updated < want || total > want {
        return (Health::Progressing, text);
    }
    (Health::Healthy, text)
}

fn stateful_set(object: &Value) -> (Health, String) {
    let want = desired(object);
    let ready = int_at(object, "/status/readyReplicas");
    let text = format!("{ready}/{want} ready");
    if want == 0 {
        return (Health::Suspended, "scaled to 0".into());
    }
    let updated = int_at(object, "/status/updatedReplicas");
    let rolling = {
        let current = str_at(object, "/status/currentRevision");
        let update = str_at(object, "/status/updateRevision");
        !current.is_empty() && !update.is_empty() && current != update
    };
    if ready == 0 {
        return (Health::Degraded, text);
    }
    if ready < want || updated < want || rolling {
        return (Health::Progressing, text);
    }
    (Health::Healthy, text)
}

fn daemon_set(object: &Value) -> (Health, String) {
    let want = int_at(object, "/status/desiredNumberScheduled");
    let ready = int_at(object, "/status/numberReady");
    let text = format!("{ready}/{want} ready");
    if want == 0 {
        return (Health::Healthy, "no nodes".into());
    }
    if ready == 0 {
        return (Health::Degraded, text);
    }
    if ready < want || int_at(object, "/status/updatedNumberScheduled") < want {
        return (Health::Progressing, text);
    }
    (Health::Healthy, text)
}

fn job(object: &Value) -> (Health, String) {
    if condition(object, "Failed").is_some_and(|c| str_at(c, "/status") == "True") {
        return (Health::Degraded, "failed".into());
    }
    if condition(object, "Complete").is_some_and(|c| str_at(c, "/status") == "True") {
        return (Health::Healthy, "complete".into());
    }
    if object.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true) {
        return (Health::Suspended, "suspended".into());
    }
    if int_at(object, "/status/failed") > 0 && int_at(object, "/status/active") == 0 {
        return (Health::Degraded, "pods failed".into());
    }
    (Health::Progressing, "running".into())
}

fn cron_job(object: &Value) -> (Health, String) {
    if object.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true) {
        return (Health::Suspended, "suspended".into());
    }
    let schedule = str_at(object, "/spec/schedule");
    (Health::Healthy, schedule.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn health(kind: Kind, object: Value) -> Health {
        of(kind, &object).unwrap().0
    }

    #[test]
    fn deployments() {
        let ok = json!({"spec": {"replicas": 3}, "status": {"replicas": 3, "readyReplicas": 3, "updatedReplicas": 3,
            "conditions": [{"type": "Available", "status": "True"}]}});
        assert_eq!(
            of(Kind::Deployment, &ok).unwrap(),
            (Health::Healthy, "3/3 ready".into())
        );
        // A rollout: new pods starting, old ones around.
        let rolling = json!({"spec": {"replicas": 3}, "status": {"replicas": 4, "readyReplicas": 3, "updatedReplicas": 1}});
        assert_eq!(health(Kind::Deployment, rolling), Health::Progressing);
        let starting = json!({"spec": {"replicas": 2}, "status": {"replicas": 2, "readyReplicas": 1, "updatedReplicas": 2}});
        assert_eq!(health(Kind::Deployment, starting), Health::Progressing);
        let stuck = json!({"spec": {"replicas": 2}, "status": {"replicas": 2, "updatedReplicas": 1, "readyReplicas": 1,
            "conditions": [{"type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded"}]}});
        assert_eq!(health(Kind::Deployment, stuck), Health::Degraded);
        let down = json!({"spec": {"replicas": 2}, "status": {"replicas": 2,
            "conditions": [{"type": "Available", "status": "False"}]}});
        assert_eq!(health(Kind::Deployment, down), Health::Degraded);
        // Available=False with some pods ready is still a rollout, not an outage.
        let partial = json!({"spec": {"replicas": 2}, "status": {"replicas": 2, "readyReplicas": 1, "updatedReplicas": 2,
            "conditions": [{"type": "Available", "status": "False"}]}});
        assert_eq!(health(Kind::Deployment, partial), Health::Progressing);
        let quota = json!({"spec": {"replicas": 2}, "status": {"conditions": [{"type": "ReplicaFailure", "status": "True"}]}});
        assert_eq!(health(Kind::Deployment, quota), Health::Degraded);
        let off = json!({"spec": {"replicas": 0}, "status": {}});
        assert_eq!(
            of(Kind::Deployment, &off).unwrap(),
            (Health::Suspended, "scaled to 0".into())
        );
        // `spec.replicas` defaults to one.
        let one = json!({"spec": {}, "status": {"replicas": 1, "readyReplicas": 1, "updatedReplicas": 1}});
        assert_eq!(health(Kind::Deployment, one), Health::Healthy);
    }

    #[test]
    fn stateful_and_daemon_sets() {
        let ok = json!({"spec": {"replicas": 2}, "status": {"readyReplicas": 2, "updatedReplicas": 2,
            "currentRevision": "a", "updateRevision": "a"}});
        assert_eq!(health(Kind::StatefulSet, ok), Health::Healthy);
        let rolling = json!({"spec": {"replicas": 2}, "status": {"readyReplicas": 2, "updatedReplicas": 1,
            "currentRevision": "a", "updateRevision": "b"}});
        assert_eq!(health(Kind::StatefulSet, rolling), Health::Progressing);
        let down = json!({"spec": {"replicas": 2}, "status": {}});
        assert_eq!(health(Kind::StatefulSet, down), Health::Degraded);

        let ds = json!({"status": {"desiredNumberScheduled": 3, "numberReady": 3, "updatedNumberScheduled": 3}});
        assert_eq!(health(Kind::DaemonSet, ds), Health::Healthy);
        let partial = json!({"status": {"desiredNumberScheduled": 3, "numberReady": 2, "updatedNumberScheduled": 3}});
        assert_eq!(health(Kind::DaemonSet, partial), Health::Progressing);
        let none = json!({"status": {"desiredNumberScheduled": 3, "numberReady": 0}});
        assert_eq!(health(Kind::DaemonSet, none), Health::Degraded);
        let empty = json!({"status": {"desiredNumberScheduled": 0}});
        assert_eq!(health(Kind::DaemonSet, empty), Health::Healthy);
    }

    #[test]
    fn jobs_and_cron_jobs() {
        let done = json!({"status": {"conditions": [{"type": "Complete", "status": "True"}]}});
        assert_eq!(health(Kind::Job, done), Health::Healthy);
        let failed = json!({"status": {"conditions": [{"type": "Failed", "status": "True"}]}});
        assert_eq!(health(Kind::Job, failed), Health::Degraded);
        let running = json!({"status": {"active": 1}});
        assert_eq!(health(Kind::Job, running), Health::Progressing);
        let paused = json!({"spec": {"suspend": true}, "status": {}});
        assert_eq!(health(Kind::Job, paused), Health::Suspended);
        let cron = json!({"spec": {"schedule": "*/5 * * * *"}});
        assert_eq!(
            of(Kind::CronJob, &cron).unwrap(),
            (Health::Healthy, "*/5 * * * *".into())
        );
        let cron_paused = json!({"spec": {"suspend": true, "schedule": "@daily"}});
        assert_eq!(health(Kind::CronJob, cron_paused), Health::Suspended);
        assert_eq!(of(Kind::Service, &json!({})), None);
    }

    #[test]
    fn worst_sorts_first() {
        let mut all = [
            Health::Healthy,
            Health::Unknown,
            Health::Degraded,
            Health::Progressing,
            Health::Suspended,
        ];
        all.sort();
        assert_eq!(all[0], Health::Degraded);
        assert_eq!(all[4], Health::Unknown);
    }
}
