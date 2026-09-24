//! Hand-written column sets for the core kinds, matching `kubectl get` (and `-o wide`).
//!
//! Kinds without a provider use the server-side `Table` ([`crate::table`]), which carries CRD
//! printer columns. CPU and memory cells are [`CellValue::Empty`] here: list views fill them from
//! [`crate::metrics`] (phase 07 provides the data).

pub mod pods;

use gpui::App;
use jiff::Timestamp;
use kubyl_core::{CellValue, ColumnDef, ColumnProvider, ColumnWidth, ResourceColumns, Tone};
use serde_json::Value;

use crate::format::{
    array_at, human_duration, int_at, join_or_none, map_pairs, name, object_age, seconds_since,
    str_at, timestamp,
};
pub use pods::{PodStatus, pod_status, status_tone};

type CellFn = fn(&Value, &str, Timestamp) -> CellValue;
/// Group, kind, columns and cells of one registered kind.
type KindEntry = (&'static str, &'static str, fn() -> Vec<ColumnDef>, CellFn);

/// Columns of one kind: `name` and `age` cells are handled here, the rest by `cell`.
struct Kind {
    columns: fn() -> Vec<ColumnDef>,
    cell: CellFn,
}

impl ColumnProvider for Kind {
    fn columns(&self) -> Vec<ColumnDef> {
        (self.columns)()
    }

    fn cell(&self, object: &Value, column: &str) -> CellValue {
        let now = Timestamp::now();
        match column {
            "name" => match (self.cell)(object, column, now) {
                CellValue::Empty => text(name(object)),
                custom => custom,
            },
            "age" => muted(object_age(object, now)),
            _ => (self.cell)(object, column, now),
        }
    }
}

/// Registers the built-in column sets.
pub fn register(cx: &mut App) {
    let kinds: [KindEntry; 23] = [
        ("", "Pod", pod_columns, pod_cell),
        ("apps", "Deployment", deployment_columns, deployment_cell),
        ("apps", "StatefulSet", statefulset_columns, statefulset_cell),
        ("apps", "DaemonSet", daemonset_columns, daemonset_cell),
        ("apps", "ReplicaSet", replicaset_columns, replicaset_cell),
        ("batch", "Job", job_columns, job_cell),
        ("batch", "CronJob", cronjob_columns, cronjob_cell),
        ("", "Service", service_columns, service_cell),
        (
            "networking.k8s.io",
            "Ingress",
            ingress_columns,
            ingress_cell,
        ),
        ("", "ConfigMap", configmap_columns, configmap_cell),
        ("", "Secret", secret_columns, secret_cell),
        ("", "PersistentVolumeClaim", pvc_columns, pvc_cell),
        ("", "PersistentVolume", pv_columns, pv_cell),
        (
            "storage.k8s.io",
            "StorageClass",
            storageclass_columns,
            storageclass_cell,
        ),
        ("", "Node", node_columns, node_cell),
        ("", "Namespace", namespace_columns, namespace_cell),
        ("", "Event", event_columns, event_cell),
        ("events.k8s.io", "Event", event_columns, event_cell),
        (
            "",
            "ServiceAccount",
            serviceaccount_columns,
            serviceaccount_cell,
        ),
        ("rbac.authorization.k8s.io", "Role", role_columns, no_cell),
        (
            "rbac.authorization.k8s.io",
            "ClusterRole",
            role_columns,
            no_cell,
        ),
        (
            "rbac.authorization.k8s.io",
            "RoleBinding",
            binding_columns,
            binding_cell,
        ),
        (
            "rbac.authorization.k8s.io",
            "ClusterRoleBinding",
            binding_columns,
            binding_cell,
        ),
    ];
    for (group, kind, columns, cell) in kinds {
        ResourceColumns::register(cx, group, kind, Kind { columns, cell });
    }
}

// ----- Helpers -----

fn text(value: impl Into<String>) -> CellValue {
    let value = value.into();
    if value.is_empty() {
        CellValue::Empty
    } else {
        CellValue::Text(value.into())
    }
}

/// Secondary text (node names, ages, IPs) in the muted color.
fn muted(value: impl Into<String>) -> CellValue {
    CellValue::Tinted {
        label: value.into().into(),
        tone: Tone::Neutral,
    }
}

fn status(label: impl Into<String>, tone: Tone) -> CellValue {
    CellValue::Status {
        label: label.into().into(),
        tone,
    }
}

fn no_cell(_: &Value, _: &str, _: Timestamp) -> CellValue {
    CellValue::Empty
}

fn flex(id: &'static str, title: &'static str, min: f32) -> ColumnDef {
    ColumnDef::new(id, title, ColumnWidth::Flex { weight: 1.0, min })
}

fn fixed(id: &'static str, title: &'static str, width: f32) -> ColumnDef {
    ColumnDef::new(id, title, ColumnWidth::Fixed(width))
}

fn name_column() -> ColumnDef {
    flex("name", "Name", 180.0).mono()
}

fn age_column() -> ColumnDef {
    fixed("age", "Age", 54.0).mono()
}

fn ratio(a: i64, b: i64) -> CellValue {
    let label = format!("{a}/{b}");
    if a < b {
        CellValue::Tinted {
            label: label.into(),
            tone: Tone::Bad,
        }
    } else {
        text(label)
    }
}

fn number(value: i64) -> CellValue {
    text(value.to_string())
}

/// Container names and images of a pod template (`-o wide`).
fn template_containers(object: &Value, pointer: &str, images: bool) -> CellValue {
    let names: Vec<&str> = array_at(object, pointer)
        .iter()
        .map(|c| str_at(c, if images { "/image" } else { "/name" }))
        .collect();
    text(names.join(","))
}

fn selector(object: &Value) -> CellValue {
    let labels = map_pairs(object.pointer("/spec/selector/matchLabels"));
    let exprs: Vec<String> = array_at(object, "/spec/selector/matchExpressions")
        .iter()
        .map(|e| {
            let values: Vec<&str> = array_at(e, "/values")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let op = str_at(e, "/operator").to_lowercase();
            format!("{} {op} ({})", str_at(e, "/key"), values.join(","))
        })
        .collect();
    let all: Vec<String> = labels.into_iter().chain(exprs).collect();
    text(join_or_none(&all))
}

fn workload_wide(object: &Value, column: &str) -> CellValue {
    match column {
        "containers" => template_containers(object, "/spec/template/spec/containers", false),
        "images" => template_containers(object, "/spec/template/spec/containers", true),
        "selector" => selector(object),
        _ => CellValue::Empty,
    }
}

fn wide_workload_columns() -> [ColumnDef; 3] {
    [
        fixed("containers", "Containers", 140.0).mono().wide(),
        fixed("images", "Images", 240.0).mono().wide(),
        fixed("selector", "Selector", 200.0).mono().wide(),
    ]
}

// ----- Workloads -----

fn pod_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("ready", "Ready", 50.0).mono(),
        fixed("status", "Status", 138.0),
        fixed("restarts", "Restarts", 72.0).mono().align_end(),
        fixed("cpu", "CPU", 92.0).mono(),
        fixed("memory", "Memory", 92.0).mono(),
        fixed("node", "Node", 120.0).mono(),
        age_column(),
        fixed("ip", "IP", 110.0).mono().wide(),
        fixed("nominated", "Nominated node", 130.0).mono().wide(),
    ]
}

fn pod_cell(pod: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "ready" => {
            let s = pod_status(pod);
            if s.is_degraded() {
                CellValue::Tinted {
                    label: s.ready_label().into(),
                    tone: Tone::Bad,
                }
            } else {
                text(s.ready_label())
            }
        }
        "status" => {
            let s = pod_status(pod);
            let tone = s.tone();
            status(s.reason, tone)
        }
        "restarts" => {
            let restarts = pod_status(pod).restarts;
            if restarts >= 3 {
                CellValue::Tinted {
                    label: restarts.to_string().into(),
                    tone: Tone::Bad,
                }
            } else {
                text(restarts.to_string())
            }
        }
        "node" => match str_at(pod, "/spec/nodeName") {
            "" => muted("<none>"),
            node => muted(node),
        },
        "ip" => match str_at(pod, "/status/podIP") {
            "" => muted("<none>"),
            ip => muted(ip),
        },
        "nominated" => match str_at(pod, "/status/nominatedNodeName") {
            "" => muted("<none>"),
            node => muted(node),
        },
        _ => CellValue::Empty,
    }
}

fn deployment_columns() -> Vec<ColumnDef> {
    let mut columns = vec![
        name_column(),
        fixed("ready", "Ready", 70.0).mono(),
        fixed("updated", "Up-to-date", 92.0).mono().align_end(),
        fixed("available", "Available", 84.0).mono().align_end(),
        age_column(),
    ];
    columns.extend(wide_workload_columns());
    columns
}

fn deployment_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "ready" => ratio(
            int_at(object, "/status/readyReplicas"),
            int_at(object, "/spec/replicas"),
        ),
        "updated" => number(int_at(object, "/status/updatedReplicas")),
        "available" => number(int_at(object, "/status/availableReplicas")),
        _ => workload_wide(object, column),
    }
}

fn statefulset_columns() -> Vec<ColumnDef> {
    let mut columns = vec![
        name_column(),
        fixed("ready", "Ready", 70.0).mono(),
        age_column(),
    ];
    columns.extend(wide_workload_columns().into_iter().take(2));
    columns
}

fn statefulset_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "ready" => ratio(
            int_at(object, "/status/readyReplicas"),
            int_at(object, "/spec/replicas"),
        ),
        _ => workload_wide(object, column),
    }
}

fn daemonset_columns() -> Vec<ColumnDef> {
    let mut columns = vec![
        name_column(),
        fixed("desired", "Desired", 70.0).mono().align_end(),
        fixed("current", "Current", 70.0).mono().align_end(),
        fixed("ready", "Ready", 60.0).mono().align_end(),
        fixed("updated", "Up-to-date", 92.0).mono().align_end(),
        fixed("available", "Available", 84.0).mono().align_end(),
        fixed("node_selector", "Node selector", 160.0).mono(),
        age_column(),
    ];
    columns.extend(wide_workload_columns());
    columns
}

fn daemonset_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "desired" => number(int_at(object, "/status/desiredNumberScheduled")),
        "current" => number(int_at(object, "/status/currentNumberScheduled")),
        "ready" => {
            let ready = int_at(object, "/status/numberReady");
            let desired = int_at(object, "/status/desiredNumberScheduled");
            if ready < desired {
                CellValue::Tinted {
                    label: ready.to_string().into(),
                    tone: Tone::Bad,
                }
            } else {
                number(ready)
            }
        }
        "updated" => number(int_at(object, "/status/updatedNumberScheduled")),
        "available" => number(int_at(object, "/status/numberAvailable")),
        "node_selector" => muted(join_or_none(&map_pairs(
            object.pointer("/spec/template/spec/nodeSelector"),
        ))),
        _ => workload_wide(object, column),
    }
}

fn replicaset_columns() -> Vec<ColumnDef> {
    let mut columns = vec![
        name_column(),
        fixed("desired", "Desired", 70.0).mono().align_end(),
        fixed("current", "Current", 70.0).mono().align_end(),
        fixed("ready", "Ready", 60.0).mono().align_end(),
        age_column(),
    ];
    columns.extend(wide_workload_columns());
    columns
}

fn replicaset_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "desired" => number(int_at(object, "/spec/replicas")),
        "current" => number(int_at(object, "/status/replicas")),
        "ready" => number(int_at(object, "/status/readyReplicas")),
        _ => workload_wide(object, column),
    }
}

/// `Complete`, `Failed`, `Suspended`, `Terminating` or `Running`, like kubectl 1.30+.
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
    } else if condition("Suspended") {
        "Suspended"
    } else if job.pointer("/metadata/deletionTimestamp").is_some() {
        "Terminating"
    } else {
        "Running"
    }
}

fn job_columns() -> Vec<ColumnDef> {
    let mut columns = vec![
        name_column(),
        fixed("status", "Status", 110.0),
        fixed("completions", "Completions", 96.0).mono(),
        fixed("duration", "Duration", 76.0).mono(),
        age_column(),
    ];
    columns.extend(wide_workload_columns());
    columns
}

fn job_cell(job: &Value, column: &str, now: Timestamp) -> CellValue {
    match column {
        "status" => {
            let label = job_status(job);
            let tone = match label {
                "Complete" => Tone::Good,
                "Failed" => Tone::Bad,
                "Suspended" | "Terminating" => Tone::Warning,
                _ => Tone::Info,
            };
            status(label, tone)
        }
        "completions" => {
            let succeeded = int_at(job, "/status/succeeded");
            match job.pointer("/spec/completions").and_then(Value::as_i64) {
                Some(completions) => text(format!("{succeeded}/{completions}")),
                None => match int_at(job, "/spec/parallelism") {
                    p if p > 1 => text(format!("{succeeded}/1 of {p}")),
                    _ => text(format!("{succeeded}/1")),
                },
            }
        }
        "duration" => {
            let Some(start) = timestamp(str_at(job, "/status/startTime")) else {
                return CellValue::Empty;
            };
            let end = timestamp(str_at(job, "/status/completionTime")).unwrap_or(now);
            muted(human_duration(seconds_since(start, end)))
        }
        _ => workload_wide(job, column),
    }
}

fn cronjob_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("schedule", "Schedule", 120.0).mono(),
        fixed("timezone", "Timezone", 90.0).mono(),
        fixed("suspend", "Suspend", 70.0).mono(),
        fixed("active", "Active", 60.0).mono().align_end(),
        fixed("last_schedule", "Last schedule", 104.0).mono(),
        age_column(),
        fixed("containers", "Containers", 140.0).mono().wide(),
        fixed("images", "Images", 240.0).mono().wide(),
    ]
}

fn cronjob_cell(object: &Value, column: &str, now: Timestamp) -> CellValue {
    match column {
        "schedule" => text(str_at(object, "/spec/schedule")),
        "timezone" => match str_at(object, "/spec/timeZone") {
            "" => muted("<none>"),
            tz => text(tz),
        },
        "suspend" => {
            if object.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true) {
                status("True", Tone::Warning)
            } else {
                muted("False")
            }
        }
        "active" => number(array_at(object, "/status/active").len() as i64),
        "last_schedule" => match str_at(object, "/status/lastScheduleTime") {
            "" => muted("<none>"),
            time => muted(crate::format::age(time, now)),
        },
        "containers" => template_containers(
            object,
            "/spec/jobTemplate/spec/template/spec/containers",
            false,
        ),
        "images" => template_containers(
            object,
            "/spec/jobTemplate/spec/template/spec/containers",
            true,
        ),
        _ => CellValue::Empty,
    }
}

// ----- Network -----

fn service_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("type", "Type", 104.0),
        fixed("cluster_ip", "Cluster IP", 118.0).mono(),
        fixed("external_ip", "External IP", 130.0).mono(),
        fixed("ports", "Ports", 150.0).mono(),
        age_column(),
        fixed("selector", "Selector", 200.0).mono().wide(),
    ]
}

fn load_balancer_addresses(object: &Value) -> Vec<String> {
    array_at(object, "/status/loadBalancer/ingress")
        .iter()
        .filter_map(|i| {
            i["ip"]
                .as_str()
                .or_else(|| i["hostname"].as_str())
                .map(String::from)
        })
        .collect()
}

fn service_cell(svc: &Value, column: &str, _: Timestamp) -> CellValue {
    let external_ips: Vec<String> = array_at(svc, "/spec/externalIPs")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    match column {
        "type" => text(str_at(svc, "/spec/type")),
        "cluster_ip" => muted(match str_at(svc, "/spec/clusterIP") {
            "" => "<none>",
            ip => ip,
        }),
        "external_ip" => match str_at(svc, "/spec/type") {
            "LoadBalancer" => {
                let mut all = load_balancer_addresses(svc);
                all.extend(external_ips);
                if all.is_empty() {
                    muted("<pending>")
                } else {
                    text(all.join(","))
                }
            }
            "ExternalName" => text(str_at(svc, "/spec/externalName")),
            _ => muted(join_or_none(&external_ips)),
        },
        "ports" => {
            let ports: Vec<String> = array_at(svc, "/spec/ports")
                .iter()
                .map(|p| {
                    let protocol = match str_at(p, "/protocol") {
                        "" => "TCP",
                        protocol => protocol,
                    };
                    match p["nodePort"].as_i64() {
                        Some(node_port) => {
                            format!("{}:{node_port}/{protocol}", int_at(p, "/port"))
                        }
                        None => format!("{}/{protocol}", int_at(p, "/port")),
                    }
                })
                .collect();
            text(join_or_none(&ports))
        }
        "selector" => muted(join_or_none(&map_pairs(svc.pointer("/spec/selector")))),
        _ => CellValue::Empty,
    }
}

fn ingress_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("class", "Class", 90.0),
        fixed("hosts", "Hosts", 200.0).mono(),
        fixed("address", "Address", 130.0).mono(),
        fixed("ports", "Ports", 70.0).mono(),
        age_column(),
    ]
}

fn ingress_cell(ing: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "class" => {
            let class = match str_at(ing, "/spec/ingressClassName") {
                "" => str_at(ing, "/metadata/annotations/kubernetes.io~1ingress.class"),
                class => class,
            };
            if class.is_empty() {
                muted("<none>")
            } else {
                text(class)
            }
        }
        "hosts" => {
            let hosts: Vec<&str> = array_at(ing, "/spec/rules")
                .iter()
                .filter_map(|r| r["host"].as_str())
                .collect();
            text(if hosts.is_empty() {
                "*".to_string()
            } else {
                hosts.join(",")
            })
        }
        "address" => muted(load_balancer_addresses(ing).join(",")),
        "ports" => text(if array_at(ing, "/spec/tls").is_empty() {
            "80"
        } else {
            "80, 443"
        }),
        _ => CellValue::Empty,
    }
}

// ----- Config -----

fn configmap_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("data", "Data", 60.0).mono().align_end(),
        age_column(),
    ]
}

fn map_len(object: &Value, pointer: &str) -> usize {
    object
        .pointer(pointer)
        .and_then(Value::as_object)
        .map_or(0, |m| m.len())
}

fn configmap_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "data" => number((map_len(object, "/data") + map_len(object, "/binaryData")) as i64),
        _ => CellValue::Empty,
    }
}

fn secret_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("type", "Type", 260.0).mono(),
        fixed("data", "Data", 60.0).mono().align_end(),
        age_column(),
    ]
}

/// Only counts keys; secret values are never read into cells.
fn secret_cell(object: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "type" => muted(str_at(object, "/type")),
        "data" => number((map_len(object, "/data") + map_len(object, "/stringData")) as i64),
        _ => CellValue::Empty,
    }
}

// ----- Storage -----

fn access_modes(object: &Value, pointer: &str) -> String {
    let modes: Vec<&str> = array_at(object, pointer)
        .iter()
        .filter_map(Value::as_str)
        .map(|m| match m {
            "ReadWriteOnce" => "RWO",
            "ReadOnlyMany" => "ROX",
            "ReadWriteMany" => "RWX",
            "ReadWriteOncePod" => "RWOP",
            other => other,
        })
        .collect();
    modes.join(",")
}

fn volume_phase_tone(phase: &str) -> Tone {
    match phase {
        "Bound" => Tone::Good,
        "Available" => Tone::Info,
        "Pending" | "Released" | "Terminating" => Tone::Warning,
        "Lost" | "Failed" => Tone::Bad,
        _ => Tone::Neutral,
    }
}

fn pvc_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("status", "Status", 100.0),
        fixed("volume", "Volume", 200.0).mono(),
        fixed("capacity", "Capacity", 80.0).mono(),
        fixed("access", "Access modes", 100.0).mono(),
        fixed("class", "Storage class", 120.0).mono(),
        age_column(),
    ]
}

fn pvc_cell(pvc: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "status" => {
            let phase = if pvc.pointer("/metadata/deletionTimestamp").is_some() {
                "Terminating"
            } else {
                str_at(pvc, "/status/phase")
            };
            status(phase, volume_phase_tone(phase))
        }
        "volume" => muted(str_at(pvc, "/spec/volumeName")),
        "capacity" => text(str_at(pvc, "/status/capacity/storage")),
        "access" => text(access_modes(pvc, "/status/accessModes")),
        "class" => muted(str_at(pvc, "/spec/storageClassName")),
        _ => CellValue::Empty,
    }
}

fn pv_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("capacity", "Capacity", 80.0).mono(),
        fixed("access", "Access modes", 100.0).mono(),
        fixed("reclaim", "Reclaim policy", 110.0),
        fixed("status", "Status", 100.0),
        fixed("claim", "Claim", 200.0).mono(),
        fixed("class", "Storage class", 120.0).mono(),
        age_column(),
    ]
}

fn pv_cell(pv: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "capacity" => text(str_at(pv, "/spec/capacity/storage")),
        "access" => text(access_modes(pv, "/spec/accessModes")),
        "reclaim" => text(str_at(pv, "/spec/persistentVolumeReclaimPolicy")),
        "status" => {
            let phase = if pv.pointer("/metadata/deletionTimestamp").is_some() {
                "Terminating"
            } else {
                str_at(pv, "/status/phase")
            };
            status(phase, volume_phase_tone(phase))
        }
        "claim" => match (
            str_at(pv, "/spec/claimRef/namespace"),
            str_at(pv, "/spec/claimRef/name"),
        ) {
            (_, "") => CellValue::Empty,
            (ns, name) => muted(format!("{ns}/{name}")),
        },
        "class" => muted(str_at(pv, "/spec/storageClassName")),
        _ => CellValue::Empty,
    }
}

fn storageclass_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("provisioner", "Provisioner", 200.0).mono(),
        fixed("reclaim", "Reclaim policy", 110.0),
        fixed("binding", "Volume binding mode", 160.0),
        fixed("expansion", "Allow expansion", 116.0).mono(),
        age_column(),
    ]
}

fn storageclass_cell(sc: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "name" => {
            let default = str_at(
                sc,
                "/metadata/annotations/storageclass.kubernetes.io~1is-default-class",
            ) == "true";
            text(if default {
                format!("{} (default)", name(sc))
            } else {
                name(sc).to_string()
            })
        }
        "provisioner" => muted(str_at(sc, "/provisioner")),
        "reclaim" => text(match str_at(sc, "/reclaimPolicy") {
            "" => "Delete",
            policy => policy,
        }),
        "binding" => text(match str_at(sc, "/volumeBindingMode") {
            "" => "Immediate",
            mode => mode,
        }),
        "expansion" => text(
            sc.pointer("/allowVolumeExpansion")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .to_string(),
        ),
        _ => CellValue::Empty,
    }
}

// ----- Cluster -----

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

fn node_address(node: &Value, kind: &str) -> String {
    array_at(node, "/status/addresses")
        .iter()
        .find(|a| str_at(a, "/type") == kind)
        .map(|a| str_at(a, "/address").to_string())
        .unwrap_or_else(|| "<none>".into())
}

fn node_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("status", "Status", 190.0),
        fixed("roles", "Roles", 110.0),
        fixed("cpu", "CPU", 96.0).mono(),
        fixed("memory", "Memory", 96.0).mono(),
        fixed("version", "Version", 90.0).mono(),
        age_column(),
        fixed("internal_ip", "Internal IP", 110.0).mono().wide(),
        fixed("external_ip", "External IP", 110.0).mono().wide(),
        fixed("os_image", "OS image", 180.0).wide(),
        fixed("kernel", "Kernel", 150.0).mono().wide(),
        fixed("runtime", "Container runtime", 170.0).mono().wide(),
    ]
}

fn node_cell(node: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "status" => {
            let label = node_status(node);
            let tone = if label.starts_with("NotReady") {
                Tone::Bad
            } else if label.starts_with("Unknown") || label.contains("SchedulingDisabled") {
                Tone::Warning
            } else {
                Tone::Good
            };
            status(label, tone)
        }
        "roles" => text(join_or_none(&node_roles(node))),
        "version" => muted(str_at(node, "/status/nodeInfo/kubeletVersion")),
        "internal_ip" => muted(node_address(node, "InternalIP")),
        "external_ip" => muted(node_address(node, "ExternalIP")),
        "os_image" => text(str_at(node, "/status/nodeInfo/osImage")),
        "kernel" => muted(str_at(node, "/status/nodeInfo/kernelVersion")),
        "runtime" => muted(str_at(node, "/status/nodeInfo/containerRuntimeVersion")),
        _ => CellValue::Empty,
    }
}

fn namespace_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("status", "Status", 110.0),
        age_column(),
    ]
}

fn namespace_cell(ns: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "status" => {
            let phase = str_at(ns, "/status/phase");
            let tone = if phase == "Active" {
                Tone::Good
            } else {
                Tone::Warning
            };
            status(phase, tone)
        }
        _ => CellValue::Empty,
    }
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

fn event_columns() -> Vec<ColumnDef> {
    vec![
        fixed("last_seen", "Last seen", 80.0).mono(),
        fixed("type", "Type", 80.0),
        fixed("reason", "Reason", 150.0),
        fixed("object", "Object", 240.0).mono(),
        flex("message", "Message", 240.0),
        fixed("count", "Count", 60.0).mono().align_end().wide(),
        fixed("name", "Name", 220.0).mono().wide(),
    ]
}

fn event_cell(event: &Value, column: &str, now: Timestamp) -> CellValue {
    match column {
        "last_seen" => muted(
            event_time(event)
                .map(|t| human_duration(seconds_since(t, now)))
                .unwrap_or_default(),
        ),
        "type" => match str_at(event, "/type") {
            "Warning" => CellValue::Tinted {
                label: "Warning".into(),
                tone: Tone::Warning,
            },
            kind => muted(kind),
        },
        "reason" => text(str_at(event, "/reason")),
        "object" => text(event_object(event)),
        "message" => text(event_message(event).replace('\n', " ")),
        "count" => {
            let count = event
                .pointer("/series/count")
                .or_else(|| event.pointer("/count"))
                .or_else(|| event.pointer("/deprecatedCount"))
                .and_then(Value::as_i64)
                .unwrap_or(1);
            number(count)
        }
        _ => CellValue::Empty,
    }
}

// ----- Access control -----

fn serviceaccount_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("secrets", "Secrets", 70.0).mono().align_end(),
        age_column(),
    ]
}

fn serviceaccount_cell(sa: &Value, column: &str, _: Timestamp) -> CellValue {
    match column {
        "secrets" => number(array_at(sa, "/secrets").len() as i64),
        _ => CellValue::Empty,
    }
}

fn role_columns() -> Vec<ColumnDef> {
    vec![name_column(), age_column()]
}

fn binding_columns() -> Vec<ColumnDef> {
    vec![
        name_column(),
        fixed("role", "Role", 240.0).mono(),
        age_column(),
        fixed("users", "Users", 160.0).mono().wide(),
        fixed("groups", "Groups", 160.0).mono().wide(),
        fixed("serviceaccounts", "Service accounts", 200.0)
            .mono()
            .wide(),
    ]
}

fn binding_cell(binding: &Value, column: &str, _: Timestamp) -> CellValue {
    let subjects = |kind: &str| -> CellValue {
        let names: Vec<String> = array_at(binding, "/subjects")
            .iter()
            .filter(|s| str_at(s, "/kind") == kind)
            .map(|s| match (kind, str_at(s, "/namespace")) {
                ("ServiceAccount", ns) if !ns.is_empty() => {
                    format!("{ns}/{}", str_at(s, "/name"))
                }
                _ => str_at(s, "/name").to_string(),
            })
            .collect();
        muted(names.join(", "))
    };
    match column {
        "role" => text(format!(
            "{}/{}",
            str_at(binding, "/roleRef/kind"),
            str_at(binding, "/roleRef/name")
        )),
        "users" => subjects("User"),
        "groups" => subjects("Group"),
        "serviceaccounts" => subjects("ServiceAccount"),
        _ => CellValue::Empty,
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
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cell(kind: &Kind, object: &Value, column: &str) -> CellValue {
        kind.cell(object, column)
    }

    #[test]
    fn deployment_ready_is_red_when_short() {
        let kind = Kind {
            columns: deployment_columns,
            cell: deployment_cell,
        };
        let d = json!({"metadata": {"name": "web"}, "spec": {"replicas": 3}, "status": {"readyReplicas": 2}});
        assert_eq!(
            cell(&kind, &d, "ready"),
            CellValue::Tinted {
                label: "2/3".into(),
                tone: Tone::Bad
            }
        );
        assert_eq!(cell(&kind, &d, "name"), CellValue::Text("web".into()));
        assert!(is_failing("apps", "Deployment", &d));
    }

    #[test]
    fn service_ports_and_external_ips() {
        let svc = json!({"spec": {"type": "LoadBalancer", "clusterIP": "10.0.0.1",
            "ports": [{"port": 80, "nodePort": 31234, "protocol": "TCP"}, {"port": 53, "protocol": "UDP"}]}});
        assert_eq!(
            service_cell(&svc, "ports", Timestamp::now()),
            CellValue::Text("80:31234/TCP,53/UDP".into())
        );
        assert_eq!(
            service_cell(&svc, "external_ip", Timestamp::now()),
            muted("<pending>")
        );
    }

    #[test]
    fn secrets_only_count_keys() {
        let secret = json!({"type": "Opaque", "data": {"password": "c2VjcmV0", "user": "YQ=="}});
        assert_eq!(
            secret_cell(&secret, "data", Timestamp::now()),
            CellValue::Text("2".into())
        );
    }

    #[test]
    fn node_status_and_roles() {
        let node = json!({"metadata": {"labels": {"node-role.kubernetes.io/control-plane": ""}},
            "spec": {"unschedulable": true},
            "status": {"conditions": [{"type": "Ready", "status": "True"}]}});
        assert_eq!(node_status(&node), "Ready,SchedulingDisabled");
        assert_eq!(node_roles(&node), ["control-plane"]);
        assert!(!is_failing("", "Node", &node));
    }

    #[test]
    fn job_completions_and_status() {
        let job = json!({"spec": {"completions": 3}, "status": {"succeeded": 1,
            "conditions": [{"type": "Failed", "status": "True"}]}});
        assert_eq!(job_status(&job), "Failed");
        assert_eq!(
            job_cell(&job, "completions", Timestamp::now()),
            CellValue::Text("1/3".into())
        );
    }

    #[test]
    fn events_from_both_apis() {
        let core = json!({"type": "Warning", "reason": "BackOff", "message": "Back-off restarting",
            "involvedObject": {"kind": "Pod", "name": "web-0"}, "lastTimestamp": "2026-09-24T10:00:00Z"});
        assert_eq!(event_object(&core), "pod/web-0");
        assert!(event_time(&core).is_some());
        let new = json!({"type": "Normal", "note": "Scheduled", "regarding": {"kind": "Pod", "name": "a"},
            "eventTime": "2026-09-24T10:00:00.000000Z"});
        assert_eq!(event_message(&new), "Scheduled");
        assert!(event_time(&new).is_some());
    }
}
