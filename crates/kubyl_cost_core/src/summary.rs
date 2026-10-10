//! What the Cost view shows, from parsed allocations: the totals, the per-namespace rows and the
//! cost-over-time series.

use crate::model::{Allocation, IDLE, UNMOUNTED};

/// The tiles above the table.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    /// Everything, idle included when it was asked for.
    pub total: f64,
    /// Idle capacity (0 without `includeIdle`).
    pub idle: f64,
    pub cpu: f64,
    pub ram: f64,
    pub storage: f64,
    pub network: f64,
    /// GPU, load balancers, shared and external costs.
    pub other: f64,
    /// Usage over request, weighted by cost, over what namespaces requested (idle left out).
    pub cpu_efficiency: f64,
    pub ram_efficiency: f64,
    pub efficiency: f64,
}

fn weighted(
    allocations: &[&Allocation],
    cost: impl Fn(&Allocation) -> f64,
    eff: impl Fn(&Allocation) -> f64,
) -> f64 {
    let weight: f64 = allocations.iter().map(|a| cost(a)).sum();
    if weight <= 0.0 {
        return 0.0;
    }
    allocations.iter().map(|a| cost(a) * eff(a)).sum::<f64>() / weight
}

/// The totals of an accumulated allocation set.
pub fn totals(set: &[Allocation]) -> Totals {
    let used: Vec<&Allocation> = set.iter().filter(|a| !a.is_idle()).collect();
    Totals {
        total: set.iter().map(|a| a.total_cost).sum(),
        idle: set
            .iter()
            .filter(|a| a.is_idle())
            .map(|a| a.total_cost)
            .sum(),
        cpu: set.iter().map(|a| a.cpu_cost).sum(),
        ram: set.iter().map(|a| a.ram_cost).sum(),
        storage: set.iter().map(|a| a.storage_cost()).sum(),
        network: set.iter().map(|a| a.network_cost).sum(),
        other: set
            .iter()
            .map(|a| a.gpu_cost + a.load_balancer_cost + a.shared_cost + a.external_cost)
            .sum(),
        cpu_efficiency: weighted(&used, |a| a.cpu_cost, |a| a.cpu_efficiency),
        ram_efficiency: weighted(&used, |a| a.ram_cost, |a| a.ram_efficiency),
        efficiency: weighted(&used, |a| a.total_cost, |a| a.total_efficiency),
    }
}

/// A table row: a namespace, idle capacity or unmounted volumes.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub cpu: f64,
    pub ram: f64,
    pub storage: f64,
    pub network: f64,
    pub total: f64,
    /// `None` for idle capacity and unmounted volumes (nobody requested anything).
    pub efficiency: Option<f64>,
}

impl Row {
    pub fn is_idle(&self) -> bool {
        self.name == IDLE
    }

    /// `idle`, `unmounted volumes` or the namespace.
    pub fn label(&self) -> &str {
        match self.name.as_str() {
            IDLE => "idle",
            UNMOUNTED => "unmounted volumes",
            name => name,
        }
    }
}

/// The rows of an accumulated set, in the order OpenCost's parser gave (largest first, idle
/// and unmounted last).
pub fn rows(set: &[Allocation]) -> Vec<Row> {
    set.iter()
        .map(|a| Row {
            name: a.name.clone(),
            cpu: a.cpu_cost,
            ram: a.ram_cost,
            storage: a.storage_cost(),
            network: a.network_cost,
            total: a.total_cost,
            efficiency: (a.name != IDLE && a.name != UNMOUNTED).then_some(a.total_efficiency),
        })
        .collect()
}

/// Cost over time: one stacked value per step for the biggest namespaces, the rest folded into
/// "other", idle on top.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    /// Start of each step, Unix seconds.
    pub times: Vec<f64>,
    /// `(name, cost per step)`, biggest namespace first; `other` and `__idle__` last.
    pub stacks: Vec<(String, Vec<f64>)>,
}

impl Series {
    pub fn is_empty(&self) -> bool {
        self.stacks.iter().all(|(_, v)| v.iter().all(|c| *c <= 0.0))
    }
}

/// The start of each step: the windows OpenCost reports, and for a step without data (an empty
/// set) the neighbouring steps' spacing.
fn step_starts(sets: &[Vec<Allocation>], step: i64) -> Vec<f64> {
    let known: Vec<(usize, i64)> = sets
        .iter()
        .enumerate()
        .filter_map(|(ix, set)| {
            set.iter()
                .find_map(|a| a.start)
                .map(|t| (ix, t.as_second()))
        })
        .collect();
    let Some(&(anchor_ix, anchor)) = known.first() else {
        return Vec::new();
    };
    (0..sets.len())
        .map(|ix| (anchor + (ix as i64 - anchor_ix as i64) * step) as f64)
        .collect()
}

/// The series of per-step allocation sets: at most `top` namespaces by their cost over the
/// whole window.
pub fn series(sets: &[Vec<Allocation>], step_seconds: i64, top: usize) -> Series {
    let times = step_starts(sets, step_seconds);
    if times.is_empty() {
        return Series::default();
    }
    let mut names: Vec<String> = Vec::new();
    for set in sets {
        for a in set {
            if !names.contains(&a.name) {
                names.push(a.name.clone());
            }
        }
    }
    let cost = |name: &str, set: &[Allocation]| -> f64 {
        set.iter()
            .find(|a| a.name == name)
            .map(|a| a.total_cost)
            .unwrap_or(0.0)
    };
    let total_of = |name: &str| -> f64 { sets.iter().map(|s| cost(name, s)).sum() };
    let mut namespaces: Vec<&String> = names
        .iter()
        .filter(|n| n.as_str() != IDLE && n.as_str() != UNMOUNTED)
        .collect();
    namespaces.sort_by(|a, b| {
        total_of(b)
            .partial_cmp(&total_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });
    let mut stacks: Vec<(String, Vec<f64>)> = namespaces
        .iter()
        .take(top)
        .map(|n| (n.to_string(), sets.iter().map(|s| cost(n, s)).collect()))
        .collect();
    let mut other = vec![0.0; sets.len()];
    let mut folded = false;
    for n in namespaces
        .iter()
        .skip(top)
        .map(|n| n.as_str())
        .chain(names.iter().map(String::as_str).filter(|n| *n == UNMOUNTED))
    {
        folded = true;
        for (slot, set) in other.iter_mut().zip(sets) {
            *slot += cost(n, set);
        }
    }
    if folded {
        stacks.push(("other".into(), other));
    }
    if names.iter().any(|n| n == IDLE) {
        stacks.push((IDLE.into(), sets.iter().map(|s| cost(IDLE, s)).collect()));
    }
    Series { times, stacks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse;
    use serde_json::json;

    fn allocation(name: &str, cpu: f64, ram: f64, eff: f64) -> serde_json::Value {
        json!({"name": name, "cpuCost": cpu, "ramCost": ram, "pvCost": 0.1, "networkCost": 0.05,
            "totalCost": cpu + ram + 0.15, "cpuEfficiency": eff, "ramEfficiency": eff / 2.0, "totalEfficiency": eff,
            "window": {"start": "2026-10-09T00:00:00Z", "end": "2026-10-10T00:00:00Z"}})
    }

    #[test]
    fn totals_weight_efficiency_by_cost_and_leave_idle_out() {
        let sets = parse(&json!({"code": 200, "data": [{
            "big": allocation("big", 8.0, 2.0, 0.5),
            "small": allocation("small", 1.0, 1.0, 1.0),
            "__idle__": {"name": "__idle__", "cpuCost": 4.0, "ramCost": 2.0, "totalCost": 6.0},
        }]}))
        .unwrap();
        let totals = totals(&sets[0]);
        assert!((totals.total - (10.15 + 2.15 + 6.0)).abs() < 1e-9);
        assert_eq!(totals.idle, 6.0);
        assert_eq!(totals.cpu, 13.0);
        assert_eq!(totals.ram, 5.0);
        assert!((totals.storage - 0.2).abs() < 1e-9);
        // Weighted by the cpu cost of the namespaces: (8*0.5 + 1*1.0) / 9.
        assert!((totals.cpu_efficiency - 5.0 / 9.0).abs() < 1e-9);
        assert!((totals.ram_efficiency - (2.0 * 0.25 + 1.0 * 0.5) / 3.0).abs() < 1e-9);
        assert_eq!(super::totals(&[]), Totals::default());
    }

    #[test]
    fn rows_label_idle_and_have_no_efficiency_for_it() {
        let sets = parse(&json!({"code": 200, "data": [{
            "shop": allocation("shop", 2.0, 1.0, 0.3),
            "__idle__": {"name": "__idle__", "totalCost": 4.0},
            "__unmounted__": {"name": "__unmounted__", "pvCost": 1.0, "totalCost": 1.0},
        }]}))
        .unwrap();
        let rows = rows(&sets[0]);
        let labels: Vec<&str> = rows.iter().map(Row::label).collect();
        assert_eq!(labels, ["shop", "idle", "unmounted volumes"]);
        assert_eq!(rows[0].efficiency, Some(0.3));
        assert_eq!(rows[1].efficiency, None);
        assert!(rows[1].is_idle() && !rows[0].is_idle());
    }

    #[test]
    fn the_chart_stacks_the_top_namespaces_other_and_idle() {
        let step = |t: &str, items: &[(&str, f64)], idle: f64| {
            let mut set = serde_json::Map::new();
            for (name, cost) in items {
                let mut a = allocation(name, *cost, 0.0, 0.5);
                a["window"]["start"] = json!(t);
                a["totalCost"] = json!(*cost);
                set.insert(name.to_string(), a);
            }
            if idle > 0.0 {
                set.insert(
                    "__idle__".into(),
                    json!({"name": "__idle__", "totalCost": idle, "window": {"start": t}}),
                );
            }
            serde_json::Value::Object(set)
        };
        let sets = parse(&json!({"code": 200, "data": [
            step("2026-10-08T00:00:00Z", &[("a", 5.0), ("b", 3.0), ("c", 1.0), ("d", 0.5)], 2.0),
            step("2026-10-09T00:00:00Z", &[("a", 6.0), ("b", 2.0), ("c", 1.5), ("d", 0.5)], 1.0),
        ]}))
        .unwrap();
        let series = series(&sets, 86_400, 2);
        assert_eq!(series.times, [1_791_417_600.0, 1_791_504_000.0]);
        let names: Vec<&str> = series.stacks.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "b", "other", "__idle__"]);
        assert_eq!(series.stacks[0].1, [5.0, 6.0]);
        assert_eq!(series.stacks[2].1, [1.5, 2.0], "c and d folded");
        assert_eq!(series.stacks[3].1, [2.0, 1.0]);
        assert!(!series.is_empty());
    }

    #[test]
    fn steps_without_data_get_their_time_from_their_neighbours() {
        let sets = parse(&json!({"code": 200, "data": [
            {}, {}, {"a": allocation("a", 1.0, 0.0, 0.5)}, {},
        ]}))
        .unwrap();
        let series = series(&sets, 3600, 5);
        let start = "2026-10-09T00:00:00Z"
            .parse::<jiff::Timestamp>()
            .unwrap()
            .as_second() as f64;
        assert_eq!(
            series.times,
            [start - 7200.0, start - 3600.0, start, start + 3600.0]
        );
        assert_eq!(series.stacks.len(), 1);
        assert_eq!(series.stacks[0].1[0], 0.0);
        // Nothing at all: nothing to draw.
        let none = parse(&json!({"code": 200, "data": [{}, {}]})).unwrap();
        assert!(super::series(&none, 3600, 5).times.is_empty());
        assert!(Series::default().is_empty());
    }
}
