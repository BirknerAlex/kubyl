//! The PromQL library.
//!
//! Every query has a stable id, a raw expression over cAdvisor / kube-state-metrics series, and
//! (where kube-prometheus ships one) a variant over its recording rules, which is much cheaper on
//! big clusters. Which rules (and other metrics: node-exporter, PSI, volume stats…) exist is read
//! once per cluster from Prometheus's metric-name index. Users can replace any query in
//! settings.json (`metrics.queries.<id>`).
//!
//! `$sel` stands for extra label matchers (`,namespace="payments"`); it sits inside a selector's
//! braces after at least one other matcher. A filter label ending in `=~` or `!~` is a regex
//! matcher (`("pod=~", "web-.+")`).
//!
//! v2 added network (bandwidth, packets, drops, errors), disk (IOPS, throughput, busy time),
//! CPU throttling, PSI pressure, OOM kills, node-exporter node series and volume stats.

use std::collections::{BTreeMap, HashSet};

/// Bumped when ids or semantics change, so overrides can be checked against it.
pub const LIBRARY_VERSION: u32 = 2;

const CPU_RULE: &str = "node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m";
const CPU_RULE_OLD: &str =
    "node_namespace_pod_container:container_cpu_usage_seconds_total:sum_irate";
const MEMORY_RULE: &str = "node_namespace_pod_container:container_memory_working_set_bytes";
const OWNER_RULE: &str = "namespace_workload_pod:kube_pod_owner:relabel";

/// Container CPU in cores (raw cAdvisor).
macro_rules! cpu {
    () => {
        "rate(container_cpu_usage_seconds_total{container!=\"\",image!=\"\"$sel}[5m])"
    };
}

/// Container working set in bytes (raw cAdvisor).
macro_rules! memory {
    () => {
        "container_memory_working_set_bytes{container!=\"\",image!=\"\"$sel}"
    };
}

/// Maps pods to their node (kube-state-metrics), like kube-prometheus' own rules do.
macro_rules! pod_node {
    () => {
        " * on (namespace, pod) group_left(node) topk by (namespace, pod) (1, max by (namespace, pod, node) (kube_pod_info{node!=\"\"}))"
    };
}

/// Leaves out host-network pods: their sandbox reports the node's interfaces.
macro_rules! not_host_network {
    () => {
        " unless on (namespace, pod) kube_pod_info{host_network=\"true\"}"
    };
}

/// A pod-network counter as a per-second rate, summed over what the filters select.
macro_rules! pod_net {
    ($metric:literal) => {
        concat!(
            "sum(rate(",
            $metric,
            "{namespace!=\"\"$sel}[5m])",
            not_host_network!(),
            ")"
        )
    };
}

/// The same, grouped for breakdown charts.
macro_rules! net_by {
    ($by:literal, $metric:literal) => {
        concat!(
            "sum by (",
            $by,
            ") (rate(",
            $metric,
            "{namespace!=\"\"$sel}[5m])",
            not_host_network!(),
            ")"
        )
    };
}

/// A per-container counter as a per-second rate (container-level series only: cAdvisor also
/// reports the pod's cgroup, which would count everything twice).
macro_rules! container_rate {
    ($metric:literal) => {
        concat!("sum(rate(", $metric, "{container!=\"\"$sel}[5m]))")
    };
}

macro_rules! throttle_by {
    ($by:literal, $metric:literal) => {
        concat!(
            "sum by (",
            $by,
            ") (rate(",
            $metric,
            "{container!=\"\"$sel}[5m]))"
        )
    };
}

/// Pressure (PSI) of the pod cgroup; pressure isn't additive, so several pods show the highest.
macro_rules! pod_psi {
    ($metric:literal) => {
        concat!(
            "max(rate(",
            $metric,
            "{container=\"\",id=~\".*pod[0-9a-f_-]+(.slice)?\"$sel}[5m]))"
        )
    };
}

/// node-exporter series carry `instance`, not the node name: join through node_uname_info.
macro_rules! node_join {
    () => {
        " * on (instance) group_left(nodename) node_uname_info{nodename!=\"\"$sel}"
    };
}

/// Interfaces that aren't the node's own (pod veths, CNI bridges and tunnels, loopback).
macro_rules! virtual_devices {
    () => {
        "lo|veth.+|cali.+|cilium.+|lxc.+|flannel.+|cni.+|docker.+|br-.+|virbr.+|vxlan.+|genev.+|tunl.+|kube-.+|tap.+|tun.+|erspan.+|gre.+|gretap.+|ip6.+|ip_vti.+|sit.+|nodelocaldns"
    };
}

/// Block devices worth charting (kube-prometheus' selection).
macro_rules! disks {
    () => {
        "(/dev/)?(mmcblk.p.+|nvme.+|rbd.+|sd.+|vd.+|xvd.+|dm-.+|md.+|dasd.+)"
    };
}

/// Filesystems worth watching: not tmpfs/overlay/pseudo filesystems, not file bind mounts or
/// per-pod volume mounts.
macro_rules! node_fs {
    () => {
        "fstype!~\"tmpfs|overlay|squashfs|rootfs|nsfs|ramfs|fuse.*\",mountpoint!~\"/etc/.+|/run(/.*)?|/boot(/.*)?|/var/lib/kubelet/pods/.+\""
    };
}

macro_rules! node_net {
    ($metric:literal) => {
        concat!(
            "sum(rate(",
            $metric,
            "{device!~\"",
            virtual_devices!(),
            "\"}[5m])",
            node_join!(),
            ")"
        )
    };
}

macro_rules! node_disk {
    ($metric:literal) => {
        concat!(
            "sum(rate(",
            $metric,
            "{device=~\"",
            disks!(),
            "\"}[5m])",
            node_join!(),
            ")"
        )
    };
}

/// One entry of the library.
#[derive(Clone, Copy, Debug)]
pub struct QueryDef {
    pub id: &'static str,
    pub doc: &'static str,
    pub raw: &'static str,
    /// `(rule it needs, expression)`; alternatives are tried in order.
    pub rules: &'static [(&'static str, &'static str)],
}

macro_rules! q {
    ($id:literal, $doc:literal, raw: $raw:expr, rules: [$(($rule:expr, $expr:expr)),* $(,)?]) => {
        QueryDef { id: $id, doc: $doc, raw: $raw, rules: &[$(($rule, $expr)),*] }
    };
}

/// Every query, by id.
pub const LIBRARY: &[QueryDef] = &[
    q!("cluster_cpu", "CPU used by all containers, in cores.",
    raw: concat!("sum(", cpu!(), ")"),
    rules: [
        (CPU_RULE, "sum(node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m{namespace!=\"\"$sel})"),
        (CPU_RULE_OLD, "sum(node_namespace_pod_container:container_cpu_usage_seconds_total:sum_irate{namespace!=\"\"$sel})"),
    ]),
    q!("cluster_memory", "Working set of all containers, in bytes.",
        raw: concat!("sum(", memory!(), ")"),
        rules: [(MEMORY_RULE, "sum(node_namespace_pod_container:container_memory_working_set_bytes{namespace!=\"\",container!=\"\"$sel})")]),
    q!("namespace_cpu", "CPU by namespace, in cores.",
    raw: concat!("sum by (namespace) (", cpu!(), ")"),
    rules: [
        (CPU_RULE, "sum by (namespace) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m{namespace!=\"\"$sel})"),
        (CPU_RULE_OLD, "sum by (namespace) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_irate{namespace!=\"\"$sel})"),
    ]),
    q!("namespace_memory", "Working set by namespace, in bytes.",
        raw: concat!("sum by (namespace) (", memory!(), ")"),
        rules: [(MEMORY_RULE, "sum by (namespace) (node_namespace_pod_container:container_memory_working_set_bytes{namespace!=\"\",container!=\"\"$sel})")]),
    q!("pod_cpu", "CPU by pod, in cores.",
    raw: concat!("sum by (namespace, pod) (", cpu!(), ")"),
    rules: [
        (CPU_RULE, "sum by (namespace, pod) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m{namespace!=\"\"$sel})"),
        (CPU_RULE_OLD, "sum by (namespace, pod) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_irate{namespace!=\"\"$sel})"),
    ]),
    q!("pod_memory", "Working set by pod, in bytes.",
        raw: concat!("sum by (namespace, pod) (", memory!(), ")"),
        rules: [(MEMORY_RULE, "sum by (namespace, pod) (node_namespace_pod_container:container_memory_working_set_bytes{namespace!=\"\",container!=\"\"$sel})")]),
    q!("container_cpu", "CPU by container, in cores.",
        raw: concat!("sum by (namespace, pod, container) (", cpu!(), ")"),
        rules: [(CPU_RULE, "sum by (namespace, pod, container) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m{namespace!=\"\"$sel})")]),
    q!("container_memory", "Working set by container, in bytes.",
        raw: concat!("sum by (namespace, pod, container) (", memory!(), ")"),
        rules: [(MEMORY_RULE, "sum by (namespace, pod, container) (node_namespace_pod_container:container_memory_working_set_bytes{namespace!=\"\",container!=\"\"$sel})")]),
    q!("workload_cpu", "CPU by workload (Deployment, StatefulSet…), in cores. Needs kube-prometheus' owner rule.",
        raw: concat!("sum by (namespace, workload, workload_type) (", cpu!(), " * on (namespace, pod) group_left(workload, workload_type) namespace_workload_pod:kube_pod_owner:relabel)"),
        rules: [(OWNER_RULE, "sum by (namespace, workload, workload_type) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m{namespace!=\"\"$sel} * on (namespace, pod) group_left(workload, workload_type) namespace_workload_pod:kube_pod_owner:relabel)")]),
    q!("workload_memory", "Working set by workload, in bytes. Needs kube-prometheus' owner rule.",
        raw: concat!("sum by (namespace, workload, workload_type) (", memory!(), " * on (namespace, pod) group_left(workload, workload_type) namespace_workload_pod:kube_pod_owner:relabel)"),
        rules: [(OWNER_RULE, "sum by (namespace, workload, workload_type) (node_namespace_pod_container:container_memory_working_set_bytes{namespace!=\"\",container!=\"\"$sel} * on (namespace, pod) group_left(workload, workload_type) namespace_workload_pod:kube_pod_owner:relabel)")]),
    q!("node_cpu", "CPU used by pods on each node, in cores.",
        raw: concat!("sum by (node) (", cpu!(), pod_node!(), ")"),
        rules: [(CPU_RULE, "sum by (node) (node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m{namespace!=\"\"$sel})")]),
    q!("node_memory", "Working set of pods on each node, in bytes.",
        raw: concat!("sum by (node) (", memory!(), pod_node!(), ")"),
        rules: [(MEMORY_RULE, "sum by (node) (node_namespace_pod_container:container_memory_working_set_bytes{namespace!=\"\",container!=\"\"$sel})")]),
    q!("pods_running", "Running pods (kube-state-metrics).",
        raw: "sum(kube_pod_status_phase{phase=\"Running\"$sel})",
        rules: []),
    q!("nodes_ready", "Ready nodes (kube-state-metrics).",
        raw: "sum(kube_node_status_condition{condition=\"Ready\",status=\"true\"$sel})",
        rules: []),
    q!("restarts", "Container restarts in the last hour by pod (kube-state-metrics).",
        raw: "sum by (namespace, pod) (increase(kube_pod_container_status_restarts_total{namespace!=\"\"$sel}[1h]))",
        rules: []),
    // ----- Pods, workloads and namespaces (cAdvisor): one value for whatever the filters
    // select. Pod network is counted on the pod's sandbox, so host-network pods (which report
    // the whole node's traffic) are left out.
    q!("net_rx_bytes", "Received bytes per second.",
        raw: pod_net!("container_network_receive_bytes_total"), rules: []),
    q!("net_tx_bytes", "Transmitted bytes per second.",
        raw: pod_net!("container_network_transmit_bytes_total"), rules: []),
    q!("net_rx_packets", "Received packets per second.",
        raw: pod_net!("container_network_receive_packets_total"), rules: []),
    q!("net_tx_packets", "Transmitted packets per second.",
        raw: pod_net!("container_network_transmit_packets_total"), rules: []),
    q!("net_rx_dropped", "Dropped received packets per second.",
        raw: pod_net!("container_network_receive_packets_dropped_total"), rules: []),
    q!("net_tx_dropped", "Dropped transmitted packets per second.",
        raw: pod_net!("container_network_transmit_packets_dropped_total"), rules: []),
    q!("net_rx_errors", "Receive errors per second.",
        raw: pod_net!("container_network_receive_errors_total"), rules: []),
    q!("net_tx_errors", "Transmit errors per second.",
        raw: pod_net!("container_network_transmit_errors_total"), rules: []),
    q!("fs_reads", "Disk reads per second (IOPS).",
        raw: container_rate!("container_fs_reads_total"), rules: []),
    q!("fs_writes", "Disk writes per second (IOPS).",
        raw: container_rate!("container_fs_writes_total"), rules: []),
    q!("fs_read_bytes", "Bytes read from disk per second.",
        raw: container_rate!("container_fs_reads_bytes_total"), rules: []),
    q!("fs_write_bytes", "Bytes written to disk per second.",
        raw: container_rate!("container_fs_writes_bytes_total"), rules: []),
    q!("cpu_throttled", "Share of CPU periods throttled by the CFS quota (containers with CPU limits).",
        raw: concat!(container_rate!("container_cpu_cfs_throttled_periods_total"), " / ", container_rate!("container_cpu_cfs_periods_total")),
        rules: []),
    q!("psi_cpu", "CPU pressure: share of time some task waited for CPU (highest pod).",
        raw: pod_psi!("container_pressure_cpu_waiting_seconds_total"), rules: []),
    q!("psi_memory", "Memory pressure: share of time some task waited for memory (highest pod).",
        raw: pod_psi!("container_pressure_memory_waiting_seconds_total"), rules: []),
    q!("psi_io", "I/O pressure: share of time some task waited for I/O (highest pod).",
        raw: pod_psi!("container_pressure_io_waiting_seconds_total"), rules: []),
    q!("oom_kills", "OOM kills per 5 minutes.",
        raw: "sum(increase(container_oom_events_total{container!=\"\"$sel}[5m]))", rules: []),
    q!("restarts_5m", "Container restarts per 5 minutes (kube-state-metrics).",
        raw: "sum(increase(kube_pod_container_status_restarts_total{namespace!=\"\"$sel}[5m]))", rules: []),
    // ----- Nodes (node-exporter), joined to the node name through node_uname_info.
    q!("node_cpu_util", "Node CPU utilization.",
        raw: concat!("1 - avg(rate(node_cpu_seconds_total{mode=\"idle\"}[5m])", node_join!(), ")"), rules: []),
    q!("node_memory_util", "Node memory in use (1 - MemAvailable / MemTotal).",
        raw: concat!("1 - sum(node_memory_MemAvailable_bytes", node_join!(), ") / sum(node_memory_MemTotal_bytes", node_join!(), ")"), rules: []),
    q!("node_net_rx_bytes", "Node received bytes per second (physical interfaces).",
        raw: node_net!("node_network_receive_bytes_total"), rules: []),
    q!("node_net_tx_bytes", "Node transmitted bytes per second.",
        raw: node_net!("node_network_transmit_bytes_total"), rules: []),
    q!("node_net_rx_packets", "Node received packets per second.",
        raw: node_net!("node_network_receive_packets_total"), rules: []),
    q!("node_net_tx_packets", "Node transmitted packets per second.",
        raw: node_net!("node_network_transmit_packets_total"), rules: []),
    q!("node_net_rx_dropped", "Node dropped received packets per second.",
        raw: node_net!("node_network_receive_drop_total"), rules: []),
    q!("node_net_tx_dropped", "Node dropped transmitted packets per second.",
        raw: node_net!("node_network_transmit_drop_total"), rules: []),
    q!("node_net_rx_errors", "Node receive errors per second.",
        raw: node_net!("node_network_receive_errs_total"), rules: []),
    q!("node_net_tx_errors", "Node transmit errors per second.",
        raw: node_net!("node_network_transmit_errs_total"), rules: []),
    q!("node_disk_reads", "Node disk reads per second (IOPS).",
        raw: node_disk!("node_disk_reads_completed_total"), rules: []),
    q!("node_disk_writes", "Node disk writes per second (IOPS).",
        raw: node_disk!("node_disk_writes_completed_total"), rules: []),
    q!("node_disk_read_bytes", "Node bytes read per second.",
        raw: node_disk!("node_disk_read_bytes_total"), rules: []),
    q!("node_disk_write_bytes", "Node bytes written per second.",
        raw: node_disk!("node_disk_written_bytes_total"), rules: []),
    q!("node_disk_busy", "Busiest disk's share of time with I/O in flight.",
        raw: concat!("max(rate(node_disk_io_time_seconds_total{device=~\"", disks!(), "\"}[5m])", node_join!(), ")"), rules: []),
    q!("node_tcp_retrans", "TCP segments retransmitted per second.",
        raw: concat!("sum(rate(node_netstat_Tcp_RetransSegs[5m])", node_join!(), ")"), rules: []),
    q!("node_conntrack", "Conntrack table use (entries / limit); full tables drop new connections.",
        raw: concat!("sum(node_nf_conntrack_entries", node_join!(), ") / sum(node_nf_conntrack_entries_limit", node_join!(), ")"), rules: []),
    q!("node_psi_cpu", "Node CPU pressure (share of time some task waited).",
        raw: concat!("max(rate(node_pressure_cpu_waiting_seconds_total[5m])", node_join!(), ")"), rules: []),
    q!("node_psi_memory", "Node memory pressure.",
        raw: concat!("max(rate(node_pressure_memory_waiting_seconds_total[5m])", node_join!(), ")"), rules: []),
    q!("node_psi_io", "Node I/O pressure.",
        raw: concat!("max(rate(node_pressure_io_waiting_seconds_total[5m])", node_join!(), ")"), rules: []),
    q!("node_fs_used", "The fullest of the node's real filesystems (root, /var…).",
        raw: concat!("max((1 - node_filesystem_avail_bytes{", node_fs!(), "} / node_filesystem_size_bytes{", node_fs!(), "})", node_join!(), ")"), rules: []),
    // ----- Persistent volume claims (kubelet volume stats; not every storage driver reports).
    q!("volume_used", "Bytes used on the volume.",
        raw: "sum(kubelet_volume_stats_used_bytes{namespace!=\"\"$sel})", rules: []),
    q!("volume_capacity", "Volume capacity in bytes.",
        raw: "sum(kubelet_volume_stats_capacity_bytes{namespace!=\"\"$sel})", rules: []),
    q!("volume_inodes", "Share of inodes in use.",
        raw: "sum(kubelet_volume_stats_inodes_used{namespace!=\"\"$sel}) / sum(kubelet_volume_stats_inodes{namespace!=\"\"$sel})", rules: []),
    // ----- Breakdowns for the overview charts.
    q!("namespace_network", "Network (received + transmitted bytes/s) by namespace.",
        raw: concat!(net_by!("namespace", "container_network_receive_bytes_total"), " + ", net_by!("namespace", "container_network_transmit_bytes_total")),
        rules: []),
    q!("pod_network", "Network (received + transmitted bytes/s) by pod.",
        raw: concat!(net_by!("namespace, pod", "container_network_receive_bytes_total"), " + ", net_by!("namespace, pod", "container_network_transmit_bytes_total")),
        rules: []),
    q!("namespace_throttling", "Share of CPU periods throttled by namespace.",
        raw: concat!(throttle_by!("namespace", "container_cpu_cfs_throttled_periods_total"), " / ", throttle_by!("namespace", "container_cpu_cfs_periods_total")),
        rules: []),
    q!("pod_throttling", "Share of CPU periods throttled by pod.",
        raw: concat!(throttle_by!("namespace, pod", "container_cpu_cfs_throttled_periods_total"), " / ", throttle_by!("namespace, pod", "container_cpu_cfs_periods_total")),
        rules: []),
    q!("node_disk_iops", "Disk operations per second (reads + writes) by node.",
        raw: concat!("sum by (nodename) ((rate(node_disk_reads_completed_total{device=~\"", disks!(), "\"}[5m]) + rate(node_disk_writes_completed_total{device=~\"", disks!(), "\"}[5m]))", node_join!(), ")"),
        rules: []),
    q!("node_net_drops", "Dropped packets per second (in + out) by node.",
        raw: concat!("sum by (nodename) ((rate(node_network_receive_drop_total{device!~\"", virtual_devices!(), "\"}[5m]) + rate(node_network_transmit_drop_total{device!~\"", virtual_devices!(), "\"}[5m]))", node_join!(), ")"),
        rules: []),
];

/// The rendered expression for a query id, with filters and the cluster's rules applied.
#[derive(Clone, Debug, Default)]
pub struct Queries {
    overrides: BTreeMap<String, String>,
    /// Metric names the Prometheus has (recording rules included).
    rules: HashSet<String>,
}

impl Queries {
    pub fn new(overrides: BTreeMap<String, String>, names: HashSet<String>) -> Self {
        Self {
            overrides,
            rules: names,
        }
    }

    /// Metric names the Prometheus has.
    pub fn rules(&self) -> &HashSet<String> {
        &self.rules
    }

    /// Whether the Prometheus has series named `metric`.
    pub fn has(&self, metric: &str) -> bool {
        self.rules.contains(metric)
    }

    /// PromQL for `id` with `filters` as extra label matchers. `None` for unknown ids.
    pub fn render(&self, id: &str, filters: &[(&str, &str)]) -> Option<String> {
        let template = match self.overrides.get(id) {
            Some(custom) => custom.clone(),
            None => {
                let def = LIBRARY.iter().find(|d| d.id == id)?;
                def.rules
                    .iter()
                    .find(|(rule, _)| self.rules.contains(*rule))
                    .map(|(_, expr)| *expr)
                    .unwrap_or(def.raw)
                    .to_string()
            }
        };
        Some(template.replace("$sel", &matchers(filters)))
    }
}

/// `,namespace="payments",pod=~"web-.+"`, values escaped for PromQL strings. A label ending in
/// `=~` or `!~` carries its own operator.
pub fn matchers(filters: &[(&str, &str)]) -> String {
    filters
        .iter()
        .map(|(label, value)| {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            if label.ends_with("=~") || label.ends_with("!~") {
                format!(",{label}\"{escaped}\"")
            } else {
                format!(",{label}=\"{escaped}\"")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_renders() {
        let plain = Queries::default();
        let rules = Queries::new(
            BTreeMap::new(),
            [CPU_RULE, MEMORY_RULE, OWNER_RULE]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
        for def in LIBRARY {
            for queries in [&plain, &rules] {
                let promql = queries
                    .render(def.id, &[("namespace", "payments")])
                    .unwrap();
                assert!(!promql.is_empty(), "{} is empty", def.id);
                assert!(!promql.contains("$sel"), "{}: {promql}", def.id);
                assert!(
                    promql.contains("namespace=\"payments\""),
                    "{}: {promql}",
                    def.id
                );
                assert_eq!(
                    promql.matches('(').count(),
                    promql.matches(')').count(),
                    "{}",
                    def.id
                );
            }
        }
        assert!(plain.render("nope", &[]).is_none());
    }

    #[test]
    fn prefers_recording_rules() {
        let plain = Queries::default();
        assert_eq!(
            plain.render("pod_cpu", &[]).unwrap(),
            "sum by (namespace, pod) (rate(container_cpu_usage_seconds_total{container!=\"\",image!=\"\"}[5m]))"
        );
        let old = Queries::new(BTreeMap::new(), HashSet::from([CPU_RULE_OLD.to_string()]));
        assert!(old.render("pod_cpu", &[]).unwrap().contains("sum_irate"));
        let both = Queries::new(
            BTreeMap::new(),
            HashSet::from([CPU_RULE_OLD.to_string(), CPU_RULE.to_string()]),
        );
        assert!(both.render("pod_cpu", &[]).unwrap().contains("sum_rate5m"));
    }

    #[test]
    fn overrides_and_escaping() {
        let queries = Queries::new(
            BTreeMap::from([("pod_cpu".to_string(), "my_cpu{x=\"1\"$sel}".to_string())]),
            HashSet::new(),
        );
        assert_eq!(
            queries.render("pod_cpu", &[("pod", "a\"b")]).unwrap(),
            "my_cpu{x=\"1\",pod=\"a\\\"b\"}"
        );
    }
}
