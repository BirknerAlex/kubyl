//! The `"cost"` section of settings.json (where OpenCost is, when detection can't tell) and
//! what the view remembers per cluster (state.json: window and include-idle).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use kubyl_settings_core::{SettingsSection, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::window::Window;

/// Where OpenCost's API is: `namespace/service:port`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub namespace: String,
    pub service: String,
    /// A number or a port name.
    pub port: String,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}:{}", self.namespace, self.service, self.port)
    }
}

impl FromStr for Endpoint {
    type Err = String;

    /// `opencost/opencost:9003` (the port is optional: 9003).
    fn from_str(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (namespace, rest) = text
            .split_once('/')
            .ok_or_else(|| format!("`{text}` isn't namespace/service[:port]"))?;
        let (service, port) = rest.split_once(':').unwrap_or((rest, "9003"));
        let valid = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        };
        if !valid(namespace) || !valid(service) || !valid(port) {
            return Err(format!("`{text}` isn't namespace/service[:port]"));
        }
        Ok(Endpoint {
            namespace: namespace.into(),
            service: service.into(),
            port: port.into(),
        })
    }
}

/// Settings for one cluster.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ClusterCost {
    /// OpenCost's API Service as `namespace/service:port` (default port 9003), for installs
    /// detection doesn't find. Reached through the API server's service proxy.
    pub service: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CostSettings {
    /// Per cluster: the context name, or the cluster id.
    pub clusters: BTreeMap<String, ClusterCost>,
}

impl SettingsSection for CostSettings {
    const KEY: Option<&'static str> = Some("cost");
}

impl CostSettings {
    /// The configured endpoint of a cluster, looked up under each of its settings keys. An
    /// invalid one is an error the view shows.
    pub fn endpoint(&self, keys: &[String]) -> Option<Result<Endpoint, String>> {
        keys.iter()
            .find_map(|k| self.clusters.get(k)?.service.as_deref())
            .map(str::parse)
    }
}

/// What the Cost view remembers for a cluster.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub window: Window,
    pub include_idle: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            window: Window::Day,
            include_idle: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostState {
    /// By the cluster's stable key (the context name; `<cluster>/<user>` for grouped entries).
    pub clusters: BTreeMap<String, Prefs>,
}

impl StateSection for CostState {
    const KEY: &'static str = "cost";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_parse_and_print() {
        let e: Endpoint = "finops/opencost:9003".parse().unwrap();
        assert_eq!(
            (e.namespace.as_str(), e.service.as_str(), e.port.as_str()),
            ("finops", "opencost", "9003")
        );
        assert_eq!(e.to_string(), "finops/opencost:9003");
        assert_eq!("finops/cost".parse::<Endpoint>().unwrap().port, "9003");
        assert_eq!("a/b:http".parse::<Endpoint>().unwrap().port, "http");
        for bad in [
            "opencost",
            "/x:1",
            "a/:1",
            "a/b c:1",
            "a/b:1/../x",
            "a/b?x=1:1",
            "",
        ] {
            assert!(bad.parse::<Endpoint>().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_override_is_found_under_any_key() {
        let settings: CostSettings = serde_json::from_str(
            r#"{"clusters": {"prod": {"service": "finops/opencost:9003"}, "bad": {"service": "nonsense"}}}"#,
        )
        .unwrap();
        let keys = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(
            settings
                .endpoint(&keys(&["prod@/f", "prod"]))
                .unwrap()
                .unwrap()
                .to_string(),
            "finops/opencost:9003"
        );
        assert!(settings.endpoint(&keys(&["bad"])).unwrap().is_err());
        assert!(settings.endpoint(&keys(&["other"])).is_none());
    }

    #[test]
    fn prefs_default_to_a_day_with_idle() {
        let state: CostState =
            serde_json::from_str(r#"{"clusters": {"kind": {"window": "7d"}}}"#).unwrap();
        let prefs = state.clusters["kind"];
        assert_eq!(prefs.window, Window::Week);
        assert!(prefs.include_idle, "a missing value takes the default");
        assert_eq!(Prefs::default().window, Window::Day);
    }
}
