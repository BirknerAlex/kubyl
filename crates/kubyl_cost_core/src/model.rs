//! OpenCost allocations, read leniently: a missing field is zero, a negative cost (OpenCost
//! reports tiny negative values while Prometheus has too little data) is zero, and `NaN` or
//! `null` efficiencies are zero.

use jiff::Timestamp;
use serde_json::Value;

/// OpenCost's name for idle capacity.
pub const IDLE: &str = "__idle__";
/// And for volumes no pod mounts.
pub const UNMOUNTED: &str = "__unmounted__";

/// One allocation: what a namespace (or idle capacity) cost over a window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Allocation {
    pub name: String,
    pub cpu_cost: f64,
    pub ram_cost: f64,
    /// Persistent volumes.
    pub pv_cost: f64,
    pub network_cost: f64,
    pub gpu_cost: f64,
    pub load_balancer_cost: f64,
    pub shared_cost: f64,
    pub external_cost: f64,
    pub total_cost: f64,
    /// Usage over request, 0 to 1 (can exceed 1 when pods use more than they request).
    pub cpu_efficiency: f64,
    pub ram_efficiency: f64,
    pub total_efficiency: f64,
    pub minutes: f64,
    /// The window of the allocation, when OpenCost says.
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
}

impl Allocation {
    pub fn is_idle(&self) -> bool {
        self.name == IDLE
    }

    /// Storage as the table shows it: persistent volumes.
    pub fn storage_cost(&self) -> f64 {
        self.pv_cost
    }
}

/// Costs and efficiencies are never negative here.
fn amount(value: &Value, key: &str) -> f64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
        .max(0.0)
}

fn time(value: &Value, pointer: &str) -> Option<Timestamp> {
    value.pointer(pointer)?.as_str()?.parse().ok()
}

fn allocation(name: &str, value: &Value) -> Allocation {
    // `total_cost` is the sum of the parts when OpenCost leaves it out.
    let parts = amount(value, "cpuCost")
        + amount(value, "ramCost")
        + amount(value, "pvCost")
        + amount(value, "networkCost")
        + amount(value, "gpuCost")
        + amount(value, "loadBalancerCost")
        + amount(value, "sharedCost")
        + amount(value, "externalCost");
    let total = if value.get("totalCost").is_some() {
        amount(value, "totalCost")
    } else {
        parts
    };
    Allocation {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string(),
        cpu_cost: amount(value, "cpuCost"),
        ram_cost: amount(value, "ramCost"),
        pv_cost: amount(value, "pvCost"),
        network_cost: amount(value, "networkCost"),
        gpu_cost: amount(value, "gpuCost"),
        load_balancer_cost: amount(value, "loadBalancerCost"),
        shared_cost: amount(value, "sharedCost"),
        external_cost: amount(value, "externalCost"),
        total_cost: total,
        cpu_efficiency: amount(value, "cpuEfficiency"),
        ram_efficiency: amount(value, "ramEfficiency"),
        total_efficiency: amount(value, "totalEfficiency"),
        minutes: amount(value, "minutes"),
        start: time(value, "/window/start"),
        end: time(value, "/window/end"),
    }
}

/// An allocation response: OpenCost's `code` and, for an error, its `message`.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    /// `code` isn't 200: OpenCost's own message (a window it can't read, Prometheus down).
    Api(String),
    /// Not an allocation response at all.
    Invalid(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Api(message) => write!(f, "OpenCost says: {message}"),
            ParseError::Invalid(why) => write!(f, "not an OpenCost allocation response ({why})"),
        }
    }
}

/// The allocation sets of a response, one `Vec` per step (an empty one for a step without
/// data), each sorted by cost, largest first (`__idle__` and `__unmounted__` last).
pub fn parse(response: &Value) -> Result<Vec<Vec<Allocation>>, ParseError> {
    let code = response.get("code").and_then(Value::as_i64);
    if code.is_some_and(|c| c != 200) {
        let message = response
            .get("message")
            .or_else(|| response.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("an error without a message");
        return Err(ParseError::Api(message.to_string()));
    }
    let sets = response
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ParseError::Invalid("no `data` array".into()))?;
    Ok(sets
        .iter()
        .map(|set| {
            let mut allocations: Vec<Allocation> = set
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter(|(_, v)| v.is_object())
                        .map(|(name, v)| allocation(name, v))
                        .collect()
                })
                .unwrap_or_default();
            allocations.sort_by(|a, b| {
                let special = |x: &Allocation| x.name == IDLE || x.name == UNMOUNTED;
                special(a)
                    .cmp(&special(b))
                    .then(
                        b.total_cost
                            .partial_cmp(&a.total_cost)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
                    .then_with(|| a.name.cmp(&b.name))
            });
            allocations
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allocations_are_read_leniently() {
        let response = json!({"code": 200, "data": [{
            "shop": {"name": "shop", "window": {"start": "2026-10-09T00:00:00Z", "end": "2026-10-10T00:00:00Z"},
                "cpuCost": 1.5, "ramCost": 0.5, "pvCost": 0.25, "networkCost": 0.1, "totalCost": 2.35,
                "cpuEfficiency": 0.4, "ramEfficiency": 0.8, "totalEfficiency": 0.55, "minutes": 1440.0},
            "__idle__": {"name": "__idle__", "cpuCost": 3.0, "ramCost": 1.0, "totalCost": 4.0},
            "tiny": {"name": "tiny", "cpuCost": -0.00014, "ramCost": null, "cpuEfficiency": "NaN", "totalCost": -0.00015},
            "bank": {"cpuCost": 5.0, "ramCost": 1.0},
        }]});
        let sets = parse(&response).unwrap();
        assert_eq!(sets.len(), 1);
        let names: Vec<&str> = sets[0].iter().map(|a| a.name.as_str()).collect();
        // Largest first, idle last.
        assert_eq!(names, ["bank", "shop", "tiny", "__idle__"]);
        let shop = sets[0].iter().find(|a| a.name == "shop").unwrap();
        assert_eq!(shop.total_cost, 2.35);
        assert_eq!(shop.cpu_efficiency, 0.4);
        assert_eq!(shop.start, "2026-10-09T00:00:00Z".parse().ok());
        // Negative costs and `NaN`/`null` count as zero.
        let tiny = sets[0].iter().find(|a| a.name == "tiny").unwrap();
        assert_eq!(
            (
                tiny.cpu_cost,
                tiny.ram_cost,
                tiny.cpu_efficiency,
                tiny.total_cost
            ),
            (0.0, 0.0, 0.0, 0.0)
        );
        // The name comes from the key and the total from the parts when a field is missing.
        let bank = sets[0].iter().find(|a| a.name == "bank").unwrap();
        assert_eq!(bank.total_cost, 6.0);
        assert!(sets[0].last().unwrap().is_idle());
    }

    #[test]
    fn steps_without_data_stay_in_place() {
        let response =
            json!({"code": 200, "data": [{}, {"a": {"name": "a", "totalCost": 1.0}}, null]});
        let sets = parse(&response).unwrap();
        assert_eq!(sets.len(), 3);
        assert!(sets[0].is_empty() && sets[2].is_empty());
        assert_eq!(sets[1].len(), 1);
    }

    #[test]
    fn errors_say_what_opencost_said() {
        let err = parse(&json!({"code": 500, "message": "Prometheus is unreachable"})).unwrap_err();
        assert_eq!(err, ParseError::Api("Prometheus is unreachable".into()));
        assert!(err.to_string().contains("Prometheus is unreachable"));
        assert!(matches!(
            parse(&json!({"hello": 1})),
            Err(ParseError::Invalid(_))
        ));
        assert!(matches!(parse(&json!([1, 2])), Err(ParseError::Invalid(_))));
        // No `code` but data: accepted.
        assert!(parse(&json!({"data": []})).unwrap().is_empty());
    }
}
