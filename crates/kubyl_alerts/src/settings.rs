//! The `"alerts"` section of settings.json, and the view options kept in state.json (never
//! alert data).

use std::collections::BTreeMap;
use std::time::Duration;

use kubyl_settings::{SettingsSection, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::Severity;

/// When the sidebar shows a cluster's "Alerts" row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SidebarMode {
    /// When the cluster has an alert source.
    #[default]
    Auto,
    Always,
    Never,
}

/// Which clusters notify about new alerts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyClusters {
    /// The active cluster.
    Active,
    /// Clusters of favorites.
    Favorites,
    /// Clusters marked production.
    #[default]
    Production,
    /// Every connected cluster with an alert source.
    All,
}

/// Toasts for alerts that start firing (opt-in).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct NotifySettings {
    pub enabled: bool,
    /// `critical`, `warning`, `info` or `none`.
    pub min_severity: String,
    pub clusters: NotifyClusters,
    /// Also when they resolve.
    pub resolved: bool,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            min_severity: "critical".into(),
            clusters: NotifyClusters::Production,
            resolved: false,
        }
    }
}

/// An Alertmanager named in settings: a Service (reached through the API server) or an
/// external URL. Services named here may receive your token when they sit behind an auth proxy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AlertmanagerConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Port number or name. Default: `web`/`http-web`/`http`, else 9093.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// `http` (default) or `https`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// API path prefix, e.g. `/am` (Alertmanager's `routePrefix`) or `/alertmanager` (Mimir).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `namespace/name` of a service account whose short-lived token is used behind an auth
    /// proxy when yours can't be.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account: Option<String>,
    /// `X-Scope-OrgID` for Mimir and Cortex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// An external Alertmanager instead of a Service. Its Authorization header is kept in the
    /// OS keychain ("Alerts: Set Alertmanager Authorization Header…"), never in this file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// CA certificate file (PEM) for `url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<String>,
    /// Client certificate and key files (PEM) for mutual TLS. The key stays in its file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_certificate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// Accept any certificate from `url`. The Authorization header isn't sent then (use
    /// `ca_file`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub insecure_skip_tls_verify: bool,
}

/// Alerts of one cluster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ClusterSettings {
    /// No alerts for this cluster.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Look for Alertmanager (ignored when `alertmanagers` names some).
    pub discover: bool,
    /// Alertmanagers to use instead of discovery.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alertmanagers: Vec<AlertmanagerConfig>,
    /// Matchers that select this cluster's alerts on a central Alertmanager, e.g.
    /// `cluster="prod-eu-1"`. Silences always include them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matchers: Vec<String>,
    /// Read rules and pending alerts from the cluster's Prometheus.
    pub rules: bool,
}

impl Default for ClusterSettings {
    fn default() -> Self {
        Self {
            disabled: false,
            discover: true,
            alertmanagers: Vec::new(),
            matchers: Vec::new(),
            rules: true,
        }
    }
}

/// Alerts (Alertmanager), silences and alerting rules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AlertsSettings {
    pub enabled: bool,
    /// Look for Alertmanager by itself.
    pub discover: bool,
    /// Seconds between refreshes while a view shows a cluster's alerts.
    pub refresh_interval: u64,
    /// Seconds between refreshes for badges, the status bar and notifications.
    pub background_refresh_interval: u64,
    /// `auto` (when the cluster has an alert source), `always` or `never`.
    pub sidebar: SidebarMode,
    /// The label that holds the severity.
    pub severity_label: String,
    /// Other severity values: `{"page": "critical", "high": "warning"}`.
    pub severities: BTreeMap<String, String>,
    /// Alerts that always fire to prove the pipeline works (not problems).
    pub heartbeat_alerts: Vec<String>,
    /// Alerts left out of every list.
    pub hidden_alerts: Vec<String>,
    /// How long "Acknowledge" silences an alert (`1h`, `30m`, `1d`).
    pub ack_duration: String,
    pub notify: NotifySettings,
    /// Per cluster, keyed by cluster id or context name.
    pub clusters: BTreeMap<String, ClusterSettings>,
}

impl Default for AlertsSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            discover: true,
            refresh_interval: 15,
            background_refresh_interval: 60,
            sidebar: SidebarMode::Auto,
            severity_label: "severity".into(),
            severities: BTreeMap::new(),
            heartbeat_alerts: vec!["Watchdog".into()],
            hidden_alerts: vec!["InfoInhibitor".into()],
            ack_duration: "1h".into(),
            notify: NotifySettings::default(),
            clusters: BTreeMap::new(),
        }
    }
}

impl SettingsSection for AlertsSettings {
    const KEY: Option<&'static str> = Some("alerts");
}

impl AlertsSettings {
    /// The settings of a cluster by the first of its keys that has some
    /// (`ConnectionManager::settings_keys`: entry id, member ids, context names).
    pub fn cluster(&self, keys: &[String]) -> ClusterSettings {
        keys.iter()
            .find_map(|key| self.clusters.get(key))
            .cloned()
            .unwrap_or_default()
    }

    pub fn refresh(&self) -> Duration {
        Duration::from_secs(self.refresh_interval.max(5))
    }

    pub fn background_refresh(&self) -> Duration {
        Duration::from_secs(self.background_refresh_interval.max(15))
    }

    pub fn min_notify_severity(&self) -> Severity {
        Severity::from_value(&self.notify.min_severity, &BTreeMap::new())
    }

    /// `ack_duration` (default one hour).
    pub fn ack(&self) -> Duration {
        parse_duration(&self.ack_duration).unwrap_or(Duration::from_secs(3600))
    }
}

/// `90s`, `30m`, `1h`, `2h30m`, `1d`, `1w`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut total = 0u64;
    let mut number = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: u64 = number.parse().ok()?;
        number.clear();
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return None,
        };
        // Typed input (the silence editor's custom end): overflow is invalid, not a panic.
        total = total.checked_add(n.checked_mul(unit)?)?;
    }
    if !number.is_empty() {
        return None;
    }
    (total > 0).then(|| Duration::from_secs(total))
}

/// How the Alerts table groups rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    #[default]
    Name,
    Namespace,
    Severity,
    Receiver,
    Target,
    None,
}

impl GroupBy {
    pub const ALL: [GroupBy; 6] = [
        GroupBy::Name,
        GroupBy::Namespace,
        GroupBy::Severity,
        GroupBy::Receiver,
        GroupBy::Target,
        GroupBy::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::Name => "alert name",
            GroupBy::Namespace => "namespace",
            GroupBy::Severity => "severity",
            GroupBy::Receiver => "receiver",
            GroupBy::Target => "target",
            GroupBy::None => "nothing",
        }
    }
}

/// The Alerts view's options (state.json `alerts`). Never alert data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertsState {
    pub group_by: GroupBy,
    /// Silenced and inhibited alerts are listed (after the active ones), marked (default).
    pub show_suppressed: bool,
    /// Severity chips turned on (`critical`…); empty = all.
    pub severities: Vec<String>,
    /// State chips turned on (`firing`, `pending`, `silenced`, `inhibited`); empty = all.
    pub states: Vec<String>,
    /// Only the active namespace (alerts without a namespace show under "Cluster").
    pub active_namespace_only: bool,
    /// Rules tab: only firing, pending or failing rules.
    pub rules_problems_only: bool,
}

impl Default for AlertsState {
    fn default() -> Self {
        Self {
            group_by: GroupBy::default(),
            show_suppressed: true,
            severities: Vec::new(),
            states: Vec::new(),
            active_namespace_only: false,
            rules_problems_only: false,
        }
    }
}

impl StateSection for AlertsState {
    const KEY: &'static str = "alerts";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_cluster_keys() {
        let settings: AlertsSettings = serde_json::from_str(
            r#"{"clusters": {"shop/c/u@/k": {"matchers": ["cluster=\"eu\""]}, "kind-dev": {"disabled": true}}}"#,
        )
        .unwrap();
        assert_eq!(settings.refresh_interval, 15);
        assert_eq!(settings.heartbeat_alerts, ["Watchdog"]);
        let keys = [
            "group:c,u@/k/".to_string(),
            "shop/c/u@/k".to_string(),
            "shop/c/u".to_string(),
        ];
        assert_eq!(settings.cluster(&keys).matchers, ["cluster=\"eu\""]);
        assert!(settings.cluster(&["kind-dev".into()]).disabled);
        assert!(settings.cluster(&["other".into()]).discover);
        assert_eq!(settings.ack(), Duration::from_secs(3600));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("2h30m"), Some(Duration::from_secs(9000)));
        assert_eq!(parse_duration("1w"), Some(Duration::from_secs(604_800)));
        assert_eq!(parse_duration("1x"), None);
        assert_eq!(parse_duration("99999999999999w"), None, "overflow");
        assert_eq!(
            parse_duration("18446744073709551615s1s"),
            None,
            "overflow in the sum"
        );
        assert_eq!(parse_duration("15"), None);
        assert_eq!(parse_duration(""), None);
    }
}
