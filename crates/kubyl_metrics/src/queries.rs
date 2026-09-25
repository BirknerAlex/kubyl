//! The PromQL library.
//!
//! Every query has a stable id, a raw expression over cAdvisor / kube-state-metrics series, and
//! (where kube-prometheus ships one) a variant over its recording rules, which is much cheaper on
//! big clusters. Which rules exist is detected once per cluster ([`RULE_PROBE`]). Users can
//! replace any query in settings.json (`metrics.queries.<id>`).
//!
//! `$sel` stands for extra label matchers (`,namespace="payments"`); it sits inside a selector's
//! braces after at least one other matcher.

use std::collections::{BTreeMap, HashSet};

/// Bumped when ids or semantics change, so overrides can be checked against it.
pub const LIBRARY_VERSION: u32 = 1;

const CPU_RULE: &str = "node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m";
const CPU_RULE_OLD: &str =
    "node_namespace_pod_container:container_cpu_usage_seconds_total:sum_irate";
const MEMORY_RULE: &str = "node_namespace_pod_container:container_memory_working_set_bytes";
const OWNER_RULE: &str = "namespace_workload_pod:kube_pod_owner:relabel";

/// Finds which recording rules the cluster's Prometheus has.
pub const RULE_PROBE: &str = "count by (__name__) ({__name__=~\"node_namespace_pod_container:container_cpu_usage_seconds_total:sum_rate5m|node_namespace_pod_container:container_cpu_usage_seconds_total:sum_irate|node_namespace_pod_container:container_memory_working_set_bytes|namespace_workload_pod:kube_pod_owner:relabel\"})";

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
    q!("network_receive", "Received bytes per second by namespace.",
        raw: "sum by (namespace) (rate(container_network_receive_bytes_total{namespace!=\"\"$sel}[5m]))",
        rules: []),
    q!("network_transmit", "Transmitted bytes per second by namespace.",
        raw: "sum by (namespace) (rate(container_network_transmit_bytes_total{namespace!=\"\"$sel}[5m]))",
        rules: []),
    q!("filesystem", "Used bytes of persistent volumes by claim (kubelet volume stats).",
        raw: "sum by (namespace, persistentvolumeclaim) (kubelet_volume_stats_used_bytes{namespace!=\"\"$sel})",
        rules: []),
    q!("restarts", "Container restarts in the last hour by pod (kube-state-metrics).",
        raw: "sum by (namespace, pod) (increase(kube_pod_container_status_restarts_total{namespace!=\"\"$sel}[1h]))",
        rules: []),
];

/// The rendered expression for a query id, with filters and the cluster's rules applied.
#[derive(Clone, Debug, Default)]
pub struct Queries {
    overrides: BTreeMap<String, String>,
    rules: HashSet<String>,
}

impl Queries {
    pub fn new(overrides: BTreeMap<String, String>, rules: HashSet<String>) -> Self {
        Self { overrides, rules }
    }

    pub fn rules(&self) -> &HashSet<String> {
        &self.rules
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

/// `,namespace="payments",pod="web-0"`, values escaped for PromQL strings.
pub fn matchers(filters: &[(&str, &str)]) -> String {
    filters
        .iter()
        .map(|(label, value)| {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            format!(",{label}=\"{escaped}\"")
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
