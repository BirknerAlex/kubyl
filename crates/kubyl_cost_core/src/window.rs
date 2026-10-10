//! The time windows and the allocation query.

use serde::{Deserialize, Serialize};

/// How far back the view looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Window {
    #[default]
    #[serde(rename = "24h")]
    Day,
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
}

impl Window {
    pub const ALL: [Window; 3] = [Window::Day, Window::Week, Window::Month];

    /// OpenCost's `window` parameter.
    pub fn param(self) -> &'static str {
        match self {
            Window::Day => "24h",
            Window::Week => "7d",
            Window::Month => "30d",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Window::Day => "24 hours",
            Window::Week => "7 days",
            Window::Month => "30 days",
        }
    }

    /// The chart's bucket: hourly over a day, daily beyond.
    pub fn step(self) -> &'static str {
        match self {
            Window::Day => "1h",
            Window::Week | Window::Month => "1d",
        }
    }

    pub fn step_seconds(self) -> i64 {
        match self {
            Window::Day => 3600,
            Window::Week | Window::Month => 86_400,
        }
    }

    pub fn seconds(self) -> i64 {
        match self {
            Window::Day => 86_400,
            Window::Week => 7 * 86_400,
            Window::Month => 30 * 86_400,
        }
    }

    pub fn from_param(text: &str) -> Option<Window> {
        Window::ALL.into_iter().find(|w| w.param() == text)
    }
}

/// An allocation request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Query {
    pub window: Window,
    /// Idle capacity as its own allocation (`__idle__`); without it OpenCost leaves idle
    /// cost out of the totals.
    pub include_idle: bool,
    /// One allocation per namespace for the whole window, or one set per step (the chart).
    pub accumulate: bool,
}

impl Query {
    /// The URL parameters, always aggregated by namespace.
    pub fn params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![
            ("window", self.window.param().to_string()),
            ("aggregate", "namespace".to_string()),
            ("includeIdle", self.include_idle.to_string()),
            ("accumulate", self.accumulate.to_string()),
        ];
        if !self.accumulate {
            params.push(("step", self.window.step().to_string()));
        }
        params
    }
}

/// The path of the allocation API.
pub const ALLOCATION_PATH: &str = "/allocation/compute";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_and_their_parameters() {
        assert_eq!(Window::Day.param(), "24h");
        assert_eq!(Window::from_param("30d"), Some(Window::Month));
        assert_eq!(Window::from_param("1h"), None);
        assert_eq!(Window::Day.step(), "1h");
        assert_eq!(Window::Week.step(), "1d");
        assert_eq!(Window::Month.seconds() / Window::Month.step_seconds(), 30);
        assert_eq!(serde_json::to_string(&Window::Week).unwrap(), "\"7d\"");
        assert_eq!(
            serde_json::from_str::<Window>("\"30d\"").unwrap(),
            Window::Month
        );
    }

    #[test]
    fn queries_aggregate_by_namespace_and_step_only_for_the_chart() {
        let totals = Query {
            window: Window::Week,
            include_idle: true,
            accumulate: true,
        }
        .params();
        assert!(totals.contains(&("window", "7d".into())));
        assert!(totals.contains(&("aggregate", "namespace".into())));
        assert!(totals.contains(&("includeIdle", "true".into())));
        assert!(totals.contains(&("accumulate", "true".into())));
        assert!(totals.iter().all(|(k, _)| *k != "step"));
        let chart = Query {
            window: Window::Day,
            include_idle: false,
            accumulate: false,
        }
        .params();
        assert!(chart.contains(&("step", "1h".into())));
        assert!(chart.contains(&("includeIdle", "false".into())));
        assert!(chart.contains(&("accumulate", "false".into())));
    }
}
