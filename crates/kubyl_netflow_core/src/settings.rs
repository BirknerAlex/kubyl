//! The `"netflow"` section of settings.json and the view options in state.json. Neither ever
//! holds flows or filters.

use std::collections::BTreeMap;
use std::path::PathBuf;

use kubyl_settings_core::{SettingsSection, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::provider::BackendKind;

/// Network flow settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NetflowSettings {
    /// Show the Network Flows row under every cluster.
    pub sidebar: bool,
    /// Flows kept in memory per stream.
    pub max_flows: usize,
    /// Minutes of flows kept in memory per stream.
    pub max_age_minutes: u64,
    /// Keep the values of URL query parameters in HTTP flows. Off: `?token=…` (they can hold
    /// credentials).
    pub keep_query_values: bool,
    /// Per cluster (entry id, member id or context name).
    pub clusters: BTreeMap<String, ClusterNetflowSettings>,
}

impl Default for NetflowSettings {
    fn default() -> Self {
        Self {
            sidebar: true,
            max_flows: 20_000,
            max_age_minutes: 60,
            keep_query_values: false,
            clusters: BTreeMap::new(),
        }
    }
}

impl SettingsSection for NetflowSettings {
    const KEY: Option<&'static str> = Some("netflow");
}

impl NetflowSettings {
    /// A cluster's overrides, looked up by its settings keys (`ConnectionManager::settings_keys`).
    pub fn for_keys(&self, keys: &[String]) -> ClusterNetflowSettings {
        keys.iter()
            .find_map(|k| self.clusters.get(k))
            .cloned()
            .unwrap_or_default()
    }
}

/// One cluster's overrides.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ClusterNetflowSettings {
    /// The backend to use when detection picks another one, or `off`.
    pub backend: Option<BackendSetting>,
    pub hubble: Option<HubbleSettings>,
    pub whisker: Option<ServiceSettings>,
    pub loki: Option<LokiSettings>,
}

/// A backend picked in settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackendSetting {
    Hubble,
    Whisker,
    Netobserv,
    Off,
}

impl BackendSetting {
    pub fn kind(self) -> Option<BackendKind> {
        match self {
            BackendSetting::Hubble => Some(BackendKind::Hubble),
            BackendSetting::Whisker => Some(BackendKind::Whisker),
            BackendSetting::Netobserv => Some(BackendKind::NetObserv),
            BackendSetting::Off => None,
        }
    }

    pub fn of(kind: BackendKind) -> Self {
        match kind {
            BackendKind::Hubble => BackendSetting::Hubble,
            BackendKind::Whisker => BackendSetting::Whisker,
            BackendKind::NetObserv => BackendSetting::Netobserv,
        }
    }
}

/// Where Hubble Relay is, when it isn't `kube-system/hubble-relay`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HubbleSettings {
    pub namespace: Option<String>,
    pub service: Option<String>,
    /// The Service port (80 plain, 443 TLS by default).
    pub port: Option<u16>,
    /// Relay serves TLS (read from its `hubble-relay-config` ConfigMap when unset).
    pub tls: Option<bool>,
    /// A ConfigMap with the CA that signed Relay's certificate, `namespace/name` (default: the
    /// Relay's namespace, `cilium-root-ca.crt`).
    pub ca_config_map: Option<String>,
    /// The key in that ConfigMap (default `ca.crt`).
    pub ca_key: Option<String>,
    /// Or a PEM file on this machine.
    pub ca_file: Option<PathBuf>,
    /// The name Relay's certificate must have (default `relay.hubble-relay.cilium.io`).
    pub server_name: Option<String>,
}

/// A Service reached through the API server's service proxy.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ServiceSettings {
    pub namespace: Option<String>,
    pub service: Option<String>,
    pub port: Option<u16>,
}

/// NetObserv's Loki, when the FlowCollector doesn't say or says something Kubyl can't reach.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LokiSettings {
    /// A Service through the service proxy.
    pub namespace: Option<String>,
    pub service: Option<String>,
    pub port: Option<String>,
    pub scheme: Option<String>,
    /// A path prefix (`/loki` for some gateways).
    pub path: Option<String>,
    /// Or a URL Kubyl calls directly (no credentials are sent).
    pub url: Option<String>,
}

/// View options kept in state.json.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetflowState {
    /// The time window (`15m`, `1h`…).
    pub window: String,
    /// Topology zoom (`namespaces`, `workloads`).
    pub zoom: String,
    pub details_open: bool,
}

impl Default for NetflowState {
    fn default() -> Self {
        Self {
            window: "15m".into(),
            zoom: "namespaces".into(),
            details_open: true,
        }
    }
}

impl StateSection for NetflowState {
    const KEY: &'static str = "netflow";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_by_settings_key() {
        let settings: NetflowSettings = serde_json::from_value(serde_json::json!({
            "clusters": {
                "kind-kubyl-cilium": {
                    "backend": "hubble",
                    "hubble": { "namespace": "cilium", "service": "relay" }
                },
                "homelab": { "backend": "off" }
            }
        }))
        .unwrap();
        assert_eq!(settings.max_flows, 20_000);
        assert!(settings.sidebar && !settings.keep_query_values);
        let cilium = settings.for_keys(&["group:x".into(), "kind-kubyl-cilium".into()]);
        assert_eq!(cilium.backend, Some(BackendSetting::Hubble));
        assert_eq!(cilium.hubble.unwrap().namespace.as_deref(), Some("cilium"));
        assert_eq!(
            settings
                .for_keys(&["homelab".into()])
                .backend
                .unwrap()
                .kind(),
            None
        );
        assert_eq!(
            settings.for_keys(&["nope".into()]),
            ClusterNetflowSettings::default()
        );
    }
}
