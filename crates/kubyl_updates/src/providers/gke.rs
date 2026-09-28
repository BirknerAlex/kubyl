//! Google GKE (`updates-gke`).
//!
//! The project, location and cluster come from `updates.clusters.<id>.gke` or the
//! `gke_<project>_<location>_<cluster>` context and cluster entry names that `gcloud container
//! clusters get-credentials` writes. The token comes from `gcloud auth print-access-token` (or
//! Application Default Credentials when the exec plugin uses them, or when gcloud has no
//! account). Reads the cluster, the location's `serverConfig` (channel versions) and running
//! operations; writes are `clusters.update` (master version, release channel) and
//! `nodePools.update`.

pub mod api;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::Method;
use jiff::Timestamp;
use serde_json::{Value, json};
use url::Url;

use self::api::{Api, Cluster, NodePool, Operation, ServerConfig};
use super::CloudContext;
use super::cloud::{self, Action, BearerToken, Cache, CliEnv, CliError, ExecInfo, GCLOUD, Http};
use crate::model::{
    Current, Note, Plan, Pool, PoolKind, PoolState, Progress, ProviderKind, Scope, Status, Target,
    TargetKind, Writes,
};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};
use crate::settings::GkeSettings;
use crate::version::{self, Version};

/// The id of the control plane pool.
pub const CONTROL_PLANE: &str = "control-plane";
/// The channel name Kubyl offers for "no channel" (`UNSPECIFIED` in the API).
pub const NO_CHANNEL: &str = "NONE";
const API: &str = "https://container.googleapis.com";
const DOCS_RELEASE_NOTES: &str = "https://cloud.google.com/kubernetes-engine/docs/release-notes";
const DOCS_CHANNELS: &str =
    "https://cloud.google.com/kubernetes-engine/docs/concepts/release-channels";
/// `gcloud auth print-access-token` doesn't say when its token expires; it hands out tokens
/// with a few minutes left at least.
const TOKEN_FOR: Duration = Duration::from_secs(3 * 60);

/// The GKE provider of a cluster. Reads [`cloud::GKE_ENDPOINT`] once.
pub fn provider(ctx: CloudContext) -> Arc<dyn UpdateProvider> {
    Arc::new(build(ctx, cloud::env_lookup, None))
}

/// The provider with the environment read by `lookup` and, in tests, a preset token.
fn build(
    ctx: CloudContext,
    lookup: impl Fn(&str) -> Option<String>,
    preset: Option<BearerToken>,
) -> Gke {
    let base = cloud::endpoint_override(cloud::GKE_ENDPOINT, lookup);
    Gke {
        inner: Arc::new(Inner::new(ctx, None, base, preset)),
    }
}

// ---------------------------------------------------------------------------------------------
// Which cluster

/// The cluster Kubyl talks to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GkeTarget {
    pub project: String,
    /// A region (`europe-west1`) or a zone (`europe-west1-b`).
    pub location: String,
    pub cluster: String,
    /// Use Application Default Credentials (the exec plugin does).
    pub adc: bool,
    /// The exec plugin's `CLOUDSDK_*` and `GOOGLE_APPLICATION_CREDENTIALS` variables.
    pub env: CliEnv,
}

impl GkeTarget {
    /// `/v1/projects/p/locations/l`.
    pub fn location_path(&self) -> String {
        format!(
            "/v1/projects/{}/locations/{}",
            cloud::encode(&self.project).replace("%3A", ":"),
            cloud::encode(&self.location)
        )
    }

    /// `/v1/projects/p/locations/l/clusters/c`.
    pub fn cluster_path(&self) -> String {
        format!(
            "{}/clusters/{}",
            self.location_path(),
            cloud::encode(&self.cluster)
        )
    }

    pub fn node_pool_path(&self, pool: &str) -> String {
        format!("{}/nodePools/{}", self.cluster_path(), cloud::encode(pool))
    }

    /// A zonal cluster (`europe-west1-b`): one control plane replica.
    pub fn zonal(&self) -> bool {
        self.location
            .rsplit('-')
            .next()
            .is_some_and(|last| last.len() == 1 && last.chars().all(|c| c.is_ascii_lowercase()))
    }

    /// `--region europe-west1 --project p`, for commands to copy.
    pub fn gcloud_flags(&self) -> String {
        let kind = if self.zonal() { "zone" } else { "region" };
        format!("--{kind} {} --project {}", self.location, self.project)
    }

    /// `Google GKE (GKE API · project p · europe-west1)`.
    pub fn label(&self) -> String {
        format!(
            "{} (GKE API · project {} · {})",
            ProviderKind::Gke.label(),
            self.project,
            self.location
        )
    }

    fn resource(&self) -> String {
        format!("cluster {}", self.cluster)
    }

    /// The command that renews the token.
    pub fn login(&self) -> String {
        if self.adc {
            "gcloud auth application-default login".into()
        } else {
            "gcloud auth login".into()
        }
    }
}

/// `gke_<project>_<location>_<cluster>`: project, location, cluster.
pub fn parse_name(name: &str) -> Option<(String, String, String)> {
    let mut parts = name.split('_');
    if parts.next()? != "gke" {
        return None;
    }
    let (project, location, cluster) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || project.is_empty() || location.is_empty() || cluster.is_empty() {
        return None;
    }
    Some((project.into(), location.into(), cluster.into()))
}

/// Picks project, location and cluster: settings first, then the context and cluster entry
/// names.
pub fn resolve(
    settings: Option<&GkeSettings>,
    exec: Option<&ExecInfo>,
    context: &str,
    cluster_entry: &str,
) -> Result<GkeTarget, ProviderError> {
    let named = [cluster_entry, context].into_iter().find_map(parse_name);
    let setting = |f: fn(&GkeSettings) -> &Option<String>| {
        settings
            .and_then(|s| f(s).clone())
            .filter(|v| !v.is_empty())
    };
    let project = setting(|s| &s.project).or_else(|| named.as_ref().map(|n| n.0.clone()));
    let location = setting(|s| &s.location).or_else(|| named.as_ref().map(|n| n.1.clone()));
    let cluster = setting(|s| &s.cluster).or_else(|| named.as_ref().map(|n| n.2.clone()));
    let (Some(project), Some(location), Some(cluster)) = (project, location, cluster) else {
        return Err(ProviderError::Other(format!(
            "Kubyl can't tell the GKE project, location and cluster from this context's name. \
             Set \"updates\": {{\"clusters\": {{\"{context}\": {{\"gke\": {{\"project\": …, \
             \"location\": …, \"cluster\": …}}}}}}}} in settings.json."
        )));
    };
    let adc = exec.is_some_and(|e| e.has_flag("--use_application_default_credentials"));
    let env = exec
        .map(|e| {
            e.env
                .filter(|n| n.starts_with("CLOUDSDK_") || n == "GOOGLE_APPLICATION_CREDENTIALS")
        })
        .unwrap_or_default();
    Ok(GkeTarget {
        project,
        location,
        cluster,
        adc,
        env,
    })
}

// ---------------------------------------------------------------------------------------------
// The token

/// `gcloud auth print-access-token` or the ADC variant.
pub fn token_args(adc: bool) -> &'static [&'static str] {
    if adc {
        &["auth", "application-default", "print-access-token"]
    } else {
        &["auth", "print-access-token"]
    }
}

/// The token gcloud printed.
pub fn parse_token(stdout: &[u8]) -> Result<BearerToken, String> {
    let text = String::from_utf8_lossy(stdout);
    let token = text.lines().map(str::trim).find(|l| !l.is_empty());
    match token {
        Some(token) if !token.contains(' ') => Ok(BearerToken(token.to_string().into())),
        _ => Err("gcloud printed no access token".into()),
    }
}

/// What a failed `gcloud` run means, and what fixes it.
pub fn classify(error: &CliError, adc: bool) -> ProviderError {
    let shown = format!("gcloud {}", token_args(adc).join(" "));
    let CliError::Failed { stderr, .. } = error else {
        return error.to_provider_error(GCLOUD, &shown, None);
    };
    let lower = stderr.to_lowercase();
    let summary = cloud::stderr_summary(stderr);
    if adc
        || lower.contains("application-default login")
        || lower.contains("default credentials were not found")
    {
        return ProviderError::Credentials {
            message: format!("Application Default Credentials aren't set up or expired: {summary}"),
            command: Some("gcloud auth application-default login".into()),
        };
    }
    if lower.contains("gcloud auth login")
        || lower.contains("active account")
        || lower.contains("reauthentication")
        || lower.contains("refresh")
        || lower.contains("invalid_grant")
    {
        return ProviderError::Credentials {
            message: format!("Your gcloud sign-in has expired or isn't set up: {summary}"),
            command: Some("gcloud auth login".into()),
        };
    }
    error.to_provider_error(GCLOUD, &shown, Some("gcloud auth login".into()))
}

async fn token_with(target: &GkeTarget, adc: bool) -> Result<BearerToken, ProviderError> {
    let stdout = cloud::run(GCLOUD, token_args(adc), &target.env)
        .await
        .map_err(|err| classify(&err, adc))?;
    parse_token(&stdout).map_err(ProviderError::Other)
}

/// A token like the exec plugin gets one: gcloud's account, else ADC.
async fn fetch_token(target: &GkeTarget) -> Result<(BearerToken, Timestamp), ProviderError> {
    let token = match token_with(target, target.adc).await {
        Ok(token) => token,
        Err(first @ ProviderError::Credentials { .. }) if !target.adc => {
            token_with(target, true).await.map_err(|_| first)?
        }
        Err(err) => return Err(err),
    };
    Ok((token, Timestamp::now() + TOKEN_FOR))
}

// ---------------------------------------------------------------------------------------------
// What GKE reports

/// Everything one read fetched.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cluster: Cluster,
    /// `None`: Kubyl couldn't read it.
    pub server_config: Option<ServerConfig>,
    /// The location's operations.
    pub operations: Vec<Operation>,
    pub notes: Vec<Note>,
}

/// The pool id of a node pool.
pub fn node_pool_id(name: &str) -> String {
    format!("nodepool/{name}")
}

fn newer(a: &str, b: &str) -> bool {
    version::compare(a, b) == std::cmp::Ordering::Greater
}

/// The targets GKE offers from `current`: the channel's valid versions (or the location's
/// master versions without a channel) that are newer. The channel's auto-upgrade target (else
/// its default) is recommended; versions skipping a minor are blocked. `busy` blocks all.
pub fn targets(
    current: &str,
    channel: Option<&str>,
    config: &ServerConfig,
    busy: Option<&str>,
) -> Vec<Target> {
    let Some(cur) = Version::parse(current) else {
        return Vec::new();
    };
    let channel_config = channel.and_then(|c| config.channel(c));
    let versions = match channel_config {
        Some(c) => &c.valid_versions,
        None => &config.valid_master_versions,
    };
    let mut newer_versions: Vec<&String> = versions.iter().filter(|v| newer(v, current)).collect();
    newer_versions.sort_by(|a, b| version::compare(b, a));
    newer_versions.dedup();
    let recommended = match channel_config {
        Some(c) => c
            .upgrade_target_version
            .as_ref()
            .filter(|v| newer(v, current))
            .or(c.default_version.as_ref().filter(|v| newer(v, current))),
        None => config
            .default_cluster_version
            .as_ref()
            .filter(|v| newer(v, current)),
    };
    let next = format!("{}.{}", cur.major, cur.minor.saturating_add(1));
    newer_versions
        .into_iter()
        .filter_map(|v| {
            let parsed = Version::parse(v)?;
            let steps = cur.minors_to(&parsed);
            let mut target = Target::new(v.clone(), TargetKind::Available);
            target.minor = steps >= 1;
            target.url = Some(DOCS_RELEASE_NOTES.into());
            target.channels = channel.map(str::to_string).into_iter().collect();
            if steps > 1 {
                target.kind = TargetKind::Blocked;
                target.blocked.push(format!(
                    "GKE control planes update one minor at a time ({} → {next} first)",
                    cur.minor_string()
                ));
            } else if let Some(reason) = busy {
                target.kind = TargetKind::Blocked;
                target.blocked.push(reason.to_string());
            } else if recommended == Some(v) {
                target.kind = TargetKind::Recommended;
            }
            Some(target)
        })
        .collect()
}

/// A GKE version in free text (`… to 1.31.1-gke.1678000.`).
fn version_in(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')' | '"' | '[' | ']'))
        .map(|word| word.trim_end_matches(['.', ':']))
        .find(|word| word.contains("-gke.") && Version::parse(word).is_some())
        .map(str::to_string)
}

/// The version an operation moves to: named in its detail or status message, else (for a node
/// pool) the control plane's version, which node pools update to.
pub fn operation_target(op: &Operation, master: &str) -> Option<String> {
    op.detail
        .iter()
        .chain(op.status_message.iter())
        .find_map(|text| version_in(text))
        .or_else(|| op.node_pool().map(|_| master.to_string()))
}

fn pool_state(status: Option<&str>) -> PoolState {
    match status.unwrap_or_default() {
        "RECONCILING" => PoolState::Updating,
        "PROVISIONING" => PoolState::Queued,
        "ERROR" | "DEGRADED" | "RUNNING_WITH_ERROR" => PoolState::Degraded,
        "STOPPING" => PoolState::Paused,
        _ => PoolState::Idle,
    }
}

/// `surge 1 · maxUnavailable 0`, `blue-green`.
fn surge(pool: &NodePool) -> Option<String> {
    let settings = pool.upgrade_settings.as_ref()?;
    if settings.strategy.as_deref() == Some("BLUE_GREEN") {
        return Some("blue-green".into());
    }
    Some(format!(
        "surge {} · maxUnavailable {}",
        settings.max_surge.unwrap_or(0),
        settings.max_unavailable.unwrap_or(0)
    ))
}

/// A node pool row.
pub fn node_pool(
    pool: &NodePool,
    master: &str,
    autopilot: bool,
    running: Option<&Operation>,
) -> Pool {
    let mut row = Pool::new(
        node_pool_id(&pool.name),
        pool.name.clone(),
        PoolKind::NodePool,
    );
    row.version = pool.version.clone();
    row.surge = surge(pool);
    row.state = pool_state(pool.status.as_deref());
    let autoscaling = pool.autoscaling.as_ref().filter(|a| a.enabled);
    if autoscaling.is_none() {
        row.nodes = pool
            .initial_node_count
            .map(|n| n as usize * pool.locations.len().max(1));
    }
    let mut message = Vec::new();
    if let Some(config) = &pool.config {
        message.extend(config.machine_type.clone());
        message.extend(config.image_type.clone());
    }
    if let Some(a) = autoscaling {
        message.push(match (a.total_min_node_count, a.total_max_node_count) {
            (Some(min), Some(max)) => format!("autoscaling {min}–{max} nodes"),
            _ => format!(
                "autoscaling {}–{} per zone",
                a.min_node_count.unwrap_or(0),
                a.max_node_count.unwrap_or(0)
            ),
        });
    }
    if pool.management.as_ref().is_some_and(|m| m.auto_upgrade) {
        message.push("auto-upgrade".into());
    }
    if let Some(status) = pool.status_message.as_ref().filter(|s| !s.is_empty()) {
        message.push(status.clone());
    }
    row.message = (!message.is_empty()).then(|| message.join(" · "));
    if let Some(op) = running {
        row.state = PoolState::Updating;
        row.target_version = operation_target(op, master);
    }
    row.updatable = !autopilot
        && pool.status.as_deref() == Some("RUNNING")
        && pool.version.as_deref().is_some_and(|v| newer(master, v));
    row
}

/// The status of a snapshot.
pub fn status(target: &GkeTarget, snap: &Snapshot) -> Status {
    let cluster = &snap.cluster;
    let master = cluster.current_master_version.clone().unwrap_or_default();
    let channel = cluster.channel().map(str::to_string);
    let autopilot = cluster.autopilot();
    let running: Vec<&Operation> = snap
        .operations
        .iter()
        .filter(|o| o.running() && o.is_for(&target.cluster))
        .collect();
    let master_op = running.iter().copied().find(|o| {
        o.node_pool().is_none()
            && o.operation_type
                .as_deref()
                .is_some_and(|t| t.contains("UPGRADE_MASTER"))
    });
    let cluster_status = cluster.status.as_deref().unwrap_or("RUNNING");
    let busy = if let Some(op) = running.first() {
        Some(format!(
            "GKE is running an operation on this cluster ({}); it runs one at a time",
            op.operation_type.as_deref().unwrap_or("operation")
        ))
    } else if cluster_status != "RUNNING" {
        Some(format!(
            "The cluster is {}: GKE updates running clusters",
            cluster_status.to_lowercase()
        ))
    } else {
        None
    };
    let targets = snap
        .server_config
        .as_ref()
        .map(|config| targets(&master, channel.as_deref(), config, busy.as_deref()))
        .unwrap_or_default();

    let mut control_plane = Pool::new(CONTROL_PLANE, "Control plane", PoolKind::ControlPlane);
    control_plane.version = Some(master.clone());
    control_plane.state = pool_state(cluster.status.as_deref());
    control_plane.message = Some(if target.zonal() {
        "zonal: the control plane is unavailable while it updates".into()
    } else {
        "regional: replicas update one at a time".into()
    });
    if let Some(op) = master_op {
        control_plane.state = PoolState::Updating;
        control_plane.target_version = operation_target(op, &master);
    }
    control_plane.updatable = busy.is_none();
    let mut pools = vec![control_plane];
    for pool in &cluster.node_pools {
        let op = running
            .iter()
            .copied()
            .find(|o| o.node_pool() == Some(pool.name.as_str()));
        pools.push(node_pool(pool, &master, autopilot, op));
    }

    let progress = running.first().map(|op| {
        let pool = op.node_pool();
        Progress {
            target: operation_target(op, &master).unwrap_or_default(),
            percent: op.percent(),
            message: match pool {
                Some(pool) => format!("GKE is updating node pool {pool} ({})", op.name),
                None => format!(
                    "GKE is running {} on the control plane ({})",
                    op.operation_type.as_deref().unwrap_or("an operation"),
                    op.name
                ),
            },
            started: op.start_time.as_deref().and_then(|t| t.parse().ok()),
            failing: op.error.as_ref().and_then(|e| e.message.clone()),
        }
    });

    let mut notes = Vec::new();
    if autopilot {
        notes.push(Note {
            warning: false,
            title: "Autopilot".into(),
            text: "GKE updates Autopilot nodes after the control plane; node pools aren't \
                   updated on their own."
                .into(),
            command: None,
            url: None,
        });
    }
    if let Some(message) = cluster.status_message.as_ref().filter(|m| !m.is_empty()) {
        notes.push(Note {
            warning: cluster_status != "RUNNING",
            title: format!("Cluster {}", cluster_status.to_lowercase()),
            text: message.clone(),
            command: None,
            url: None,
        });
    }
    notes.extend(snap.notes.iter().cloned());

    let mut channels: Vec<String> = snap
        .server_config
        .iter()
        .flat_map(|c| &c.channels)
        .map(|c| c.channel.clone())
        .collect();
    if !autopilot && !channels.is_empty() {
        channels.push(NO_CHANNEL.into());
    }

    Status {
        provider: target.label(),
        current: Current {
            version: master,
            kubernetes: None,
            platform: None,
            channel,
            channels,
            cluster_id: Some(target.cluster_path().trim_start_matches("/v1/").to_string()),
            support: None,
            conditions: Vec::new(),
        },
        targets,
        history: Vec::new(),
        progress,
        components: Vec::new(),
        pools,
        addons: Vec::new(),
        notes,
        writes: Writes {
            control_plane: true,
            pools: !autopilot && !cluster.node_pools.is_empty(),
            channel: true,
            addons: false,
            reason: None,
        },
        docs: vec![
            ("GKE release notes".into(), DOCS_RELEASE_NOTES.into()),
            ("GKE release channels".into(), DOCS_CHANNELS.into()),
        ],
    }
}

// ---------------------------------------------------------------------------------------------
// Writes

fn request(
    operation: &str,
    method: &str,
    path: String,
    body: Value,
    permission: &str,
    resource: String,
) -> Value {
    json!({
        "operation": operation,
        "method": method,
        "path": path,
        "body": body,
        "permission": permission,
        "resource": resource,
    })
}

/// The API's channel for one Kubyl offers (`NONE` → `UNSPECIFIED`).
pub fn api_channel(channel: &str) -> Result<String, String> {
    let upper = channel.trim().to_uppercase();
    match upper.as_str() {
        "NONE" | "UNSPECIFIED" | "" => Ok("UNSPECIFIED".into()),
        "RAPID" | "REGULAR" | "STABLE" | "EXTENDED" => Ok(upper),
        _ => Err(format!("GKE has no release channel {channel}.")),
    }
}

/// What a write would do: the control plane or a node pool to `version`, or the channel.
pub fn plan(
    target: &GkeTarget,
    status: &Status,
    scope: &Scope,
    version: &str,
    cluster: &str,
) -> Result<Plan, String> {
    let current = status.current.version.clone();
    let resource = target.resource();
    match scope {
        Scope::ControlPlane => {
            let offered = status
                .target(version)
                .ok_or_else(|| format!("GKE doesn't offer {version} for {cluster}."))?;
            if !offered.startable() {
                return Err(offered.blocked.join("; "));
            }
            let mut notes = vec![format!(
                "Node pools keep {current} until you (or auto-upgrade) update them."
            )];
            if target.zonal() {
                notes.push(
                    "Zonal cluster: the Kubernetes API is unavailable while the control plane \
                     updates."
                        .into(),
                );
            }
            notes.push(
                "This can't be undone: GKE can't downgrade a control plane to an earlier minor."
                    .into(),
            );
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update the control plane of {cluster} to {version}"),
                from: current,
                to: version.to_string(),
                kind_label: if offered.minor {
                    "minor update".into()
                } else {
                    "patch update".into()
                },
                changes: vec![format!(
                    "clusters.update {}: desiredMasterVersion = {version} (like gcloud container \
                     clusters upgrade {} --master --cluster-version {version} {})",
                    target.cluster,
                    target.cluster,
                    target.gcloud_flags()
                )],
                risks: Vec::new(),
                notes,
                irreversible: true,
                request: request(
                    "clusters.update",
                    "PUT",
                    target.cluster_path(),
                    json!({ "update": { "desiredMasterVersion": version } }),
                    "container.clusters.update",
                    resource,
                ),
            })
        }
        Scope::Pool(id) => {
            let name = id
                .strip_prefix("nodepool/")
                .ok_or_else(|| format!("{id} isn't a GKE node pool"))?;
            let pool = status
                .pools
                .iter()
                .find(|p| &p.id == id)
                .ok_or_else(|| format!("{cluster} has no node pool {name}"))?;
            if !status.writes.pools {
                return Err("GKE updates Autopilot nodes itself.".into());
            }
            if let Some(progress) = &status.progress {
                return Err(format!(
                    "{}: GKE runs one operation per cluster at a time.",
                    progress.message
                ));
            }
            if newer(version, &current) {
                return Err(format!(
                    "Update the control plane to {version} first: a node pool can't be newer \
                     than the control plane ({current})."
                ));
            }
            let from = pool.version.clone().unwrap_or_default();
            if newer(&from, version) {
                return Err(format!(
                    "GKE can't downgrade node pool {name} to {version}."
                ));
            }
            if from == version {
                return Err(format!("Node pool {name} already runs {version}."));
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update node pool {name} to {version}"),
                from,
                to: version.to_string(),
                kind_label: "node pool update".into(),
                changes: vec![format!(
                    "nodePools.update {name}: nodeVersion = {version}, image type unchanged (like \
                     gcloud container clusters upgrade {} --node-pool {name} --cluster-version \
                     {version} {})",
                    target.cluster,
                    target.gcloud_flags()
                )],
                risks: Vec::new(),
                notes: vec![
                    format!(
                        "GKE replaces the nodes{}: pods are drained and PodDisruptionBudgets \
                         are respected for up to an hour per node.",
                        pool.surge
                            .as_ref()
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default()
                    ),
                    "This can't be undone once it finished.".into(),
                ],
                irreversible: true,
                request: json!({
                    "operation": "nodePools.update",
                    "method": "PUT",
                    "path": target.node_pool_path(name),
                    "body": { "nodeVersion": version },
                    // `imageType` is required: the pool's current one, read right before.
                    "keepImageType": true,
                    "permission": "container.clusters.update",
                    "resource": format!("node pool {name} of {resource}"),
                }),
            })
        }
        Scope::Channel(channel) => {
            let api = api_channel(channel)?;
            let from = status
                .current
                .channel
                .clone()
                .unwrap_or_else(|| NO_CHANNEL.into());
            if api_channel(&from).ok().as_deref() == Some(api.as_str()) {
                return Err(format!("{cluster} is already on {from}."));
            }
            let shown = if api == "UNSPECIFIED" {
                "None".to_string()
            } else {
                api.to_lowercase()
            };
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Change the release channel of {cluster} to {channel}"),
                from,
                to: channel.clone(),
                kind_label: "channel change".into(),
                changes: vec![format!(
                    "clusters.update {}: desiredReleaseChannel = {api} (like gcloud container \
                     clusters update {} --release-channel {shown} {})",
                    target.cluster,
                    target.cluster,
                    target.gcloud_flags()
                )],
                risks: Vec::new(),
                notes: vec![
                    "The channel decides which versions GKE offers and auto-upgrades the \
                     cluster to; its version doesn't change now."
                        .into(),
                ],
                irreversible: false,
                request: request(
                    "clusters.update",
                    "PUT",
                    target.cluster_path(),
                    json!({ "update": { "desiredReleaseChannel": { "channel": api } } }),
                    "container.clusters.update",
                    resource,
                ),
            })
        }
        Scope::AllPools => Err("On GKE, update node pools one at a time.".into()),
        Scope::AddOn(_) => Err("GKE add-ons follow the control plane.".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// The provider

struct Gke {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: CloudContext,
    http: Result<Http, ProviderError>,
    /// The API endpoint when overridden ([`cloud::GKE_ENDPOINT`], tests), or why the override
    /// is invalid.
    base: Result<Option<Url>, ProviderError>,
    target: Mutex<Option<GkeTarget>>,
    token: Cache<BearerToken>,
    preset: Option<BearerToken>,
}

fn note_for(what: &str, err: &ProviderError) -> Note {
    Note {
        warning: true,
        title: format!("Couldn't read {what}"),
        text: err.to_string(),
        command: None,
        url: None,
    }
}

impl Inner {
    fn new(
        ctx: CloudContext,
        target: Option<GkeTarget>,
        base: Result<Option<Url>, ProviderError>,
        preset: Option<BearerToken>,
    ) -> Self {
        Self {
            ctx,
            http: Http::new(),
            base,
            target: Mutex::new(target),
            token: Cache::default(),
            preset,
        }
    }

    async fn target(&self) -> Result<GkeTarget, ProviderError> {
        if let Some(target) = self.target.lock().unwrap().clone() {
            return Ok(target);
        }
        let exec = cloud::load_exec_info(&self.ctx).await;
        let target = resolve(
            self.ctx.settings.gke.as_ref(),
            exec.as_ref(),
            &self.ctx.context,
            &self.ctx.cluster_entry,
        )?;
        *self.target.lock().unwrap() = Some(target.clone());
        Ok(target)
    }

    async fn api(&self) -> Result<(GkeTarget, Api), ProviderError> {
        let target = self.target().await?;
        let base = match &self.base {
            Ok(Some(base)) => base.clone(),
            Ok(None) => Url::parse(API).expect("a valid URL"),
            Err(err) => return Err(err.clone()),
        };
        let token = match &self.preset {
            Some(token) => token.clone(),
            None => self.token.get(|| fetch_token(&target)).await?,
        };
        let api = Api {
            http: self.http.clone()?,
            base,
            token,
            login: target.login(),
        };
        Ok((target, api))
    }

    async fn forget_rejected<T>(&self, result: &Result<T, ProviderError>) {
        if let Err(ProviderError::Credentials { .. }) = result {
            self.token.clear().await;
        }
    }

    async fn snapshot(&self, target: &GkeTarget, api: &Api) -> Result<Snapshot, ProviderError> {
        let resource = target.resource();
        let cluster = api
            .cluster(
                &target.cluster_path(),
                &Action::new("container.clusters.get", resource.clone()),
            )
            .await?;
        let location = target.location_path();
        let config_action = Action::new(
            "container.clusters.list",
            format!("server config of {}", target.location),
        );
        let operations_action = Action::new(
            "container.operations.list",
            format!("operations in {}", target.location),
        );
        let (config, operations) = futures::join!(
            api.server_config(&location, &config_action),
            api.operations(&location, &operations_action),
        );
        let mut notes = Vec::new();
        let server_config = config
            .inspect_err(|err| notes.push(note_for("the versions GKE offers", err)))
            .ok();
        let operations = operations
            .inspect_err(|err| notes.push(note_for("GKE operations", err)))
            .unwrap_or_default();
        Ok(Snapshot {
            cluster,
            server_config,
            operations,
            notes,
        })
    }

    async fn read(&self) -> Result<Status, ProviderError> {
        let (target, api) = self.api().await?;
        let result = self.snapshot(&target, &api).await;
        self.forget_rejected(&result).await;
        Ok(status(&target, &result?))
    }

    fn plan(
        &self,
        status: &Status,
        scope: &Scope,
        version: &str,
        cluster: &str,
    ) -> Result<Plan, String> {
        let target = self
            .target
            .lock()
            .unwrap()
            .clone()
            .ok_or("Read the cluster first.")?;
        plan(&target, status, scope, version, cluster)
    }

    async fn start(&self, plan: Plan) -> Result<String, ProviderError> {
        let field = |name: &str| {
            plan.request
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ProviderError::Other("This isn't a GKE plan.".into()))
        };
        let (operation, path) = (field("operation")?, field("path")?);
        let method = Method::from_bytes(field("method")?.as_bytes())
            .map_err(|_| ProviderError::Other("This isn't a GKE plan.".into()))?;
        let action = Action::new(field("permission")?, field("resource")?);
        let mut body = plan.request.get("body").cloned().unwrap_or(Value::Null);
        let (_, api) = self.api().await?;
        if plan.request.get("keepImageType") == Some(&Value::Bool(true)) {
            let pool = api
                .node_pool(
                    &path,
                    &Action::new("container.clusters.get", action.resource.clone()),
                )
                .await?;
            let image = pool.config.and_then(|c| c.image_type).ok_or_else(|| {
                ProviderError::Other("GKE didn't say the node pool's image type.".into())
            })?;
            body["imageType"] = Value::String(image);
        }
        let result = api.write(method, &path, &body, &action).await;
        self.forget_rejected(&result).await;
        let op = result?;
        Ok(format!(
            "GKE accepted {operation} ({} → {}): operation {} is {}.",
            plan.from,
            plan.to,
            op.name,
            op.status.as_deref().unwrap_or("pending").to_lowercase()
        ))
    }
}

impl UpdateProvider for Gke {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gke
    }

    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.read().await })
    }

    fn plan(
        &self,
        status: &Status,
        scope: &Scope,
        target: &str,
        cluster: &str,
    ) -> Result<Plan, String> {
        self.inner.plan(status, scope, target, cluster)
    }

    fn start(&self, plan: &Plan) -> ProviderFuture<Result<String, ProviderError>> {
        let inner = self.inner.clone();
        let plan = plan.clone();
        Box::pin(async move { inner.start(plan).await })
    }
}

#[cfg(test)]
mod tests;
