//! The metric panels of the details dock: which charts an object gets, in what unit, and which
//! metric must exist for them (panels whose metric the Prometheus lacks are left out).
//!
//! Every series is a [`crate::queries`] id rendered with the object's filters, so the same
//! query serves a pod, a workload (its pods by name) and a namespace.

use kubyl_core::ResourceRef;
use kubyl_resources::format::{format_bytes, format_cpu};

/// How a panel's values read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Cores,
    Bytes,
    BytesPerSec,
    /// Operations, packets… per second.
    PerSec,
    /// 0–1, shown as a percentage.
    Ratio,
    Count,
}

impl Unit {
    pub fn format(self, value: f64) -> String {
        match self {
            // Idle pods use fractions of a millicore; `0m` would hide that they run at all.
            Unit::Cores if value > 0.0 && value < 0.0001 => "<0.1m".into(),
            Unit::Cores if value > 0.0 && value < 0.01 => format!("{:.1}m", value * 1000.0),
            Unit::Cores => format_cpu(value),
            Unit::Bytes if value.abs() < 1024.0 => format!("{value:.0} B"),
            Unit::Bytes => format_bytes(value),
            Unit::BytesPerSec if value.abs() < 1024.0 => format!("{value:.0} B/s"),
            Unit::BytesPerSec => format!("{}/s", format_bytes(value)),
            Unit::PerSec => format!("{}/s", compact_number(value)),
            Unit::Ratio => {
                let percent = value * 100.0;
                if percent > 0.0 && percent < 0.1 {
                    "<0.1%".into()
                } else if percent > 0.0 && percent < 10.0 {
                    format!("{percent:.1}%")
                } else {
                    format!("{percent:.0}%")
                }
            }
            Unit::Count => compact_number(value),
        }
    }

    /// Axis steps in powers of 1024.
    pub fn binary(self) -> bool {
        matches!(self, Unit::Bytes | Unit::BytesPerSec)
    }
}

/// `0`, `0.4`, `12`, `1.2k`, `3.4M`.
pub fn compact_number(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1e6 {
        format!("{:.1}M", value / 1e6)
    } else if abs >= 1e4 {
        format!("{:.0}k", value / 1e3)
    } else if abs >= 1e3 {
        format!("{:.1}k", value / 1e3)
    } else if abs >= 10.0 || value.fract() == 0.0 {
        format!("{value:.0}")
    } else if abs >= 1.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// One line of a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeriesDef {
    /// Tooltip name.
    pub name: &'static str,
    /// Prefix of the value in the panel header (`↓`, `r`…).
    pub short: &'static str,
    pub query: &'static str,
}

/// One small chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelDef {
    pub id: &'static str,
    pub title: &'static str,
    pub unit: Unit,
    /// Leave the panel out unless the Prometheus has this metric.
    pub needs: &'static str,
    pub series: &'static [SeriesDef],
    /// Shown instead of an empty chart.
    pub empty: &'static str,
}

const fn s(name: &'static str, short: &'static str, query: &'static str) -> SeriesDef {
    SeriesDef { name, short, query }
}

macro_rules! panel {
    ($id:literal, $title:literal, $unit:ident, $needs:literal, [$($series:expr),* $(,)?], $empty:expr) => {
        PanelDef { id: $id, title: $title, unit: Unit::$unit, needs: $needs, series: &[$($series),*], empty: $empty }
    };
}

const NO_DATA: &str = "No samples in this range.";
const HOST_NETWORK: &str = "No pod network samples (host-network pods use the node's).";

const CPU: PanelDef = panel!(
    "cpu",
    "CPU",
    Cores,
    "container_cpu_usage_seconds_total",
    [s("cpu", "", "cluster_cpu")],
    NO_DATA
);
const MEMORY: PanelDef = panel!(
    "memory",
    "Memory",
    Bytes,
    "container_memory_working_set_bytes",
    [s("working set", "", "cluster_memory")],
    NO_DATA
);
const NETWORK: PanelDef = panel!(
    "network",
    "Network",
    BytesPerSec,
    "container_network_receive_bytes_total",
    [
        s("receive", "↓", "net_rx_bytes"),
        s("transmit", "↑", "net_tx_bytes")
    ],
    HOST_NETWORK
);
const PACKETS: PanelDef = panel!(
    "packets",
    "Packets",
    PerSec,
    "container_network_receive_packets_total",
    [
        s("receive", "↓", "net_rx_packets"),
        s("transmit", "↑", "net_tx_packets")
    ],
    HOST_NETWORK
);
const DROPPED: PanelDef = panel!(
    "dropped",
    "Dropped packets",
    PerSec,
    "container_network_receive_packets_dropped_total",
    [
        s("receive", "↓", "net_rx_dropped"),
        s("transmit", "↑", "net_tx_dropped")
    ],
    HOST_NETWORK
);
const ERRORS: PanelDef = panel!(
    "errors",
    "Network errors",
    PerSec,
    "container_network_receive_errors_total",
    [
        s("receive", "↓", "net_rx_errors"),
        s("transmit", "↑", "net_tx_errors")
    ],
    HOST_NETWORK
);
const IOPS: PanelDef = panel!(
    "iops",
    "Disk IOPS",
    PerSec,
    "container_fs_reads_total",
    [s("read", "r", "fs_reads"), s("write", "w", "fs_writes")],
    NO_DATA
);
const THROUGHPUT: PanelDef = panel!(
    "disk",
    "Disk throughput",
    BytesPerSec,
    "container_fs_reads_bytes_total",
    [
        s("read", "r", "fs_read_bytes"),
        s("write", "w", "fs_write_bytes")
    ],
    NO_DATA
);
const THROTTLED: PanelDef = panel!(
    "throttled",
    "CPU throttled",
    Ratio,
    "container_cpu_cfs_throttled_periods_total",
    [s("throttled periods", "", "cpu_throttled")],
    "Not throttled (no CPU limit, or no samples)."
);
const PRESSURE: PanelDef = panel!(
    "pressure",
    "Pressure (PSI)",
    Ratio,
    "container_pressure_cpu_waiting_seconds_total",
    [
        s("cpu", "cpu", "psi_cpu"),
        s("memory", "mem", "psi_memory"),
        s("io", "io", "psi_io")
    ],
    NO_DATA
);
const OOM: PanelDef = panel!(
    "oom",
    "OOM kills & restarts",
    Count,
    "container_oom_events_total",
    [
        s("OOM kills", "oom", "oom_kills"),
        s("restarts", "restarts", "restarts_5m")
    ],
    NO_DATA
);

/// Pods: network, disk and health (CPU and memory are in the Usage section above).
pub const POD: &[PanelDef] = &[
    NETWORK, PACKETS, DROPPED, ERRORS, IOPS, THROUGHPUT, THROTTLED, PRESSURE, OOM,
];

/// Namespaces and workloads: everything, summed over their pods (pressure: the highest pod).
pub const GROUP: &[PanelDef] = &[
    CPU, MEMORY, NETWORK, PACKETS, DROPPED, ERRORS, IOPS, THROUGHPUT, THROTTLED, PRESSURE, OOM,
];

/// Nodes (node-exporter).
pub const NODE: &[PanelDef] = &[
    panel!(
        "cpu",
        "CPU",
        Ratio,
        "node_uname_info",
        [s("in use", "", "node_cpu_util")],
        NO_DATA
    ),
    panel!(
        "memory",
        "Memory",
        Ratio,
        "node_uname_info",
        [s("in use", "", "node_memory_util")],
        NO_DATA
    ),
    panel!(
        "network",
        "Network",
        BytesPerSec,
        "node_uname_info",
        [
            s("receive", "↓", "node_net_rx_bytes"),
            s("transmit", "↑", "node_net_tx_bytes")
        ],
        NO_DATA
    ),
    panel!(
        "packets",
        "Packets",
        PerSec,
        "node_uname_info",
        [
            s("receive", "↓", "node_net_rx_packets"),
            s("transmit", "↑", "node_net_tx_packets")
        ],
        NO_DATA
    ),
    panel!(
        "dropped",
        "Dropped packets",
        PerSec,
        "node_uname_info",
        [
            s("receive", "↓", "node_net_rx_dropped"),
            s("transmit", "↑", "node_net_tx_dropped")
        ],
        NO_DATA
    ),
    panel!(
        "errors",
        "Network errors",
        PerSec,
        "node_uname_info",
        [
            s("receive", "↓", "node_net_rx_errors"),
            s("transmit", "↑", "node_net_tx_errors")
        ],
        NO_DATA
    ),
    panel!(
        "tcp",
        "TCP retransmits",
        PerSec,
        "node_netstat_Tcp_RetransSegs",
        [s("retransmitted segments", "", "node_tcp_retrans")],
        NO_DATA
    ),
    panel!(
        "conntrack",
        "Conntrack table",
        Ratio,
        "node_nf_conntrack_entries",
        [s("in use", "", "node_conntrack")],
        NO_DATA
    ),
    panel!(
        "iops",
        "Disk IOPS",
        PerSec,
        "node_uname_info",
        [
            s("read", "r", "node_disk_reads"),
            s("write", "w", "node_disk_writes")
        ],
        NO_DATA
    ),
    panel!(
        "disk",
        "Disk throughput",
        BytesPerSec,
        "node_uname_info",
        [
            s("read", "r", "node_disk_read_bytes"),
            s("write", "w", "node_disk_write_bytes")
        ],
        NO_DATA
    ),
    panel!(
        "busy",
        "Disk busy",
        Ratio,
        "node_uname_info",
        [s("busiest disk", "", "node_disk_busy")],
        NO_DATA
    ),
    panel!(
        "pressure",
        "Pressure (PSI)",
        Ratio,
        "node_pressure_cpu_waiting_seconds_total",
        [
            s("cpu", "cpu", "node_psi_cpu"),
            s("memory", "mem", "node_psi_memory"),
            s("io", "io", "node_psi_io")
        ],
        NO_DATA
    ),
    panel!(
        "fs",
        "Fullest filesystem",
        Ratio,
        "node_uname_info",
        [s("in use", "", "node_fs_used")],
        NO_DATA
    ),
];

/// Persistent volume claims (kubelet volume stats).
pub const VOLUME: &[PanelDef] = &[
    panel!(
        "used",
        "Volume used",
        Bytes,
        "kubelet_volume_stats_used_bytes",
        [
            s("used", "", "volume_used"),
            s("capacity", "of", "volume_capacity")
        ],
        NO_DATA
    ),
    panel!(
        "inodes",
        "Inodes used",
        Ratio,
        "kubelet_volume_stats_inodes_used",
        [s("inodes", "", "volume_inodes")],
        NO_DATA
    ),
];

/// Label filters (`("pod=~", "web-.+")` for regex matchers).
pub type Filters = Vec<(String, String)>;

/// The panels for an object and the label filters that select its series, or `None` when the
/// kind has no metrics.
pub fn for_object(target: &ResourceRef, kind: &str) -> Option<(&'static [PanelDef], Filters)> {
    let name = target.name.clone()?;
    let namespace = target.namespace.clone();
    let filter = |label: &str, value: String| (label.to_string(), value);
    let pods = |pattern: String| {
        Some(vec![
            filter("namespace", namespace.clone()?),
            filter("pod=~", pattern),
        ])
    };
    let escaped = regex_escape(&name);
    let (panels, filters): (&'static [PanelDef], Filters) = match kind {
        "Pod" => (
            POD,
            vec![filter("namespace", namespace?), filter("pod", name)],
        ),
        "Node" => (NODE, vec![filter("nodename", name)]),
        "Namespace" => (GROUP, vec![filter("namespace", name)]),
        // A workload's pods by their generated names (no dependency on recording rules or on
        // listing the pods): Deployment → ReplicaSet hash → pod suffix, StatefulSet → ordinal.
        "Deployment" => (GROUP, pods(format!("{escaped}-[a-z0-9]+-[a-z0-9]+"))?),
        "StatefulSet" => (GROUP, pods(format!("{escaped}-[0-9]+"))?),
        "DaemonSet" | "ReplicaSet" | "Job" => (GROUP, pods(format!("{escaped}-[a-z0-9]+"))?),
        "PersistentVolumeClaim" => (
            VOLUME,
            vec![
                filter("namespace", namespace?),
                filter("persistentvolumeclaim", name),
            ],
        ),
        _ => return None,
    };
    Some((panels, filters))
}

/// Escapes regex metacharacters (object names are DNS labels, but `.` is allowed in some).
fn regex_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if "\\.+*?()|[]{}^$".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{LIBRARY, Queries};
    use kubyl_core::{ClusterId, Gvr};

    #[test]
    fn every_panel_query_exists() {
        for panels in [POD, GROUP, NODE, VOLUME] {
            for panel in panels {
                for series in panel.series {
                    assert!(
                        LIBRARY.iter().any(|q| q.id == series.query),
                        "{}: unknown query {}",
                        panel.id,
                        series.query
                    );
                }
            }
        }
    }

    #[test]
    fn filters_per_kind() {
        let object = |resource: &str, ns: Option<&str>, name: &str| {
            ResourceRef::object(
                ClusterId::new("c"),
                Gvr::new("", "v1", resource),
                ns.map(str::to_string),
                name.to_string(),
            )
        };
        let (panels, filters) =
            for_object(&object("pods", Some("payments"), "web-0"), "Pod").unwrap();
        assert_eq!(panels.len(), POD.len());
        assert_eq!(
            filters,
            [
                ("namespace".to_string(), "payments".to_string()),
                ("pod".to_string(), "web-0".to_string())
            ]
        );
        let (_, filters) = for_object(
            &object("deployments", Some("payments"), "checkout-api"),
            "Deployment",
        )
        .unwrap();
        assert_eq!(
            filters[1],
            ("pod=~".into(), "checkout-api-[a-z0-9]+-[a-z0-9]+".into())
        );
        let (panels, filters) = for_object(&object("nodes", None, "worker"), "Node").unwrap();
        assert_eq!(panels.len(), NODE.len());
        assert_eq!(filters, [("nodename".to_string(), "worker".to_string())]);
        assert!(for_object(&object("configmaps", Some("a"), "b"), "ConfigMap").is_none());
        assert_eq!(regex_escape("a.b"), "a\\.b");

        // Filters render into valid matchers.
        let queries = Queries::default();
        let (_, filters) = for_object(
            &object("statefulsets", Some("payments"), "ledger-writer"),
            "StatefulSet",
        )
        .unwrap();
        let refs: Vec<(&str, &str)> = filters
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let promql = queries.render("net_rx_bytes", &refs).unwrap();
        assert!(
            promql.contains(",namespace=\"payments\",pod=~\"ledger-writer-[0-9]+\""),
            "{promql}"
        );
    }

    #[test]
    fn units() {
        assert_eq!(Unit::BytesPerSec.format(1536.0), "1.5Ki/s");
        assert_eq!(Unit::BytesPerSec.format(165.0), "165 B/s");
        assert_eq!(Unit::Cores.format(0.00042), "0.4m");
        assert_eq!(Unit::Cores.format(0.00001), "<0.1m");
        assert_eq!(Unit::Cores.format(0.25), "250m");
        assert_eq!(Unit::PerSec.format(0.0), "0/s");
        assert_eq!(Unit::PerSec.format(0.25), "0.25/s");
        assert_eq!(Unit::PerSec.format(1234.0), "1.2k/s");
        assert_eq!(Unit::Ratio.format(0.034), "3.4%");
        assert_eq!(Unit::Ratio.format(0.0004), "<0.1%");
        assert_eq!(Unit::Ratio.format(0.0), "0%");
        assert_eq!(Unit::Ratio.format(0.5), "50%");
        assert_eq!(Unit::Count.format(3.0), "3");
        assert!(Unit::Bytes.binary() && !Unit::Ratio.binary());
    }
}
