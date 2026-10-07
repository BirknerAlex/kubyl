//! The alerts cache's data without the UI: counts, phases, sources, locked Alertmanagers, node
//! names, and reading and merging every source on Tokio. `kubyl_alerts::service::AlertsService`
//! keeps one entry per cluster and polls.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use kubyl_base::SharedString;
use kubyl_metrics_core::prometheus::{PromClient, PromError};
use secrecy::SecretString;
use serde_json::Value;

use crate::client::{self, AmConn};
use crate::discover::AmTarget;
use crate::matchers::Matcher;
use crate::merge::{self, MergeOptions};
use crate::model::{
    self, Alert, AlertState, ParseOptions, PromAlert, RuleGroup, Severity, Silence,
};
use crate::settings::AlertsSettings;

/// A view that asked within this long keeps the fast pace.
pub const DEMAND_TTL: Duration = Duration::from_secs(30);
/// Discovery runs again this often while nothing was found.
pub const REDISCOVER: Duration = Duration::from_secs(300);
/// Consecutive failed fetches before the sources are looked for again.
pub const MAX_FAILURES: u32 = 3;
pub const RULES_EVERY: Duration = Duration::from_secs(120);
/// Resolved alerts stay listed this long.
pub const RESOLVED_KEEP: Duration = Duration::from_secs(900);
/// At most one toast per cluster this often (the rest are batched).
pub const NOTIFY_EVERY: Duration = Duration::from_secs(60);
/// Silences take a moment to spread between HA replicas: read again this long after a write.
pub const AFTER_WRITE: Duration = Duration::from_secs(2);
/// How long discovery waits for phase 07 to find Prometheus.
pub const METRICS_WAIT: Duration = Duration::from_secs(20);
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);

/// Keychain entry of the Authorization header of an external Alertmanager URL.
pub fn auth_key(cluster: &str, url: &str) -> String {
    format!("alerts-auth:{cluster}/{url}")
}

/// An Alertmanager that wants a username and password before it can be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Locked {
    pub target: AmTarget,
    /// Signing in.
    pub busy: bool,
    /// The keychain has credentials (the forward was stopped, or the server didn't answer):
    /// reconnecting may work.
    pub saved: bool,
    /// Why the last attempt failed.
    pub problem: Option<SharedString>,
}

impl Locked {
    pub fn new(target: AmTarget) -> Self {
        Self {
            target,
            busy: false,
            saved: false,
            problem: None,
        }
    }

    pub fn label(&self) -> String {
        self.target.label()
    }
}

/// Where a cluster's alerts stand.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Not looked at yet (or the cluster isn't connected).
    Unknown,
    /// Alerts are off (`alerts.enabled`, or the cluster's `disabled`).
    Disabled,
    /// Looking for Alertmanager and Prometheus.
    Discovering,
    /// At least one source.
    Ready,
    /// No Alertmanager and no Prometheus rules API: see `tried` and `notes`.
    NoSource,
}

/// One Alertmanager and how its last read went.
#[derive(Clone, Debug)]
pub struct Source {
    pub conn: AmConn,
    pub error: Option<SharedString>,
    pub receivers: Vec<String>,
}

impl Source {
    pub fn label(&self) -> String {
        self.conn.label()
    }
}

/// Firing, pending, silenced… counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub critical: usize,
    pub warning: usize,
    pub info: usize,
    /// Firing with another (or no) severity.
    pub other: usize,
    pub pending: usize,
    pub silenced: usize,
    pub inhibited: usize,
}

impl Counts {
    pub fn of(alerts: &[Alert]) -> Self {
        let mut counts = Counts::default();
        for alert in alerts {
            match alert.state {
                AlertState::Firing | AlertState::Unprocessed => match alert.severity {
                    Severity::Critical => counts.critical += 1,
                    Severity::Warning => counts.warning += 1,
                    Severity::Info => counts.info += 1,
                    _ => counts.other += 1,
                },
                AlertState::Pending => counts.pending += 1,
                AlertState::Silenced => counts.silenced += 1,
                AlertState::Inhibited => counts.inhibited += 1,
                AlertState::Resolved => {}
            }
        }
        counts
    }

    pub fn firing(&self) -> usize {
        self.critical + self.warning + self.info + self.other
    }

    /// The most severe firing severity.
    pub fn worst(&self) -> Option<Severity> {
        if self.critical > 0 {
            Some(Severity::Critical)
        } else if self.warning > 0 {
            Some(Severity::Warning)
        } else if self.info > 0 {
            Some(Severity::Info)
        } else if self.other > 0 {
            Some(Severity::None)
        } else {
            None
        }
    }
}

/// Node names by name and InternalIP, to map `instance` labels to nodes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeMap(HashMap<String, String>);

impl NodeMap {
    /// From a node list (`/api/v1/nodes`).
    pub fn from_list(items: &[Value]) -> Self {
        let mut map = HashMap::new();
        for node in items {
            let Some(name) = node.pointer("/metadata/name").and_then(Value::as_str) else {
                continue;
            };
            map.insert(name.to_string(), name.to_string());
            for address in node
                .pointer("/status/addresses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(value) = address["address"].as_str() {
                    map.insert(value.to_string(), name.to_string());
                }
            }
        }
        Self(map)
    }

    /// The node an `instance` (`10.0.1.5:9100`, `node-1`, `[fd00::1]:9100`) names.
    pub fn node_of(&self, instance: &str) -> Option<String> {
        if let Some(name) = self.0.get(instance) {
            return Some(name.clone());
        }
        let host = match instance.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => instance,
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        self.0.get(host).cloned()
    }
}

/// Views asked with this pace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// A view shows the alerts: `refresh_interval`.
    View,
    /// Badges, the status bar: `background_refresh_interval`.
    Background,
}

/// Authorization headers for `urls` from the keychain (first id with an entry wins). The
/// keychain key uses the URL as written in settings; the result is keyed by the API URL the
/// probe compares against (with `path`, without a trailing slash).
pub async fn read_headers(
    secrets: &kubyl_kube_core::auth::Credentials,
    ids: &[String],
    urls: &[(String, String)],
) -> Vec<(String, SecretString)> {
    if urls.is_empty() {
        return Vec::new();
    }
    let ids = ids.to_vec();
    let urls = urls.to_vec();
    let secrets = secrets.clone();
    tokio::task::spawn_blocking(move || {
        headers_for(&ids, &urls, |key| {
            secrets
                .get(key)
                .inspect_err(|e| tracing::warn!("keychain: {e}"))
                .ok()
                .flatten()
        })
    })
    .await
    .unwrap_or_default()
}

pub fn headers_for(
    ids: &[String],
    urls: &[(String, String)],
    get: impl Fn(&str) -> Option<SecretString>,
) -> Vec<(String, SecretString)> {
    urls.iter()
        .filter_map(|(raw, api)| {
            ids.iter()
                .find_map(|id| get(&auth_key(id, raw)))
                .map(|header| (api.clone(), header))
        })
        .collect()
}

pub async fn read_nodes(client: &kube::Client) -> NodeMap {
    let Ok(request) = http::Request::get("/api/v1/nodes").body(Vec::new()) else {
        return NodeMap::default();
    };
    match client.request::<Value>(request).await {
        Ok(list) => NodeMap::from_list(
            list["items"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        ),
        Err(_) => NodeMap::default(),
    }
}

/// The number of a Service's named port.
pub async fn service_port(
    client: &kube::Client,
    namespace: &str,
    service: &str,
    name: &str,
) -> Option<u16> {
    let request = http::Request::get(format!("/api/v1/namespaces/{namespace}/services/{service}"))
        .body(Vec::new())
        .ok()?;
    let svc: Value = client.request(request).await.ok()?;
    svc.pointer("/spec/ports")?
        .as_array()?
        .iter()
        .find(|p| p["name"].as_str() == Some(name))
        .and_then(|p| p["port"].as_u64())
        .and_then(|p| u16::try_from(p).ok())
}

pub struct FetchInput {
    pub conns: Vec<AmConn>,
    pub prom: Option<PromClient>,
    pub want_rules: bool,
    pub rules: Arc<Vec<RuleGroup>>,
    pub matchers: Vec<Matcher>,
    pub settings: AlertsSettings,
    pub nodes: Arc<NodeMap>,
    pub prometheus_alertmanagers: Option<usize>,
}

pub struct FetchOutput {
    /// Per Alertmanager: its receivers when read this time, or its error.
    pub sources: Vec<Result<Option<Vec<String>>, String>>,
    /// Signed-in Alertmanagers that refused their saved username and password, with the
    /// forward port the read went through.
    pub rejected: Vec<(String, u16)>,
    pub silences: Vec<Silence>,
    /// False when a silences read failed: keep the previous list (`error` says why).
    pub silences_ok: bool,
    pub rules: Option<Result<Vec<RuleGroup>, String>>,
    /// `None`: nothing answered (keep the last data).
    pub merged: Option<merge::Merged>,
    pub error: Option<String>,
    pub now: Timestamp,
}

/// Reads every source and merges (on the Tokio runtime).
pub async fn fetch(input: FetchInput) -> FetchOutput {
    let matchers = &input.matchers;
    let reads = input.conns.iter().map(|conn| async move {
        let (alerts, silences, receivers) = futures::join!(
            conn.alerts(matchers),
            conn.silences(matchers),
            conn.receivers()
        );
        (alerts, silences, receivers)
    });
    let prom_reads = async {
        let Some(prom) = &input.prom else {
            return (None, None);
        };
        let alerts = prom.api("/api/v1/alerts", &[]);
        let rules = async {
            if input.want_rules {
                Some(
                    prom.api(
                        "/api/v1/rules",
                        &[
                            ("type", "alert".to_string()),
                            ("exclude_alerts", "true".to_string()),
                        ],
                    )
                    .await,
                )
            } else {
                None
            }
        };
        let (alerts, rules) = futures::join!(alerts, rules);
        (Some(alerts), rules)
    };
    let (am, (prom_alerts, rules)) = futures::join!(futures::future::join_all(reads), prom_reads);

    let severities = input.settings.severities.clone();
    let nodes = input.nodes.clone();
    let node_of = move |instance: &str| nodes.node_of(instance);
    let parse = ParseOptions {
        severity_label: &input.settings.severity_label,
        severities: &severities,
        node_of: &node_of,
    };
    let mut errors = Vec::new();
    let mut am_alerts: Vec<Alert> = Vec::new();
    let mut silences: Vec<Silence> = Vec::new();
    let mut sources = Vec::new();
    let mut am_ok = false;
    let mut silences_ok = true;
    let mut rejected = Vec::new();
    for (conn, (alerts, silence_list, receivers)) in input.conns.iter().zip(am) {
        let label = conn.label();
        match alerts {
            Ok(body) => {
                am_ok = true;
                am_alerts.extend(model::parse_am_alerts(&body, &label, &parse));
                match silence_list {
                    Ok(body) => silences.extend(model::parse_silences(&body, &label)),
                    Err(err) => {
                        silences_ok = false;
                        let message = client::explain(&conn.target, via_name(conn), &err);
                        errors.push(format!("{label}: silences: {message}"));
                    }
                }
                sources.push(Ok(receivers.ok().map(|b| model::parse_receivers(&b))));
            }
            Err(err) => {
                if let client::Via::Password { local_port } = conn.via
                    && matches!(err, PromError::Http(401, _))
                {
                    rejected.push((label.clone(), local_port));
                }
                let message = client::explain(&conn.target, via_name(conn), &err);
                errors.push(format!("{label}: {message}"));
                sources.push(Err(message));
            }
        }
    }
    let prom_alerts: Option<Result<Vec<PromAlert>, String>> = prom_alerts.map(|r| {
        r.map_err(|e| e.to_string())
            .and_then(|b| model::parse_prom_alerts(&b))
    });
    let rules: Option<Result<Vec<RuleGroup>, String>> = rules.map(|r| {
        r.map_err(|e| e.to_string())
            .and_then(|b| model::parse_rules(&b))
    });
    if let Some(Err(err)) = &prom_alerts {
        errors.push(format!("Prometheus: {err}"));
    }
    let current_rules: &[RuleGroup] = match &rules {
        Some(Ok(rules)) => rules,
        _ => &input.rules,
    };
    let now = Timestamp::now();
    let prom_ok = matches!(prom_alerts, Some(Ok(_)));
    let merged = if can_merge(input.conns.len(), am_ok, prom_ok) {
        let options = MergeOptions {
            heartbeat_alerts: &input.settings.heartbeat_alerts,
            hidden_alerts: &input.settings.hidden_alerts,
            parse: &parse,
        };
        let prom_list = match &prom_alerts {
            Some(Ok(list)) => Some(list.as_slice()),
            _ => None,
        };
        Some(merge::merge(
            (!input.conns.is_empty()).then_some(am_alerts.as_slice()),
            prom_list,
            current_rules,
            input.prometheus_alertmanagers,
            &options,
            now,
        ))
    } else {
        None
    };
    silences.sort_by_key(|s| std::cmp::Reverse(s.starts_at));
    FetchOutput {
        sources,
        rejected,
        silences,
        silences_ok,
        rules,
        merged,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
        now,
    }
}

/// Whether a read is complete enough to replace the last data. With Alertmanagers configured
/// and none answering, a Prometheus-only merge would drop their alerts (a resolved/started
/// notification storm) and show silenced ones as firing: keep the previous state instead.
pub fn can_merge(alertmanagers: usize, am_ok: bool, prom_ok: bool) -> bool {
    if alertmanagers > 0 { am_ok } else { prom_ok }
}

pub fn via_name(conn: &AmConn) -> &'static str {
    match conn.via {
        client::Via::Proxy => "proxy",
        client::Via::Route { .. } => "route",
        client::Via::Forward { .. } | client::Via::Password { .. } => "forward",
        client::Via::Url => "url",
    }
}

/// Severity counts by namespace (`None`: every namespace).
pub fn namespace_counts(alerts: &[Alert], namespace: Option<&str>) -> Counts {
    match namespace {
        None => Counts::of(alerts),
        Some(ns) => {
            let scoped: Vec<Alert> = alerts
                .iter()
                .filter(|a| a.namespace() == Some(ns))
                .cloned()
                .collect();
            Counts::of(&scoped)
        }
    }
}
