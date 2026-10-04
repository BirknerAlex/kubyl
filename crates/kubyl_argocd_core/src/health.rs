//! Health of live objects, ported from Argo CD's built-in checks (gitops-engine
//! `pkg/health`): Deployments, StatefulSets, DaemonSets, ReplicaSets, Pods, Services, Ingresses,
//! PVCs, Jobs, HPAs and APIServices. Argo CD 3.x keeps per-resource health out of the
//! Application's status by default, so Kubernetes mode derives it from Kubyl's watch caches.
//! Kinds without a check have no health (like in Argo CD).

use serde_json::Value;

use crate::model::Health;

/// Health and an optional explanation.
pub type Assessment = (Health, Option<String>);

fn int(object: &Value, pointer: &str) -> i64 {
    object.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

fn opt_int(object: &Value, pointer: &str) -> Option<i64> {
    object.pointer(pointer).and_then(Value::as_i64)
}

fn text<'a>(object: &'a Value, pointer: &str) -> &'a str {
    object
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn message(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn conditions(object: &Value) -> &[Value] {
    object
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn condition<'a>(object: &'a Value, kind: &str) -> Option<&'a Value> {
    conditions(object)
        .iter()
        .find(|c| c.get("type").and_then(Value::as_str) == Some(kind))
}

/// The health of a live object of `group`/`kind`, or `None` for kinds Argo CD doesn't assess.
pub fn assess(group: &str, kind: &str, object: &Value) -> Option<Assessment> {
    if object.pointer("/metadata/deletionTimestamp").is_some()
        && !object
            .pointer("/metadata/finalizers")
            .and_then(Value::as_array)
            .is_some_and(|f| {
                f.iter()
                    .any(|f| f.as_str() == Some("argocd.argoproj.io/hook-finalizer"))
            })
    {
        return Some((Health::Progressing, Some("Pending deletion".into())));
    }
    match (group, kind) {
        ("apps", "Deployment") => Some(deployment(object)),
        ("apps", "StatefulSet") => Some(stateful_set(object)),
        ("apps", "DaemonSet") => Some(daemon_set(object)),
        ("apps", "ReplicaSet") => Some(replica_set(object)),
        ("", "Pod") => Some(pod(object)),
        ("", "Service") => Some(service(object)),
        ("", "PersistentVolumeClaim") => Some(pvc(object)),
        ("networking.k8s.io" | "extensions", "Ingress") => Some(ingress(object)),
        ("batch", "Job") => Some(job(object)),
        ("autoscaling", "HorizontalPodAutoscaler") => Some(hpa(object)),
        ("apiregistration.k8s.io", "APIService") => Some(api_service(object)),
        _ => None,
    }
}

fn deployment(d: &Value) -> Assessment {
    if d.pointer("/spec/paused").and_then(Value::as_bool) == Some(true) {
        return (Health::Suspended, Some("Deployment is paused".into()));
    }
    if int(d, "/metadata/generation") > int(d, "/status/observedGeneration") {
        return (
            Health::Progressing,
            Some("Waiting for rollout to finish: observed deployment generation less than desired generation".into()),
        );
    }
    let name = text(d, "/metadata/name");
    let updated = int(d, "/status/updatedReplicas");
    let replicas = int(d, "/status/replicas");
    let available = int(d, "/status/availableReplicas");
    let progressing = condition(d, "Progressing");
    if progressing
        .and_then(|c| c.get("reason"))
        .and_then(Value::as_str)
        == Some("ProgressDeadlineExceeded")
    {
        return (
            Health::Degraded,
            Some(format!(
                "Deployment {name:?} exceeded its progress deadline"
            )),
        );
    }
    if let Some(desired) = opt_int(d, "/spec/replicas")
        && updated < desired
    {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for rollout to finish: {updated} out of {desired} new replicas have been updated..."
            )),
        );
    }
    if replicas > updated {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for rollout to finish: {} old replicas are pending termination...",
                replicas - updated
            )),
        );
    }
    if available < updated {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for rollout to finish: {available} of {updated} updated replicas are available..."
            )),
        );
    }
    (Health::Healthy, None)
}

fn stateful_set(s: &Value) -> Assessment {
    let observed = int(s, "/status/observedGeneration");
    if observed == 0 || int(s, "/metadata/generation") > observed {
        return (
            Health::Progressing,
            Some("Waiting for statefulset spec update to be observed...".into()),
        );
    }
    let ready = int(s, "/status/readyReplicas");
    let updated = int(s, "/status/updatedReplicas");
    let desired = opt_int(s, "/spec/replicas");
    if let Some(desired) = desired
        && ready < desired
    {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for {} pods to be ready...",
                desired - ready
            )),
        );
    }
    let strategy = text(s, "/spec/updateStrategy/type");
    if strategy == "RollingUpdate" && s.pointer("/spec/updateStrategy/rollingUpdate").is_some() {
        if let (Some(desired), Some(partition)) = (
            desired,
            opt_int(s, "/spec/updateStrategy/rollingUpdate/partition"),
        ) && updated < desired - partition
        {
            return (
                Health::Progressing,
                Some(format!(
                    "Waiting for partitioned roll out to finish: {updated} out of {} new pods have been updated...",
                    desired - partition
                )),
            );
        }
        return (
            Health::Healthy,
            Some(format!(
                "partitioned roll out complete: {updated} new pods have been updated..."
            )),
        );
    }
    if strategy == "OnDelete" {
        return (
            Health::Healthy,
            Some(format!("statefulset has {ready} ready pods")),
        );
    }
    let update = text(s, "/status/updateRevision");
    let current = text(s, "/status/currentRevision");
    if update != current {
        return (
            Health::Progressing,
            Some(format!(
                "waiting for statefulset rolling update to complete {updated} pods at revision {update}..."
            )),
        );
    }
    (Health::Healthy, None)
}

fn daemon_set(d: &Value) -> Assessment {
    if int(d, "/metadata/generation") > int(d, "/status/observedGeneration") {
        return (
            Health::Progressing,
            Some("Waiting for rollout to finish: observed daemon set generation less than desired generation".into()),
        );
    }
    let updated = int(d, "/status/updatedNumberScheduled");
    let desired = int(d, "/status/desiredNumberScheduled");
    let available = int(d, "/status/numberAvailable");
    if text(d, "/spec/updateStrategy/type") == "OnDelete" {
        return (
            Health::Healthy,
            Some(format!(
                "daemon set {updated} out of {desired} new pods have been updated"
            )),
        );
    }
    let name = text(d, "/metadata/name");
    if updated < desired {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for daemon set {name:?} rollout to finish: {updated} out of {desired} new pods have been updated..."
            )),
        );
    }
    if available < desired {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for daemon set {name:?} rollout to finish: {available} of {desired} updated pods are available..."
            )),
        );
    }
    (Health::Healthy, None)
}

fn replica_set(r: &Value) -> Assessment {
    if int(r, "/metadata/generation") > int(r, "/status/observedGeneration") {
        return (
            Health::Progressing,
            Some("Waiting for rollout to finish: observed replica set generation less than desired generation".into()),
        );
    }
    if let Some(failure) = condition(r, "ReplicaFailure")
        && failure.get("status").and_then(Value::as_str) == Some("True")
    {
        return (
            Health::Degraded,
            failure
                .get("message")
                .and_then(Value::as_str)
                .map(String::from),
        );
    }
    let available = int(r, "/status/availableReplicas");
    if let Some(desired) = opt_int(r, "/spec/replicas")
        && available < desired
    {
        return (
            Health::Progressing,
            Some(format!(
                "Waiting for rollout to finish: {available} out of {desired} new replicas are available..."
            )),
        );
    }
    (Health::Healthy, None)
}

fn pod(p: &Value) -> Assessment {
    let status_message = message(text(p, "/status/message").to_string());
    let restart_policy = match text(p, "/spec/restartPolicy") {
        "" => "Always",
        other => other,
    };
    let statuses = |pointer: &str| -> Vec<Value> {
        p.pointer(pointer)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let containers = statuses("/status/containerStatuses");
    if restart_policy == "Always" {
        let messages: Vec<String> = containers
            .iter()
            .filter_map(|c| c.pointer("/state/waiting"))
            .filter(|w| {
                let reason = w.get("reason").and_then(Value::as_str).unwrap_or_default();
                reason.starts_with("Err")
                    || reason.ends_with("Error")
                    || reason.ends_with("BackOff")
            })
            .map(|w| {
                w.get("message")
                    .and_then(Value::as_str)
                    .or_else(|| w.get("reason").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        if !messages.is_empty() {
            return (Health::Degraded, message(messages.join(", ")));
        }
    }
    match text(p, "/status/phase") {
        "Pending" => (Health::Progressing, status_message),
        "Succeeded" => (Health::Healthy, status_message),
        "Failed" => {
            if status_message.is_some() {
                return (Health::Degraded, status_message);
            }
            let mut all = statuses("/status/initContainerStatuses");
            all.extend(containers);
            for container in &all {
                let Some(terminated) = container.pointer("/state/terminated") else {
                    continue;
                };
                let msg = terminated
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !msg.is_empty() {
                    return (Health::Degraded, Some(msg.into()));
                }
                if terminated.get("reason").and_then(Value::as_str) == Some("OOMKilled") {
                    return (Health::Degraded, Some("OOMKilled".into()));
                }
                let code = terminated
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if code != 0 {
                    let name = container.get("name").and_then(Value::as_str).unwrap_or("");
                    return (
                        Health::Degraded,
                        Some(format!("container {name:?} failed with exit code {code}")),
                    );
                }
            }
            (Health::Degraded, None)
        }
        "Running" => match restart_policy {
            "Always" => {
                let ready = condition(p, "Ready")
                    .and_then(|c| c.get("status"))
                    .and_then(Value::as_str)
                    == Some("True");
                if ready {
                    return (Health::Healthy, status_message);
                }
                if containers
                    .iter()
                    .any(|c| c.pointer("/lastState/terminated").is_some())
                {
                    return (Health::Degraded, status_message);
                }
                (Health::Progressing, status_message)
            }
            // Pods that run to completion are usually hooks: progressing, not healthy.
            _ => (Health::Progressing, status_message),
        },
        _ => (Health::Unknown, status_message),
    }
}

fn service(s: &Value) -> Assessment {
    if text(s, "/spec/type") == "LoadBalancer" {
        let has_ingress = s
            .pointer("/status/loadBalancer/ingress")
            .and_then(Value::as_array)
            .is_some_and(|i| !i.is_empty());
        if !has_ingress {
            return (Health::Progressing, None);
        }
    }
    (Health::Healthy, None)
}

fn ingress(i: &Value) -> Assessment {
    let has_ingress = i
        .pointer("/status/loadBalancer/ingress")
        .and_then(Value::as_array)
        .is_some_and(|i| !i.is_empty());
    if has_ingress {
        (Health::Healthy, None)
    } else {
        (Health::Progressing, None)
    }
}

fn pvc(p: &Value) -> Assessment {
    let health = match text(p, "/status/phase") {
        "Lost" => Health::Degraded,
        "Pending" => Health::Progressing,
        "Bound" => Health::Healthy,
        _ => Health::Unknown,
    };
    (health, None)
}

fn job(j: &Value) -> Assessment {
    let (mut failed, mut complete, mut suspended) = (false, false, false);
    let (mut fail_message, mut msg) = (String::new(), String::new());
    for c in conditions(j) {
        let text = c
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match c.get("type").and_then(Value::as_str) {
            Some("Failed") => {
                failed = true;
                complete = true;
                fail_message = text;
            }
            Some("Complete") => {
                complete = true;
                msg = text;
            }
            Some("Suspended") => {
                complete = true;
                msg = text;
                if c.get("status").and_then(Value::as_str) == Some("True") {
                    suspended = true;
                }
            }
            _ => {}
        }
    }
    if !complete {
        (Health::Progressing, message(msg))
    } else if failed {
        (Health::Degraded, message(fail_message))
    } else if suspended {
        (Health::Suspended, message(fail_message))
    } else {
        (Health::Healthy, message(msg))
    }
}

fn hpa(h: &Value) -> Assessment {
    const DEGRADED: &[(&str, &str)] = &[
        ("AbleToScale", "FailedGetScale"),
        ("AbleToScale", "FailedUpdateScale"),
        ("ScalingActive", "FailedGetResourceMetric"),
        ("ScalingActive", "FailedGetObjectMetric"),
        ("ScalingActive", "FailedGetPodsMetric"),
        ("ScalingActive", "FailedGetExternalMetric"),
        ("ScalingActive", "FailedComputeMetricsReplicas"),
        ("ScalingActive", "InvalidSelector"),
    ];
    let conds = conditions(h);
    let field = |c: &Value, k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    for c in conds {
        if DEGRADED
            .iter()
            .any(|(t, r)| field(c, "type") == *t && field(c, "reason") == *r)
        {
            return (Health::Degraded, message(field(c, "message")));
        }
    }
    for c in conds {
        if matches!(field(c, "type").as_str(), "AbleToScale" | "ScalingLimited")
            && field(c, "status") == "True"
        {
            return (Health::Healthy, message(field(c, "message")));
        }
    }
    (Health::Progressing, Some("Waiting to Autoscale".into()))
}

fn api_service(a: &Value) -> Assessment {
    match condition(a, "Available") {
        Some(c) if c.get("status").and_then(Value::as_str) == Some("True") => {
            (Health::Healthy, None)
        }
        Some(c) => (
            Health::Progressing,
            c.get("message").and_then(Value::as_str).map(String::from),
        ),
        None => (Health::Progressing, None),
    }
}

/// The worst of several healths (Argo CD's order), `None` for none.
pub fn worst(healths: impl IntoIterator<Item = Health>) -> Option<Health> {
    healths.into_iter().max_by_key(|h| h.severity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn health(group: &str, kind: &str, object: Value) -> Health {
        assess(group, kind, &object).unwrap().0
    }

    #[test]
    fn deployments() {
        let ready = json!({"metadata": {"name": "web", "generation": 2},
            "spec": {"replicas": 3},
            "status": {"observedGeneration": 2, "replicas": 3, "updatedReplicas": 3, "availableReplicas": 3}});
        assert_eq!(health("apps", "Deployment", ready.clone()), Health::Healthy);
        let mut rolling = ready.clone();
        rolling["status"]["updatedReplicas"] = json!(1);
        let (h, m) = assess("apps", "Deployment", &rolling).unwrap();
        assert_eq!(h, Health::Progressing);
        assert!(m.unwrap().contains("1 out of 3"));
        let mut stuck = ready.clone();
        stuck["status"]["conditions"] = json!([{"type": "Progressing", "status": "False", "reason": "ProgressDeadlineExceeded"}]);
        assert_eq!(health("apps", "Deployment", stuck), Health::Degraded);
        let mut paused = ready.clone();
        paused["spec"]["paused"] = json!(true);
        assert_eq!(health("apps", "Deployment", paused), Health::Suspended);
        let mut new_generation = ready;
        new_generation["metadata"]["generation"] = json!(3);
        assert_eq!(
            health("apps", "Deployment", new_generation),
            Health::Progressing
        );
    }

    #[test]
    fn pods() {
        let running = json!({"spec": {"restartPolicy": "Always"}, "status": {"phase": "Running",
            "conditions": [{"type": "Ready", "status": "True"}],
            "containerStatuses": [{"name": "app", "state": {"running": {}}}]}});
        assert_eq!(health("", "Pod", running), Health::Healthy);
        let crashing = json!({"spec": {}, "status": {"phase": "Running",
            "containerStatuses": [{"name": "app", "state": {"waiting": {"reason": "CrashLoopBackOff", "message": "back-off 5m"}}}]}});
        let (h, m) = assess("", "Pod", &crashing).unwrap();
        assert_eq!((h, m.as_deref()), (Health::Degraded, Some("back-off 5m")));
        let pull = json!({"spec": {}, "status": {"phase": "Pending",
            "containerStatuses": [{"name": "app", "state": {"waiting": {"reason": "ImagePullBackOff"}}}]}});
        assert_eq!(health("", "Pod", pull), Health::Degraded);
        let pending = json!({"spec": {}, "status": {"phase": "Pending"}});
        assert_eq!(health("", "Pod", pending), Health::Progressing);
        let oom = json!({"spec": {"restartPolicy": "Never"}, "status": {"phase": "Failed",
            "containerStatuses": [{"name": "app", "state": {"terminated": {"reason": "OOMKilled", "exitCode": 137}}}]}});
        let (h, m) = assess("", "Pod", &oom).unwrap();
        assert_eq!((h, m.as_deref()), (Health::Degraded, Some("OOMKilled")));
        let hook = json!({"spec": {"restartPolicy": "Never"}, "status": {"phase": "Running"}});
        assert_eq!(health("", "Pod", hook), Health::Progressing);
        let deleting = json!({"metadata": {"deletionTimestamp": "2026-01-01T00:00:00Z"},
            "spec": {}, "status": {"phase": "Running"}});
        assert_eq!(health("", "Pod", deleting), Health::Progressing);
    }

    #[test]
    fn other_kinds() {
        let sts = json!({"metadata": {"generation": 1}, "spec": {"replicas": 2, "updateStrategy": {"type": "RollingUpdate"}},
            "status": {"observedGeneration": 1, "readyReplicas": 2, "currentRevision": "a", "updateRevision": "a"}});
        assert_eq!(health("apps", "StatefulSet", sts.clone()), Health::Healthy);
        let mut updating = sts;
        updating["status"]["updateRevision"] = json!("b");
        assert_eq!(health("apps", "StatefulSet", updating), Health::Progressing);
        let ds = json!({"metadata": {"generation": 1, "name": "agent"}, "spec": {}, "status": {"observedGeneration": 1,
            "desiredNumberScheduled": 2, "updatedNumberScheduled": 2, "numberAvailable": 1}});
        assert_eq!(health("apps", "DaemonSet", ds), Health::Progressing);
        let lb = json!({"spec": {"type": "LoadBalancer"}, "status": {"loadBalancer": {}}});
        assert_eq!(health("", "Service", lb), Health::Progressing);
        assert_eq!(
            health("", "Service", json!({"spec": {"type": "ClusterIP"}})),
            Health::Healthy
        );
        assert_eq!(
            health(
                "",
                "PersistentVolumeClaim",
                json!({"status": {"phase": "Bound"}})
            ),
            Health::Healthy
        );
        let failed_job = json!({"status": {"conditions": [{"type": "Failed", "status": "True", "message": "BackoffLimitExceeded"}]}});
        assert_eq!(health("batch", "Job", failed_job), Health::Degraded);
        assert_eq!(
            health("batch", "Job", json!({"status": {}})),
            Health::Progressing
        );
        let hpa = json!({"status": {"conditions": [{"type": "AbleToScale", "status": "True"}]}});
        assert_eq!(
            health("autoscaling", "HorizontalPodAutoscaler", hpa),
            Health::Healthy
        );
        let bad_hpa = json!({"status": {"conditions": [{"type": "ScalingActive", "status": "False", "reason": "FailedGetResourceMetric"}]}});
        assert_eq!(
            health("autoscaling", "HorizontalPodAutoscaler", bad_hpa),
            Health::Degraded
        );
        assert!(assess("", "ConfigMap", &json!({})).is_none());
        assert!(assess("argoproj.io", "Application", &json!({})).is_none());
        assert_eq!(
            worst([Health::Healthy, Health::Degraded, Health::Progressing]),
            Some(Health::Degraded)
        );
    }
}
