//! Pod status exactly like `kubectl get pods` (`printPod` in kubectl's printers).

use jiff::Timestamp;
use kubyl_base::Tone;
use serde_json::Value;

use crate::format::{array_at, int_at, seconds_since, str_at, timestamp};
use crate::route::Route;

/// What `kubectl get pods` shows in READY, STATUS and RESTARTS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodStatus {
    /// `Running`, `CrashLoopBackOff`, `Init:0/2`, `Terminating`…
    pub reason: String,
    pub ready: u32,
    pub total: u32,
    pub restarts: u32,
    pub last_restart: Option<Timestamp>,
}

impl PodStatus {
    /// `1/2`.
    pub fn ready_label(&self) -> String {
        format!("{}/{}", self.ready, self.total)
    }

    pub fn tone(&self) -> Tone {
        status_tone(&self.reason)
    }

    /// Not all containers are ready, and the pod isn't done.
    pub fn is_degraded(&self) -> bool {
        self.ready < self.total && !matches!(self.reason.as_str(), "Completed" | "Succeeded")
    }
}

/// Status color, as in the mockup: Running green, Pending yellow, ContainerCreating blue,
/// failures red, Completed dim.
pub fn status_tone(reason: &str) -> Tone {
    match reason {
        "Running" | "Succeeded" | "Ready" | "Active" | "Bound" | "Available" | "Complete"
        | "SuccessCriteriaMet" => Tone::Good,
        "Completed" => Tone::Muted,
        "Pending" | "Terminating" | "Unknown" | "SchedulingGated" | "Suspended" => Tone::Warning,
        "ContainerCreating" | "PodInitializing" => Tone::Info,
        r if r.starts_with("Init:") => {
            let rest = &r[5..];
            if rest.contains('/') {
                Tone::Info
            } else {
                Tone::Bad
            }
        }
        "CrashLoopBackOff"
        | "OOMKilled"
        | "Error"
        | "Failed"
        | "FailureTarget"
        | "NotReady"
        | "ImagePullBackOff"
        | "ErrImagePull"
        | "InvalidImageName"
        | "CreateContainerConfigError"
        | "CreateContainerError"
        | "RunContainerError"
        | "Evicted"
        | "ContainerStatusUnknown"
        | "DeadlineExceeded"
        | "Lost" => Tone::Bad,
        r if r.starts_with("ExitCode:") || r.starts_with("Signal:") => Tone::Bad,
        _ => Tone::Neutral,
    }
}

fn restartable_init(spec: &Value, name: &str) -> bool {
    array_at(spec, "/initContainers")
        .iter()
        .find(|c| str_at(c, "/name") == name)
        .is_some_and(|c| str_at(c, "/restartPolicy") == "Always")
}

fn condition_true(status: &Value, kind: &str) -> bool {
    array_at(status, "/conditions")
        .iter()
        .any(|c| str_at(c, "/type") == kind && str_at(c, "/status") == "True")
}

fn later(a: Option<Timestamp>, b: Option<Timestamp>) -> Option<Timestamp> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Derives READY/STATUS/RESTARTS like kubectl.
pub fn pod_status(pod: &Value) -> PodStatus {
    let spec = &pod["spec"];
    let status = &pod["status"];
    let init_specs = array_at(spec, "/initContainers");
    let mut total = array_at(spec, "/containers").len() as u32;
    let mut ready = 0;
    let mut restarts = 0u32;
    let mut restartable_restarts = 0u32;
    let mut last_restart = None;
    let mut last_restartable_restart = None;

    let phase = str_at(status, "/phase");
    let mut reason = match str_at(status, "/reason") {
        "" => phase.to_string(),
        reason => reason.to_string(),
    };
    for condition in array_at(status, "/conditions") {
        if str_at(condition, "/type") == "PodScheduled"
            && str_at(condition, "/reason") == "SchedulingGated"
        {
            reason = "SchedulingGated".into();
        }
    }
    total += init_specs
        .iter()
        .filter(|c| str_at(c, "/restartPolicy") == "Always")
        .count() as u32;

    let mut initializing = false;
    for (i, container) in array_at(status, "/initContainerStatuses")
        .iter()
        .enumerate()
    {
        let count = container["restartCount"].as_u64().unwrap_or_default() as u32;
        restarts += count;
        let finished = timestamp(str_at(container, "/lastState/terminated/finishedAt"));
        last_restart = later(last_restart, finished);
        let name = str_at(container, "/name");
        let restartable = restartable_init(spec, name);
        if restartable {
            restartable_restarts += count;
            last_restartable_restart = later(last_restartable_restart, finished);
        }
        let terminated = container.pointer("/state/terminated");
        let waiting_reason = str_at(container, "/state/waiting/reason");
        if let Some(terminated) = terminated
            && terminated["exitCode"].as_i64() == Some(0)
        {
            continue;
        }
        if restartable && container["started"].as_bool() == Some(true) {
            if container["ready"].as_bool() == Some(true) {
                ready += 1;
            }
            continue;
        }
        if let Some(terminated) = terminated {
            let why = str_at(terminated, "/reason");
            reason = if why.is_empty() {
                match terminated["signal"].as_i64().unwrap_or_default() {
                    0 => format!(
                        "Init:ExitCode:{}",
                        terminated["exitCode"].as_i64().unwrap_or_default()
                    ),
                    signal => format!("Init:Signal:{signal}"),
                }
            } else {
                format!("Init:{why}")
            };
        } else if !waiting_reason.is_empty() && waiting_reason != "PodInitializing" {
            reason = format!("Init:{waiting_reason}");
        } else {
            reason = format!("Init:{i}/{}", init_specs.len());
        }
        initializing = true;
        break;
    }

    if !initializing || condition_true(status, "Initialized") {
        restarts = restartable_restarts;
        last_restart = last_restartable_restart;
        let mut has_running = false;
        for container in array_at(status, "/containerStatuses").iter().rev() {
            restarts += container["restartCount"].as_u64().unwrap_or_default() as u32;
            last_restart = later(
                last_restart,
                timestamp(str_at(container, "/lastState/terminated/finishedAt")),
            );
            let waiting = str_at(container, "/state/waiting/reason");
            let terminated = container.pointer("/state/terminated");
            if !waiting.is_empty() {
                reason = waiting.to_string();
            } else if let Some(terminated) = terminated {
                let why = str_at(terminated, "/reason");
                reason = if !why.is_empty() {
                    why.to_string()
                } else {
                    match terminated["signal"].as_i64().unwrap_or_default() {
                        0 => format!(
                            "ExitCode:{}",
                            terminated["exitCode"].as_i64().unwrap_or_default()
                        ),
                        signal => format!("Signal:{signal}"),
                    }
                };
            } else if container["ready"].as_bool() == Some(true)
                && container.pointer("/state/running").is_some()
            {
                has_running = true;
                ready += 1;
            }
        }
        if reason == "Completed" && has_running {
            reason = if condition_true(status, "Ready") {
                "Running".into()
            } else {
                "NotReady".into()
            };
        }
    }

    let deleting = pod.pointer("/metadata/deletionTimestamp").is_some();
    if deleting && str_at(status, "/reason") == "NodeLost" {
        reason = "Unknown".into();
    } else if deleting && !matches!(phase, "Succeeded" | "Failed") {
        reason = "Terminating".into();
    }

    PodStatus {
        reason,
        ready,
        total,
        restarts,
        last_restart,
    }
}

/// `Complete`, `Failed`, `Terminating`, `Suspended`, `FailureTarget`, `SuccessCriteriaMet` or
/// `Running`, in kubectl's order.
pub fn job_status(job: &Value) -> &'static str {
    let condition = |kind: &str| {
        array_at(job, "/status/conditions")
            .iter()
            .any(|c| str_at(c, "/type") == kind && str_at(c, "/status") == "True")
    };
    if condition("Complete") {
        "Complete"
    } else if condition("Failed") {
        "Failed"
    } else if job.pointer("/metadata/deletionTimestamp").is_some() {
        "Terminating"
    } else if condition("Suspended") {
        "Suspended"
    } else if condition("FailureTarget") {
        "FailureTarget"
    } else if condition("SuccessCriteriaMet") {
        "SuccessCriteriaMet"
    } else {
        "Running"
    }
}

/// The tone of a [`job_status`] in the Jobs list (a running Job is `Info`).
pub fn job_tone(status: &str) -> Tone {
    match status {
        "Complete" | "SuccessCriteriaMet" => Tone::Good,
        "Failed" | "FailureTarget" => Tone::Bad,
        "Suspended" | "Terminating" => Tone::Warning,
        _ => Tone::Info,
    }
}

/// `succeeded/wanted`, with `X/1 of N` for a work queue (no `completions`, parallelism N).
pub fn job_completions(job: &Value) -> String {
    let succeeded = int_at(job, "/status/succeeded");
    match job.pointer("/spec/completions").and_then(Value::as_i64) {
        Some(completions) => format!("{succeeded}/{completions}"),
        None => match int_at(job, "/spec/parallelism") {
            p if p > 1 => format!("{succeeded}/1 of {p}"),
            _ => format!("{succeeded}/1"),
        },
    }
}

/// How long a finished Job ran, in seconds: until its completion time, or until the `Failed`
/// condition. `None` while it runs.
pub fn job_run_time(job: &Value) -> Option<i64> {
    let start = timestamp(str_at(job, "/status/startTime"))?;
    let end = timestamp(str_at(job, "/status/completionTime")).or_else(|| {
        array_at(job, "/status/conditions")
            .iter()
            .find(|c| str_at(c, "/type") == "Failed" && str_at(c, "/status") == "True")
            .and_then(|c| timestamp(str_at(c, "/lastTransitionTime")))
    })?;
    Some(seconds_since(start, end).max(0))
}

/// `Ready`, `NotReady` or `Unknown`, plus `,SchedulingDisabled` when cordoned.
pub fn node_status(node: &Value) -> String {
    let ready = array_at(node, "/status/conditions")
        .iter()
        .find(|c| str_at(c, "/type") == "Ready")
        .map(|c| match str_at(c, "/status") {
            "True" => "Ready",
            "False" => "NotReady",
            _ => "Unknown",
        })
        .unwrap_or("Unknown");
    if node.pointer("/spec/unschedulable").and_then(Value::as_bool) == Some(true) {
        format!("{ready},SchedulingDisabled")
    } else {
        ready.to_string()
    }
}

/// Node roles from `node-role.kubernetes.io/<role>` and `kubernetes.io/role` labels.
pub fn node_roles(node: &Value) -> Vec<String> {
    let mut roles: Vec<String> = node
        .pointer("/metadata/labels")
        .and_then(Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(k, v)| {
                    if let Some(role) = k.strip_prefix("node-role.kubernetes.io/") {
                        Some(role.to_string()).filter(|r| !r.is_empty())
                    } else if k == "kubernetes.io/role" {
                        v.as_str().map(String::from)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    roles.sort();
    roles.dedup();
    roles
}

/// When an event last happened (core/v1 and events.k8s.io/v1).
pub fn event_time(event: &Value) -> Option<Timestamp> {
    [
        "/series/lastObservedTime",
        "/lastTimestamp",
        "/deprecatedLastTimestamp",
        "/eventTime",
        "/firstTimestamp",
        "/metadata/creationTimestamp",
    ]
    .iter()
    .find_map(|p| timestamp(str_at(event, p)))
}

/// `Kind/name` of the object an event is about.
pub fn event_object(event: &Value) -> String {
    let object = event
        .get("involvedObject")
        .or_else(|| event.get("regarding"))
        .unwrap_or(&Value::Null);
    format!(
        "{}/{}",
        str_at(object, "/kind").to_lowercase(),
        str_at(object, "/name")
    )
}

pub fn event_message(event: &Value) -> &str {
    match str_at(event, "/message") {
        "" => str_at(event, "/note"),
        message => message,
    }
}

/// Whether an object of `group`/`kind` needs attention (toolbar "failing" count, sidebar).
pub fn is_failing(group: &str, kind: &str, object: &Value) -> bool {
    match (group, kind) {
        ("", "Pod") => pod_status(object).tone() == Tone::Bad,
        ("apps", "Deployment" | "StatefulSet" | "ReplicaSet") => {
            int_at(object, "/status/readyReplicas") < int_at(object, "/spec/replicas")
        }
        ("apps", "DaemonSet") => {
            int_at(object, "/status/numberReady") < int_at(object, "/status/desiredNumberScheduled")
        }
        ("batch", "Job") => job_status(object) == "Failed",
        ("", "Node") => !node_status(object).starts_with("Ready"),
        ("", "PersistentVolumeClaim") => str_at(object, "/status/phase") != "Bound",
        ("" | "events.k8s.io", "Event") => str_at(object, "/type") == "Warning",
        (crate::route::GROUP, crate::route::KIND) => Route::parse(object).is_rejected(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(spec: Value, status: Value) -> Value {
        json!({"metadata": {"name": "p"}, "spec": spec, "status": status})
    }

    #[test]
    fn running() {
        let p = pod(
            json!({"containers": [{"name": "a"}, {"name": "b"}]}),
            json!({"phase": "Running", "containerStatuses": [
                {"name": "a", "ready": true, "restartCount": 1, "state": {"running": {}}},
                {"name": "b", "ready": true, "restartCount": 0, "state": {"running": {}}}
            ]}),
        );
        let s = pod_status(&p);
        assert_eq!(s.reason, "Running");
        assert_eq!(s.ready_label(), "2/2");
        assert_eq!(s.restarts, 1);
        assert_eq!(s.tone(), Tone::Good);
        assert!(!s.is_degraded());
    }

    #[test]
    fn crash_loop() {
        let p = pod(
            json!({"containers": [{"name": "a"}, {"name": "b"}]}),
            json!({"phase": "Running", "containerStatuses": [
                {"name": "a", "ready": false, "restartCount": 14,
                 "state": {"waiting": {"reason": "CrashLoopBackOff"}},
                 "lastState": {"terminated": {"exitCode": 1, "finishedAt": "2026-09-24T10:00:00Z"}}},
                {"name": "b", "ready": true, "restartCount": 0, "state": {"running": {}}}
            ]}),
        );
        let s = pod_status(&p);
        assert_eq!(s.reason, "CrashLoopBackOff");
        assert_eq!(s.ready_label(), "1/2");
        assert_eq!(s.restarts, 14);
        assert!(s.last_restart.is_some());
        assert_eq!(s.tone(), Tone::Bad);
        assert!(s.is_degraded());
    }

    #[test]
    fn oom_killed_and_exit_codes() {
        let p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Running", "containerStatuses": [
                {"name": "a", "ready": false, "restartCount": 3,
                 "state": {"terminated": {"reason": "OOMKilled", "exitCode": 137}}}
            ]}),
        );
        assert_eq!(pod_status(&p).reason, "OOMKilled");
        let p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Running", "containerStatuses": [
                {"name": "a", "ready": false, "restartCount": 0,
                 "state": {"terminated": {"exitCode": 2}}}
            ]}),
        );
        assert_eq!(pod_status(&p).reason, "ExitCode:2");
    }

    #[test]
    fn init_containers() {
        let spec = json!({"initContainers": [{"name": "i1"}, {"name": "i2"}], "containers": [{"name": "a"}]});
        let p = pod(
            spec.clone(),
            json!({"phase": "Pending", "initContainerStatuses": [
                {"name": "i1", "restartCount": 0, "state": {"terminated": {"exitCode": 0}}},
                {"name": "i2", "restartCount": 0, "state": {"running": {}}}
            ]}),
        );
        assert_eq!(pod_status(&p).reason, "Init:1/2");
        assert_eq!(pod_status(&p).tone(), Tone::Info);
        let p = pod(
            spec.clone(),
            json!({"phase": "Pending", "initContainerStatuses": [
                {"name": "i1", "restartCount": 2, "state": {"terminated": {"exitCode": 1, "reason": "Error"}}}
            ]}),
        );
        assert_eq!(pod_status(&p).reason, "Init:Error");
        assert_eq!(pod_status(&p).tone(), Tone::Bad);
        let p = pod(
            spec,
            json!({"phase": "Pending", "initContainerStatuses": [
                {"name": "i1", "restartCount": 0, "state": {"waiting": {"reason": "ImagePullBackOff"}}}
            ]}),
        );
        assert_eq!(pod_status(&p).reason, "Init:ImagePullBackOff");
    }

    #[test]
    fn sidecars_count_towards_ready() {
        let p = pod(
            json!({"initContainers": [{"name": "proxy", "restartPolicy": "Always"}], "containers": [{"name": "a"}]}),
            json!({"phase": "Running",
            "conditions": [{"type": "Initialized", "status": "True"}],
            "initContainerStatuses": [
                {"name": "proxy", "restartCount": 1, "started": true, "ready": true, "state": {"running": {}}}
            ],
            "containerStatuses": [
                {"name": "a", "ready": true, "restartCount": 0, "state": {"running": {}}}
            ]}),
        );
        let s = pod_status(&p);
        assert_eq!(s.ready_label(), "2/2");
        assert_eq!(s.restarts, 1);
        assert_eq!(s.reason, "Running");
    }

    #[test]
    fn terminating_and_completed() {
        let mut p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Running", "containerStatuses": [
                {"name": "a", "ready": true, "restartCount": 0, "state": {"running": {}}}
            ]}),
        );
        p["metadata"]["deletionTimestamp"] = json!("2026-09-24T10:00:00Z");
        assert_eq!(pod_status(&p).reason, "Terminating");

        let p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Succeeded", "containerStatuses": [
                {"name": "a", "ready": false, "restartCount": 0,
                 "state": {"terminated": {"reason": "Completed", "exitCode": 0}}}
            ]}),
        );
        let s = pod_status(&p);
        assert_eq!(s.reason, "Completed");
        assert_eq!(s.tone(), Tone::Muted);
        assert!(!s.is_degraded());
    }

    #[test]
    fn pending_and_gated() {
        let p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Pending"}),
        );
        assert_eq!(pod_status(&p).reason, "Pending");
        assert_eq!(pod_status(&p).tone(), Tone::Warning);
        let p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Pending", "conditions": [{"type": "PodScheduled", "status": "False", "reason": "SchedulingGated"}]}),
        );
        assert_eq!(pod_status(&p).reason, "SchedulingGated");
        let p = pod(
            json!({"containers": [{"name": "a"}]}),
            json!({"phase": "Pending", "containerStatuses": [
                {"name": "a", "ready": false, "restartCount": 0, "state": {"waiting": {"reason": "ContainerCreating"}}}
            ]}),
        );
        assert_eq!(pod_status(&p).reason, "ContainerCreating");
        assert_eq!(pod_status(&p).tone(), Tone::Info);
    }
}
