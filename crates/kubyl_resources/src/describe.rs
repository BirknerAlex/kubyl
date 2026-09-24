//! A `kubectl describe`-like text rendering for any object, with related events.
//!
//! Pods, Deployments, Nodes, Services and Secrets get tailored sections; other kinds print
//! their `spec` and `status` as nested fields, like kubectl's generic describer. Secret values
//! are never printed, only their sizes.

use std::fmt::Write as _;

use jiff::Timestamp;
use serde_json::Value;

use crate::columns::{event_message, event_time, node_roles, node_status, pod_status};
use crate::format::{array_at, human_duration, int_at, map_pairs, seconds_since, str_at};

const KEY_WIDTH: usize = 20;

struct Out {
    text: String,
}

impl Out {
    fn field(&mut self, indent: usize, key: &str, value: impl AsRef<str>) {
        let key = format!("{key}:");
        let pad = KEY_WIDTH.saturating_sub(indent + key.len()).max(1);
        writeln!(
            self.text,
            "{:indent$}{key}{:pad$}{}",
            "",
            "",
            value.as_ref(),
            indent = indent,
            pad = pad
        )
        .ok();
    }

    fn line(&mut self, indent: usize, text: impl AsRef<str>) {
        writeln!(
            self.text,
            "{:indent$}{}",
            "",
            text.as_ref(),
            indent = indent
        )
        .ok();
    }

    /// `Labels: a=b` then continuation lines aligned under the first value.
    fn list(&mut self, indent: usize, key: &str, items: &[String]) {
        match items.split_first() {
            None => self.field(indent, key, "<none>"),
            Some((first, rest)) => {
                self.field(indent, key, first);
                for item in rest {
                    self.line(KEY_WIDTH.max(indent + key.len() + 2), item);
                }
            }
        }
    }
}

fn or_none(value: &str) -> &str {
    if value.is_empty() { "<none>" } else { value }
}

fn title_case(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Renders `object` (of `kind`) and its `events` like `kubectl describe`.
pub fn describe(kind: &str, object: &Value, events: &[Value], now: Timestamp) -> String {
    let mut out = Out {
        text: String::new(),
    };
    let meta = &object["metadata"];
    out.field(0, "Name", str_at(meta, "/name"));
    if let Some(ns) = meta["namespace"].as_str() {
        out.field(0, "Namespace", ns);
    }
    out.list(0, "Labels", &map_pairs(meta.get("labels")));
    let annotations: Vec<String> = map_pairs(meta.get("annotations"))
        .into_iter()
        .filter(|a| !a.starts_with("kubectl.kubernetes.io/last-applied-configuration="))
        .map(|a| {
            if a.chars().count() > 120 {
                format!("{}…", a.chars().take(120).collect::<String>())
            } else {
                a
            }
        })
        .collect();
    out.list(0, "Annotations", &annotations);
    if let Some(created) = meta["creationTimestamp"].as_str() {
        out.field(0, "CreationTimestamp", created);
    }
    let owners: Vec<String> = array_at(meta, "/ownerReferences")
        .iter()
        .map(|o| format!("{}/{}", str_at(o, "/kind"), str_at(o, "/name")))
        .collect();
    if !owners.is_empty() {
        out.field(0, "Controlled By", owners.join(", "));
    }

    match kind {
        "Pod" => describe_pod(&mut out, object, now),
        "Deployment" => describe_deployment(&mut out, object),
        "Node" => describe_node(&mut out, object),
        "Service" => describe_service(&mut out, object),
        "Secret" => describe_secret(&mut out, object),
        "ConfigMap" => {
            out.line(0, "");
            out.line(0, "Data");
            out.line(0, "====");
            if let Some(data) = object["data"].as_object() {
                for (key, value) in data {
                    out.line(0, format!("{key}:"));
                    out.line(0, "----");
                    out.line(0, value.as_str().unwrap_or_default());
                    out.line(0, "");
                }
            }
        }
        _ => {
            for section in ["spec", "status"] {
                if let Some(value) = object.get(section).filter(|v| !v.is_null()) {
                    out.line(0, format!("{}:", title_case(section)));
                    generic(&mut out, 2, value);
                }
            }
        }
    }

    describe_events(&mut out, events, now);
    out.text
}

fn generic(out: &mut Out, indent: usize, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                match value {
                    Value::Object(inner) if inner.is_empty() => {
                        out.field(indent, &title_case(key), "<none>")
                    }
                    Value::Object(_) => {
                        out.line(indent, format!("{}:", title_case(key)));
                        generic(out, indent + 2, value);
                    }
                    Value::Array(items)
                        if items.iter().all(|i| !i.is_object() && !i.is_array()) =>
                    {
                        let joined: Vec<String> = items.iter().map(scalar).collect();
                        out.field(indent, &title_case(key), or_none(&joined.join(", ")));
                    }
                    Value::Array(items) => {
                        out.line(indent, format!("{}:", title_case(key)));
                        for item in items {
                            generic(out, indent + 2, item);
                            out.line(0, "");
                        }
                    }
                    other => out.field(indent, &title_case(key), scalar(other)),
                }
            }
        }
        other => out.line(indent, scalar(other)),
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "<nil>".into(),
        other => other.to_string(),
    }
}

fn describe_pod(out: &mut Out, pod: &Value, now: Timestamp) {
    let status = pod_status(pod);
    out.field(0, "Priority", int_at(pod, "/spec/priority").to_string());
    out.field(
        0,
        "Service Account",
        or_none(str_at(pod, "/spec/serviceAccountName")),
    );
    let node = match (str_at(pod, "/spec/nodeName"), str_at(pod, "/status/hostIP")) {
        ("", _) => "<none>".to_string(),
        (node, "") => node.to_string(),
        (node, ip) => format!("{node}/{ip}"),
    };
    out.field(0, "Node", node);
    if let Some(start) = pod.pointer("/status/startTime").and_then(Value::as_str) {
        out.field(0, "Start Time", start);
    }
    out.field(0, "Status", &status.reason);
    out.field(0, "IP", or_none(str_at(pod, "/status/podIP")));
    out.field(0, "QoS Class", or_none(str_at(pod, "/status/qosClass")));
    for (title, pointer, statuses) in [
        (
            "Init Containers",
            "/spec/initContainers",
            "/status/initContainerStatuses",
        ),
        (
            "Containers",
            "/spec/containers",
            "/status/containerStatuses",
        ),
    ] {
        let containers = array_at(pod, pointer);
        if containers.is_empty() {
            continue;
        }
        out.line(0, format!("{title}:"));
        for container in containers {
            let name = str_at(container, "/name");
            let state = array_at(pod, statuses)
                .iter()
                .find(|s| str_at(s, "/name") == name);
            out.line(2, format!("{name}:"));
            out.field(4, "Image", str_at(container, "/image"));
            let ports: Vec<String> = array_at(container, "/ports")
                .iter()
                .map(|p| {
                    let protocol = match str_at(p, "/protocol") {
                        "" => "TCP",
                        protocol => protocol,
                    };
                    match str_at(p, "/name") {
                        "" => format!("{}/{protocol}", int_at(p, "/containerPort")),
                        name => format!("{}/{protocol} ({name})", int_at(p, "/containerPort")),
                    }
                })
                .collect();
            out.field(4, "Ports", or_none(&ports.join(", ")));
            if let Some(state) = state {
                let (label, detail) = container_state(&state["state"], now);
                out.field(4, "State", label);
                for line in detail {
                    out.line(6, line);
                }
                if let Some(last) = state.pointer("/lastState/terminated") {
                    out.field(4, "Last State", "Terminated");
                    out.field(6, "Reason", or_none(str_at(last, "/reason")));
                    out.field(6, "Exit Code", int_at(last, "/exitCode").to_string());
                }
                out.field(
                    4,
                    "Ready",
                    state["ready"].as_bool().unwrap_or(false).to_string(),
                );
                out.field(
                    4,
                    "Restart Count",
                    int_at(state, "/restartCount").to_string(),
                );
            }
            for kind in ["limits", "requests"] {
                let pairs: Vec<String> = container
                    .pointer(&format!("/resources/{kind}"))
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{k}: {}", scalar(v)))
                            .collect()
                    })
                    .unwrap_or_default();
                if !pairs.is_empty() {
                    out.list(4, &title_case(kind), &pairs);
                }
            }
            for probe in ["livenessProbe", "readinessProbe", "startupProbe"] {
                if let Some(p) = container.get(probe) {
                    out.field(
                        4,
                        &title_case(&probe.replace("Probe", "")),
                        probe_summary(p),
                    );
                }
            }
        }
    }
    let conditions: Vec<String> = array_at(pod, "/status/conditions")
        .iter()
        .map(|c| format!("{:<26}{}", str_at(c, "/type"), str_at(c, "/status")))
        .collect();
    if !conditions.is_empty() {
        out.line(0, "Conditions:");
        out.line(2, format!("{:<26}Status", "Type"));
        for condition in conditions {
            out.line(2, condition);
        }
    }
    let volumes: Vec<String> = array_at(pod, "/spec/volumes")
        .iter()
        .map(|v| {
            let kind = v
                .as_object()
                .and_then(|m| m.keys().find(|k| *k != "name").cloned())
                .unwrap_or_default();
            format!("{} ({kind})", str_at(v, "/name"))
        })
        .collect();
    out.list(0, "Volumes", &volumes);
    out.list(
        0,
        "Node-Selectors",
        &map_pairs(pod.pointer("/spec/nodeSelector")),
    );
    let tolerations: Vec<String> = array_at(pod, "/spec/tolerations")
        .iter()
        .map(|t| {
            let mut s = str_at(t, "/key").to_string();
            if let Some(value) = t["value"].as_str() {
                s.push('=');
                s.push_str(value);
            }
            if let Some(effect) = t["effect"].as_str() {
                s.push(':');
                s.push_str(effect);
            }
            if let Some(seconds) = t["tolerationSeconds"].as_i64() {
                s.push_str(&format!(" for {seconds}s"));
            }
            s
        })
        .collect();
    out.list(0, "Tolerations", &tolerations);
}

fn container_state(state: &Value, now: Timestamp) -> (String, Vec<String>) {
    if let Some(running) = state.get("running") {
        let since = str_at(running, "/startedAt");
        return ("Running".into(), vec![format!("Started:   {since}")]);
    }
    if let Some(waiting) = state.get("waiting") {
        let mut detail = vec![format!("Reason:    {}", str_at(waiting, "/reason"))];
        if let Some(message) = waiting["message"].as_str() {
            detail.push(format!("Message:   {message}"));
        }
        return ("Waiting".into(), detail);
    }
    if let Some(terminated) = state.get("terminated") {
        let finished = crate::format::timestamp(str_at(terminated, "/finishedAt"))
            .map(|t| format!(" ({} ago)", human_duration(seconds_since(t, now))))
            .unwrap_or_default();
        return (
            "Terminated".into(),
            vec![
                format!("Reason:    {}", or_none(str_at(terminated, "/reason"))),
                format!("Exit Code: {}{finished}", int_at(terminated, "/exitCode")),
            ],
        );
    }
    ("<unknown>".into(), Vec::new())
}

fn probe_summary(probe: &Value) -> String {
    let action = if let Some(http) = probe.get("httpGet") {
        format!(
            "http-get {}://:{}{}",
            match str_at(http, "/scheme") {
                "" => "http".to_string(),
                s => s.to_lowercase(),
            },
            scalar(&http["port"]),
            str_at(http, "/path")
        )
    } else if let Some(tcp) = probe.get("tcpSocket") {
        format!("tcp-socket :{}", scalar(&tcp["port"]))
    } else if let Some(exec) = probe.get("exec") {
        let command: Vec<String> = array_at(exec, "/command").iter().map(scalar).collect();
        format!("exec [{}]", command.join(" "))
    } else if probe.get("grpc").is_some() {
        format!("grpc :{}", scalar(&probe["grpc"]["port"]))
    } else {
        "<unknown>".into()
    };
    let n = |key: &str, default: i64| probe[key].as_i64().unwrap_or(default);
    format!(
        "{action} delay={}s timeout={}s period={}s #success={} #failure={}",
        n("initialDelaySeconds", 0),
        n("timeoutSeconds", 1),
        n("periodSeconds", 10),
        n("successThreshold", 1),
        n("failureThreshold", 3)
    )
}

fn describe_deployment(out: &mut Out, d: &Value) {
    let selector = map_pairs(d.pointer("/spec/selector/matchLabels")).join(",");
    out.field(0, "Selector", or_none(&selector));
    out.field(
        0,
        "Replicas",
        format!(
            "{} desired | {} updated | {} total | {} available | {} unavailable",
            int_at(d, "/spec/replicas"),
            int_at(d, "/status/updatedReplicas"),
            int_at(d, "/status/replicas"),
            int_at(d, "/status/availableReplicas"),
            int_at(d, "/status/unavailableReplicas"),
        ),
    );
    let strategy = match str_at(d, "/spec/strategy/type") {
        "" => "RollingUpdate",
        s => s,
    };
    out.field(0, "StrategyType", strategy);
    if strategy == "RollingUpdate" {
        out.field(
            0,
            "RollingUpdateStrategy",
            format!(
                "{} max unavailable, {} max surge",
                scalar(
                    d.pointer("/spec/strategy/rollingUpdate/maxUnavailable")
                        .unwrap_or(&Value::from("25%"))
                ),
                scalar(
                    d.pointer("/spec/strategy/rollingUpdate/maxSurge")
                        .unwrap_or(&Value::from("25%"))
                ),
            ),
        );
    }
    if d.pointer("/spec/paused").and_then(Value::as_bool) == Some(true) {
        out.field(0, "Paused", "true");
    }
    out.line(0, "Pod Template:");
    out.list(
        2,
        "Labels",
        &map_pairs(d.pointer("/spec/template/metadata/labels")),
    );
    for container in array_at(d, "/spec/template/spec/containers") {
        out.line(2, format!("{}:", str_at(container, "/name")));
        out.field(4, "Image", str_at(container, "/image"));
    }
    let conditions: Vec<String> = array_at(d, "/status/conditions")
        .iter()
        .map(|c| {
            format!(
                "{:<16}{:<8}{}",
                str_at(c, "/type"),
                str_at(c, "/status"),
                str_at(c, "/reason")
            )
        })
        .collect();
    if !conditions.is_empty() {
        out.line(0, "Conditions:");
        out.line(2, format!("{:<16}{:<8}Reason", "Type", "Status"));
        for condition in conditions {
            out.line(2, condition);
        }
    }
}

fn describe_node(out: &mut Out, node: &Value) {
    out.field(0, "Roles", or_none(&node_roles(node).join(",")));
    out.field(0, "Status", node_status(node));
    let taints: Vec<String> = array_at(node, "/spec/taints")
        .iter()
        .map(|t| match t["value"].as_str() {
            Some(value) => format!("{}={value}:{}", str_at(t, "/key"), str_at(t, "/effect")),
            None => format!("{}:{}", str_at(t, "/key"), str_at(t, "/effect")),
        })
        .collect();
    out.list(0, "Taints", &taints);
    out.field(
        0,
        "Unschedulable",
        node.pointer("/spec/unschedulable")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .to_string(),
    );
    let conditions: Vec<String> = array_at(node, "/status/conditions")
        .iter()
        .map(|c| {
            format!(
                "{:<22}{:<8}{:<28}{}",
                str_at(c, "/type"),
                str_at(c, "/status"),
                str_at(c, "/reason"),
                str_at(c, "/message")
            )
        })
        .collect();
    out.line(0, "Conditions:");
    out.line(
        2,
        format!("{:<22}{:<8}{:<28}Message", "Type", "Status", "Reason"),
    );
    for condition in conditions {
        out.line(2, condition);
    }
    let addresses: Vec<String> = array_at(node, "/status/addresses")
        .iter()
        .map(|a| format!("{}: {}", str_at(a, "/type"), str_at(a, "/address")))
        .collect();
    out.list(0, "Addresses", &addresses);
    for (title, pointer) in [
        ("Capacity", "/status/capacity"),
        ("Allocatable", "/status/allocatable"),
    ] {
        out.line(0, format!("{title}:"));
        if let Some(map) = node.pointer(pointer).and_then(Value::as_object) {
            for (key, value) in map {
                out.field(2, key, scalar(value));
            }
        }
    }
    out.line(0, "System Info:");
    if let Some(info) = node.pointer("/status/nodeInfo").and_then(Value::as_object) {
        for (key, value) in info {
            out.field(2, &title_case(key), scalar(value));
        }
    }
}

fn describe_service(out: &mut Out, svc: &Value) {
    out.field(
        0,
        "Selector",
        or_none(&map_pairs(svc.pointer("/spec/selector")).join(",")),
    );
    out.field(0, "Type", str_at(svc, "/spec/type"));
    out.field(0, "IP", or_none(str_at(svc, "/spec/clusterIP")));
    for port in array_at(svc, "/spec/ports") {
        let protocol = match str_at(port, "/protocol") {
            "" => "TCP",
            p => p,
        };
        out.field(
            0,
            "Port",
            format!(
                "{} {}/{protocol}",
                or_none(str_at(port, "/name")),
                int_at(port, "/port")
            ),
        );
        out.field(0, "TargetPort", scalar(&port["targetPort"]));
        if let Some(node_port) = port["nodePort"].as_i64() {
            out.field(0, "NodePort", format!("{node_port}/{protocol}"));
        }
    }
    out.field(
        0,
        "Session Affinity",
        or_none(str_at(svc, "/spec/sessionAffinity")),
    );
}

/// Only key names and sizes: never the values.
fn describe_secret(out: &mut Out, secret: &Value) {
    out.field(0, "Type", str_at(secret, "/type"));
    out.line(0, "");
    out.line(0, "Data");
    out.line(0, "====");
    if let Some(data) = secret["data"].as_object() {
        for (key, value) in data {
            // Base64: 4 characters per 3 bytes.
            let encoded = value.as_str().unwrap_or_default();
            let padding = encoded.chars().rev().take_while(|c| *c == '=').count();
            let bytes = (encoded.len() / 4 * 3).saturating_sub(padding);
            out.line(0, format!("{key}:  {bytes} bytes"));
        }
    }
}

fn describe_events(out: &mut Out, events: &[Value], now: Timestamp) {
    if events.is_empty() {
        out.field(0, "Events", "<none>");
        return;
    }
    let mut events: Vec<&Value> = events.iter().collect();
    events.sort_by_key(|e| event_time(e));
    out.line(0, "Events:");
    out.line(
        2,
        format!(
            "{:<9}{:<22}{:<8}{:<22}Message",
            "Type", "Reason", "Age", "From"
        ),
    );
    out.line(
        2,
        format!(
            "{:<9}{:<22}{:<8}{:<22}-------",
            "----", "------", "----", "----"
        ),
    );
    for event in events {
        let age = event_time(event)
            .map(|t| human_duration(seconds_since(t, now)))
            .unwrap_or_default();
        let from = match str_at(event, "/source/component") {
            "" => str_at(event, "/reportingController"),
            component => component,
        };
        out.line(
            2,
            format!(
                "{:<9}{:<22}{:<8}{:<22}{}",
                str_at(event, "/type"),
                str_at(event, "/reason"),
                age,
                from,
                event_message(event).replace('\n', " ")
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn describes_pods_with_events() {
        let pod = json!({
            "metadata": {"name": "web-0", "namespace": "payments", "labels": {"app": "web"},
                         "ownerReferences": [{"kind": "StatefulSet", "name": "web"}]},
            "spec": {"nodeName": "node-1", "containers": [{"name": "app", "image": "nginx:1.27",
                     "ports": [{"containerPort": 80}],
                     "readinessProbe": {"httpGet": {"path": "/healthz", "port": 80}}}]},
            "status": {"phase": "Running", "podIP": "10.0.0.5", "containerStatuses": [
                {"name": "app", "ready": true, "restartCount": 2, "state": {"running": {"startedAt": "2026-09-24T09:00:00Z"}}}
            ], "conditions": [{"type": "Ready", "status": "True"}]}
        });
        let events = vec![
            json!({"type": "Normal", "reason": "Pulled", "message": "Pulled image",
            "source": {"component": "kubelet"}, "lastTimestamp": "2026-09-24T09:00:00Z"}),
        ];
        let text = describe(
            "Pod",
            &pod,
            &events,
            "2026-09-24T10:00:00Z".parse().unwrap(),
        );
        assert!(text.contains("Name:"));
        assert!(text.contains("web-0"));
        assert!(text.contains("Controlled By:      StatefulSet/web"));
        assert!(text.contains("Restart Count:"));
        assert!(text.contains("http-get http://:80/healthz"));
        assert!(text.contains("Pulled"));
        assert!(text.contains("60m"));
    }

    #[test]
    fn secrets_show_sizes_not_values() {
        let secret = json!({"metadata": {"name": "db"}, "type": "Opaque",
            "data": {"password": "aHVudGVyMg=="}});
        let text = describe("Secret", &secret, &[], Timestamp::now());
        assert!(text.contains("password:  7 bytes"));
        assert!(!text.contains("aHVudGVyMg"));
        assert!(!text.contains("hunter2"));
    }

    #[test]
    fn generic_kinds_print_spec_and_status() {
        let object = json!({"metadata": {"name": "api-tls"},
            "spec": {"secretName": "api-tls", "dnsNames": ["a.example", "b.example"]},
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}});
        let text = describe("Certificate", &object, &[], Timestamp::now());
        assert!(text.contains("Spec:"));
        assert!(text.contains("SecretName:"));
        assert!(text.contains("a.example, b.example"));
        assert!(text.contains("Events:"));
    }
}
