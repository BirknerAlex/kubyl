//! Azure AKS (`updates-aks`).
//!
//! The cluster comes from `updates.clusters.<id>.aks` (subscription, resource group, name), or
//! is found by the API server's FQDN across the subscriptions the Azure CLI's token lists (the
//! default subscription first; then the tenant kubelogin names, if the cluster isn't in the
//! default one). The token comes from `az account get-access-token --resource
//! https://management.azure.com/`. Reads the managed cluster and its upgrade profile; writes
//! `PUT` the fetched cluster with only `kubernetesVersion` changed (control plane only, like
//! `az aks upgrade --control-plane-only`) or an agent pool with its `orchestratorVersion`.

pub mod api;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use url::Url;

use self::api::{API_VERSION, AgentPool, Api, ManagedCluster, UpgradeProfile};
use super::CloudContext;
use super::cloud::{self, AZ, Action, BearerToken, Cache, CliEnv, CliError, ExecInfo, Http};
use crate::model::{
    Current, Note, Plan, Pool, PoolKind, PoolState, Progress, ProviderKind, Risk, Scope, Status,
    Support, Target, TargetKind, Writes,
};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};
use crate::settings::AksSettings;
use crate::version::{self, Version};

/// The id of the control plane pool.
pub const CONTROL_PLANE: &str = "control-plane";
const API: &str = "https://management.azure.com";
/// The token's audience.
const RESOURCE: &str = "https://management.azure.com/";
const DOCS_VERSIONS: &str = "https://learn.microsoft.com/azure/aks/supported-kubernetes-versions";
/// A token without an expiry is used this long.
const TOKEN_FOR: Duration = Duration::from_secs(5 * 60);
/// Subscriptions searched at once for the cluster.
const SEARCH_AT_ONCE: usize = 6;

/// The AKS provider of a cluster. Reads [`cloud::AKS_ENDPOINT`] once.
pub fn provider(ctx: CloudContext) -> Arc<dyn UpdateProvider> {
    Arc::new(build(ctx, cloud::env_lookup, None))
}

/// The provider with the environment read by `lookup` and, in tests, a preset token.
fn build(
    ctx: CloudContext,
    lookup: impl Fn(&str) -> Option<String>,
    preset: Option<AzToken>,
) -> Aks {
    let base = cloud::endpoint_override(cloud::AKS_ENDPOINT, lookup);
    Aks {
        inner: Arc::new(Inner::new(ctx, base, preset)),
    }
}

// ---------------------------------------------------------------------------------------------
// Which cluster

/// A managed cluster's place in Azure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterRef {
    pub subscription: String,
    pub resource_group: String,
    pub name: String,
}

impl ClusterRef {
    /// `/subscriptions/…/resourceGroups/…/providers/Microsoft.ContainerService/managedClusters/…`.
    pub fn path(&self) -> String {
        format!(
            "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.ContainerService/managedClusters/{}",
            cloud::encode(&self.subscription),
            cloud::encode(&self.resource_group),
            cloud::encode(&self.name)
        )
    }

    pub fn agent_pool_path(&self, pool: &str) -> String {
        format!("{}/agentPools/{}", self.path(), cloud::encode(pool))
    }

    fn resource(&self) -> String {
        format!("managed cluster {}", self.name)
    }
}

/// A managed cluster's resource id (`resourceGroups` in any case).
pub fn parse_resource_id(id: &str) -> Option<ClusterRef> {
    let parts: Vec<&str> = id.trim_matches('/').split('/').collect();
    let find = |key: &str| {
        parts
            .iter()
            .position(|p| p.eq_ignore_ascii_case(key))
            .and_then(|i| parts.get(i + 1))
            .map(|v| v.to_string())
    };
    Some(ClusterRef {
        subscription: find("subscriptions")?,
        resource_group: find("resourceGroups")?,
        name: find("managedClusters")?,
    })
}

/// The cluster the settings name, when they name all of it.
pub fn settings_ref(settings: Option<&AksSettings>) -> Option<ClusterRef> {
    let settings = settings?;
    let value = |v: &Option<String>| v.clone().filter(|v| !v.is_empty());
    Some(ClusterRef {
        subscription: value(&settings.subscription)?,
        resource_group: value(&settings.resource_group)?,
        name: value(&settings.name)?,
    })
}

/// What kubelogin's exec plugin says: the tenant, and `AZURE_CONFIG_DIR` for the CLI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    pub tenant: Option<String>,
    pub env: CliEnv,
}

pub fn hints(exec: &ExecInfo) -> Hints {
    let tenant = (exec.program() == "kubelogin")
        .then(|| exec.flag(&["--tenant-id", "-t"]))
        .flatten()
        .or_else(|| exec.env.get("AZURE_TENANT_ID").map(str::to_string));
    Hints {
        tenant,
        env: exec.env.filter(|n| n == "AZURE_CONFIG_DIR"),
    }
}

/// The managed cluster whose FQDN (public, private or portal) is `host`.
pub fn find_by_host<'a>(clusters: &'a [ManagedCluster], host: &str) -> Option<&'a ManagedCluster> {
    let host = host.trim_end_matches('.').to_lowercase();
    clusters.iter().find(|c| {
        let p = &c.properties;
        [&p.fqdn, &p.private_fqdn, &p.azure_portal_fqdn]
            .into_iter()
            .flatten()
            .any(|fqdn| fqdn.trim_end_matches('.').eq_ignore_ascii_case(&host))
    })
}

// ---------------------------------------------------------------------------------------------
// The token

/// An `az account get-access-token` token. `Debug` never shows it.
#[derive(Clone, Debug)]
pub struct AzToken {
    pub token: BearerToken,
    pub expires: Option<Timestamp>,
    /// The CLI's default subscription.
    pub subscription: Option<String>,
    pub tenant: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenOutput {
    access_token: String,
    expires_on: Option<String>,
    /// POSIX seconds (az 2.54+).
    #[serde(rename = "expires_on")]
    expires_on_posix: Option<Value>,
    subscription: Option<String>,
    tenant: Option<String>,
}

/// `az account get-access-token --resource https://management.azure.com/ --output json
/// [--tenant T]`.
pub fn token_args(tenant: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "account",
        "get-access-token",
        "--resource",
        RESOURCE,
        "--output",
        "json",
    ]
    .map(String::from)
    .to_vec();
    if let Some(tenant) = tenant {
        args.extend(["--tenant".to_string(), tenant.to_string()]);
    }
    args
}

/// Parses the CLI's token. Errors say where the JSON broke, never what it held.
pub fn parse_token(stdout: &[u8]) -> Result<AzToken, String> {
    let output: TokenOutput = serde_json::from_slice(stdout).map_err(|err| {
        format!(
            "the output isn't the expected JSON (line {}, column {})",
            err.line(),
            err.column()
        )
    })?;
    if output.access_token.is_empty() {
        return Err("az printed no access token".into());
    }
    let posix = match &output.expires_on_posix {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    };
    // `expiresOn` is local time without a zone (`2026-09-27 13:00:00.000000`).
    let local = || {
        let text = output.expires_on.as_deref()?.replacen(' ', "T", 1);
        let civil: jiff::civil::DateTime = text.parse().ok()?;
        civil
            .to_zoned(jiff::tz::TimeZone::system())
            .ok()
            .map(|z| z.timestamp())
    };
    Ok(AzToken {
        token: BearerToken(output.access_token.into()),
        expires: posix
            .and_then(|s| Timestamp::from_second(s).ok())
            .or_else(local),
        subscription: output.subscription,
        tenant: output.tenant,
    })
}

/// `az login` (for a tenant).
pub fn login(tenant: Option<&str>) -> String {
    match tenant {
        Some(tenant) => format!("az login --tenant {tenant}"),
        None => "az login".into(),
    }
}

/// What a failed `az` run means, and what fixes it.
pub fn classify(error: &CliError, tenant: Option<&str>) -> ProviderError {
    let shown = format!("az {}", token_args(tenant).join(" "));
    let CliError::Failed { stderr, .. } = error else {
        return error.to_provider_error(AZ, &shown, None);
    };
    let lower = stderr.to_lowercase();
    if lower.contains("az login")
        || lower.contains("aadsts")
        || lower.contains("expired")
        || lower.contains("refresh token")
        || lower.contains("interaction_required")
    {
        return ProviderError::Credentials {
            message: format!(
                "Your Azure CLI sign-in has expired or isn't set up: {}",
                cloud::stderr_summary(stderr)
            ),
            command: Some(login(tenant)),
        };
    }
    error.to_provider_error(AZ, &shown, Some(login(tenant)))
}

async fn fetch_token(
    tenant: Option<String>,
    env: CliEnv,
) -> Result<(AzToken, Timestamp), ProviderError> {
    let args = token_args(tenant.as_deref());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout = cloud::run(AZ, &refs, &env)
        .await
        .map_err(|err| classify(&err, tenant.as_deref()))?;
    let token = parse_token(&stdout)
        .map_err(|m| ProviderError::Other(format!("`az account get-access-token`: {m}")))?;
    let until = cloud::fresh_until(token.expires, TOKEN_FOR, Timestamp::now());
    Ok((token, until))
}

// ---------------------------------------------------------------------------------------------
// What AKS reports

/// Everything one read fetched.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cluster: ManagedCluster,
    /// `None`: Kubyl couldn't read it.
    pub profile: Option<UpgradeProfile>,
    /// The subscription's display name, when Kubyl found the cluster by searching.
    pub subscription_name: Option<String>,
    pub notes: Vec<Note>,
}

/// The pool id of an agent pool.
pub fn agent_pool_id(name: &str) -> String {
    format!("agentpool/{name}")
}

fn newer(a: &str, b: &str) -> bool {
    version::compare(a, b) == std::cmp::Ordering::Greater
}

/// Provisioning states that mean an operation runs.
fn in_progress(state: Option<&str>) -> bool {
    !matches!(state, None | Some("Succeeded" | "Failed" | "Canceled" | ""))
}

/// The targets of the upgrade profile newer than `current`: one minor at a time (skips are
/// blocked), previews are conditional (accepting the preview is a step), the newest other one
/// is recommended. `busy` blocks all.
pub fn targets(current: &str, profile: &UpgradeProfile, busy: Option<&str>) -> Vec<Target> {
    let Some(cur) = Version::parse(current) else {
        return Vec::new();
    };
    let mut upgrades: Vec<_> = profile
        .properties
        .control_plane_profile
        .upgrades
        .iter()
        .filter(|u| newer(&u.kubernetes_version, current))
        .collect();
    upgrades.sort_by(|a, b| version::compare(&b.kubernetes_version, &a.kubernetes_version));
    upgrades.dedup_by(|a, b| a.kubernetes_version == b.kubernetes_version);
    let next = format!("{}.{}", cur.major, cur.minor + 1);
    let mut recommended = false;
    upgrades
        .into_iter()
        .filter_map(|upgrade| {
            let v = Version::parse(&upgrade.kubernetes_version)?;
            let steps = cur.minors_to(&v);
            let mut target = Target::new(upgrade.kubernetes_version.clone(), TargetKind::Available);
            target.minor = steps >= 1;
            target.url = Some(DOCS_VERSIONS.into());
            if steps > 1 {
                target.kind = TargetKind::Blocked;
                target.blocked.push(format!(
                    "AKS updates one minor at a time ({} → {next} first)",
                    cur.minor_string()
                ));
            } else if let Some(reason) = busy {
                target.kind = TargetKind::Blocked;
                target.blocked.push(reason.to_string());
            } else if upgrade.is_preview == Some(true) {
                target.kind = TargetKind::Conditional;
                target.risks.push(Risk {
                    name: "Preview version".into(),
                    message: "Preview Kubernetes versions aren't covered by AKS support; don't \
                              run them on production clusters."
                        .into(),
                    url: Some(DOCS_VERSIONS.into()),
                });
            } else if !recommended {
                recommended = true;
                target.kind = TargetKind::Recommended;
            }
            Some(target)
        })
        .collect()
}

fn pool_state(provisioning: Option<&str>, power: Option<&str>) -> PoolState {
    if power == Some("Stopped") {
        return PoolState::Paused;
    }
    match provisioning.unwrap_or_default() {
        "Failed" => PoolState::Degraded,
        "Creating" | "Starting" => PoolState::Queued,
        state if in_progress(Some(state)) => PoolState::Updating,
        _ => PoolState::Idle,
    }
}

/// An agent pool row.
pub fn agent_pool(pool: &AgentPool, control_plane: &str) -> Pool {
    let mut row = Pool::new(
        agent_pool_id(&pool.name),
        pool.name.clone(),
        PoolKind::NodePool,
    );
    let running = pool
        .current_orchestrator_version
        .clone()
        .or(pool.orchestrator_version.clone());
    row.version = running.clone();
    row.target_version = pool
        .orchestrator_version
        .clone()
        .filter(|v| Some(v) != running.as_ref());
    row.nodes = pool.count.map(|n| n as usize);
    row.surge = pool.upgrade_settings.as_ref().and_then(|s| {
        let surge = s.max_surge.as_ref().map(|v| format!("maxSurge {v}"));
        let unavailable = s
            .max_unavailable
            .as_ref()
            .map(|v| format!("maxUnavailable {v}"));
        let parts: Vec<String> = surge.into_iter().chain(unavailable).collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    });
    let power = pool.power_state.as_ref().and_then(|p| p.code.as_deref());
    row.state = pool_state(pool.provisioning_state.as_deref(), power);
    let mut message = Vec::new();
    message.extend(pool.mode.clone());
    message.extend(pool.vm_size.clone());
    if pool.enable_auto_scaling == Some(true) {
        message.push(format!(
            "autoscaling {}–{}",
            pool.min_count.unwrap_or(0),
            pool.max_count.unwrap_or(0)
        ));
    }
    if let Some(image) = &pool.node_image_version {
        message.push(format!("image {image}"));
    }
    row.message = (!message.is_empty()).then(|| message.join(" · "));
    row.updatable = pool.provisioning_state.as_deref() == Some("Succeeded")
        && power != Some("Stopped")
        && running.as_deref().is_some_and(|v| newer(control_plane, v));
    row
}

/// The status of a snapshot.
pub fn status(cref: &ClusterRef, snap: &Snapshot) -> Status {
    let cluster = &snap.cluster;
    let p = &cluster.properties;
    let current = p
        .current_kubernetes_version
        .clone()
        .or(p.kubernetes_version.clone())
        .unwrap_or_default();
    let state = p.provisioning_state.as_deref();
    let power = p.power_state.as_ref().and_then(|s| s.code.as_deref());
    let busy = if power == Some("Stopped") {
        Some("The cluster is stopped: start it to update it".to_string())
    } else if in_progress(state) {
        Some(format!(
            "AKS is running an operation on this cluster ({}); it runs one at a time",
            state.unwrap_or_default().to_lowercase()
        ))
    } else {
        None
    };
    let targets = snap
        .profile
        .as_ref()
        .map(|profile| targets(&current, profile, busy.as_deref()))
        .unwrap_or_default();

    let mut control_plane = Pool::new(CONTROL_PLANE, "Control plane", PoolKind::ControlPlane);
    control_plane.version = Some(current.clone());
    control_plane.target_version = p.kubernetes_version.clone().filter(|v| newer(v, &current));
    control_plane.state = pool_state(state, power);
    let mut message = Vec::new();
    if let Some(tier) = cluster.sku.as_ref().and_then(|s| s.tier.as_deref()) {
        message.push(format!("{tier} tier"));
    }
    let channel = p
        .auto_upgrade_profile
        .as_ref()
        .and_then(|a| a.upgrade_channel.clone())
        .filter(|c| !c.is_empty() && c != "none");
    if let Some(channel) = &channel {
        message.push(format!("auto-upgrade {channel}"));
    }
    control_plane.message = (!message.is_empty()).then(|| message.join(" · "));
    control_plane.updatable = busy.is_none();
    let mut pools = vec![control_plane];
    pools.extend(
        p.agent_pool_profiles
            .iter()
            .map(|pool| agent_pool(pool, &current)),
    );

    let progress = if let Some(target) = pools[0].target_version.clone() {
        Some(Progress {
            message: format!("AKS is updating the control plane to {target}"),
            target,
            percent: None,
            started: None,
            failing: None,
        })
    } else {
        pools[1..]
            .iter()
            .find(|pool| pool.state == PoolState::Updating)
            .map(|pool| Progress {
                target: pool.target_version.clone().unwrap_or_default(),
                percent: None,
                message: format!("AKS is updating node pool {}", pool.name),
                started: None,
                failing: None,
            })
    };

    let mut notes = Vec::new();
    if state == Some("Failed") {
        notes.push(Note {
            warning: true,
            title: "The last operation failed".into(),
            text: "The cluster's provisioning state is Failed; running the update again \
                   often fixes it."
                .into(),
            command: Some(format!(
                "az aks show --resource-group {} --name {} --query provisioningState",
                cref.resource_group, cref.name
            )),
            url: None,
        });
    }
    notes.extend(snap.notes.iter().cloned());

    let subscription = snap
        .subscription_name
        .clone()
        .unwrap_or_else(|| cref.subscription.clone());
    let mut label = vec![
        "Azure API".to_string(),
        format!("subscription {subscription}"),
    ];
    label.extend(cluster.location.clone());
    Status {
        provider: format!("{} ({})", ProviderKind::Aks.label(), label.join(" · ")),
        current: Current {
            version: current,
            kubernetes: None,
            platform: None,
            channel,
            channels: Vec::new(),
            cluster_id: Some(cluster.id.clone()).filter(|id| !id.is_empty()),
            support: (p.support_plan.as_deref() == Some("AKSLongTermSupport")).then(|| Support {
                text: "long-term support (LTS)".into(),
                warning: false,
            }),
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
            pools: !p.agent_pool_profiles.is_empty(),
            channel: false,
            addons: false,
            reason: None,
        },
        docs: vec![("AKS Kubernetes versions".into(), DOCS_VERSIONS.into())],
    }
}

// ---------------------------------------------------------------------------------------------
// Writes

fn remove(map: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        map.remove(*key);
    }
}

/// The managed cluster to `PUT`: the fetched one with `kubernetesVersion` changed and its
/// read-only fields (and `servicePrincipalProfile`, like `az aks upgrade`) left out. Agent
/// pools keep their versions: a control-plane-only update. Answers the body and the eTag to
/// send as `If-Match`.
pub fn control_plane_body(
    fetched: Value,
    version: &str,
) -> Result<(Value, Option<String>), String> {
    let mut body = fetched;
    let root = body
        .as_object_mut()
        .ok_or("AKS answered something that isn't a managed cluster")?;
    let etag = root.get("eTag").and_then(Value::as_str).map(str::to_string);
    remove(root, &["id", "name", "type", "systemData", "eTag"]);
    let properties = root
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .ok_or("the managed cluster has no properties")?;
    properties.insert("kubernetesVersion".into(), Value::String(version.into()));
    remove(
        properties,
        &[
            "provisioningState",
            "powerState",
            "maxAgentPools",
            "currentKubernetesVersion",
            "fqdn",
            "privateFQDN",
            "azurePortalFQDN",
            "resourceUID",
            "servicePrincipalProfile",
        ],
    );
    for pool in properties
        .get_mut("agentPoolProfiles")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(pool) = pool.as_object_mut() {
            remove(
                pool,
                &[
                    "provisioningState",
                    "currentOrchestratorVersion",
                    "nodeImageVersion",
                    "eTag",
                ],
            );
        }
    }
    Ok((body, etag))
}

/// The agent pool to `PUT`: the fetched one with `orchestratorVersion` changed, without its
/// read-only fields and snapshot (`creationData`, like `az aks nodepool upgrade`).
pub fn agent_pool_body(fetched: Value, version: &str) -> Result<(Value, Option<String>), String> {
    let mut body = fetched;
    let root = body
        .as_object_mut()
        .ok_or("AKS answered something that isn't an agent pool")?;
    remove(root, &["id", "name", "type", "systemData"]);
    let properties = root
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .ok_or("the agent pool has no properties")?;
    let etag = properties
        .get("eTag")
        .and_then(Value::as_str)
        .map(str::to_string);
    properties.insert("orchestratorVersion".into(), Value::String(version.into()));
    remove(
        properties,
        &[
            "provisioningState",
            "currentOrchestratorVersion",
            "nodeImageVersion",
            "eTag",
            "creationData",
        ],
    );
    Ok((body, etag))
}

/// What a write would do: the control plane or an agent pool to `version`.
pub fn plan(
    cref: &ClusterRef,
    status: &Status,
    scope: &Scope,
    version: &str,
    cluster: &str,
    snapshot: Option<&Snapshot>,
) -> Result<Plan, String> {
    let current = status.current.version.clone();
    let rg = &cref.resource_group;
    match scope {
        Scope::ControlPlane => {
            let target = status
                .target(version)
                .ok_or_else(|| format!("AKS doesn't offer {version} for {cluster}."))?;
            if !target.startable() {
                return Err(target.blocked.join("; "));
            }
            let availability_sets = snapshot.is_some_and(|s| {
                s.cluster
                    .properties
                    .agent_pool_profiles
                    .iter()
                    .any(|p| p.kind.as_deref() == Some("AvailabilitySet"))
            });
            if availability_sets {
                return Err(
                    "This cluster runs on availability sets: AKS updates its node pools with \
                     the control plane. Use az aks upgrade."
                        .into(),
                );
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update the control plane of {cluster} to {version}"),
                from: current.clone(),
                to: version.to_string(),
                kind_label: if target.minor {
                    "minor update".into()
                } else {
                    "patch update".into()
                },
                changes: vec![format!(
                    "PUT managedClusters/{}: properties.kubernetesVersion = {version}, node pools \
                     unchanged (like az aks upgrade --resource-group {rg} --name {} \
                     --kubernetes-version {version} --control-plane-only)",
                    cref.name, cref.name
                )],
                risks: target.risks.clone(),
                notes: vec![
                    format!("Node pools keep {current} until you update them."),
                    "Kubyl reads the cluster right before and sends it back with only the \
                     version changed."
                        .into(),
                    "This can't be undone: AKS can't downgrade a cluster.".into(),
                ],
                irreversible: true,
                request: json!({
                    "kind": "control-plane",
                    "path": cref.path(),
                    "version": version,
                    "permission": "Microsoft.ContainerService/managedClusters/write",
                    "resource": cref.resource(),
                }),
            })
        }
        Scope::Pool(id) => {
            let name = id
                .strip_prefix("agentpool/")
                .ok_or_else(|| format!("{id} isn't an AKS node pool"))?;
            let pool = status
                .pools
                .iter()
                .find(|p| &p.id == id)
                .ok_or_else(|| format!("{cluster} has no node pool {name}"))?;
            if newer(version, &current) {
                return Err(format!(
                    "Update the control plane to {version} first: a node pool can't be newer \
                     than the control plane ({current})."
                ));
            }
            let from = pool.version.clone().unwrap_or_default();
            if !newer(version, &from) {
                return Err(format!("Node pool {name} already runs {from}."));
            }
            if let Some(progress) = &status.progress {
                return Err(format!(
                    "{}: AKS runs one operation per cluster at a time.",
                    progress.message
                ));
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update node pool {name} to {version}"),
                from,
                to: version.to_string(),
                kind_label: "node pool update".into(),
                changes: vec![format!(
                    "PUT agentPools/{name}: properties.orchestratorVersion = {version} (like az \
                     aks nodepool upgrade --resource-group {rg} --cluster-name {} --name {name} \
                     --kubernetes-version {version})",
                    cref.name
                )],
                risks: Vec::new(),
                notes: vec![
                    format!(
                        "AKS adds surge nodes{}, then drains and replaces the old ones; \
                         PodDisruptionBudgets are respected.",
                        pool.surge
                            .as_ref()
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default()
                    ),
                    "This can't be undone: AKS can't downgrade a node pool.".into(),
                ],
                irreversible: true,
                request: json!({
                    "kind": "agent-pool",
                    "path": cref.agent_pool_path(name),
                    "version": version,
                    "permission": "Microsoft.ContainerService/managedClusters/agentPools/write",
                    "resource": format!("node pool {name} of {}", cref.resource()),
                }),
            })
        }
        Scope::AllPools => Err("On AKS, update node pools one at a time.".into()),
        Scope::Channel(_) => {
            Err("Kubyl doesn't change AKS auto-upgrade channels; use az aks update.".into())
        }
        Scope::AddOn(_) => Err("AKS add-ons follow the cluster's version.".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// The provider

struct Aks {
    inner: Arc<Inner>,
}

/// Where the provider found the cluster.
#[derive(Clone)]
struct Found {
    cref: ClusterRef,
    subscription_name: Option<String>,
    /// The tenant whose token reaches it (`None`: the CLI's default).
    tenant: Option<String>,
}

struct Inner {
    ctx: CloudContext,
    http: Result<Http, ProviderError>,
    base: Result<Option<Url>, ProviderError>,
    hints: Mutex<Option<Hints>>,
    found: Mutex<Option<Found>>,
    /// The token of the tenant in use.
    token: Cache<AzToken>,
    token_tenant: Mutex<Option<String>>,
    preset: Option<AzToken>,
    last: Mutex<Option<Arc<Snapshot>>>,
}

impl Inner {
    fn new(
        ctx: CloudContext,
        base: Result<Option<Url>, ProviderError>,
        preset: Option<AzToken>,
    ) -> Self {
        Self {
            ctx,
            http: Http::new(),
            base,
            hints: Mutex::default(),
            found: Mutex::default(),
            token: Cache::default(),
            token_tenant: Mutex::default(),
            preset,
            last: Mutex::default(),
        }
    }

    async fn hints(&self) -> Hints {
        if let Some(hints) = self.hints.lock().unwrap().clone() {
            return hints;
        }
        let hints = cloud::load_exec_info(&self.ctx)
            .await
            .as_ref()
            .map(hints)
            .unwrap_or_default();
        *self.hints.lock().unwrap() = Some(hints.clone());
        hints
    }

    /// An API client with a token for `tenant` (`None`: the CLI's default).
    async fn api(&self, tenant: Option<String>) -> Result<(Api, AzToken), ProviderError> {
        let base = match &self.base {
            Ok(Some(base)) => base.clone(),
            Ok(None) => Url::parse(API).expect("a valid URL"),
            Err(err) => return Err(err.clone()),
        };
        let token = match &self.preset {
            Some(token) => token.clone(),
            None => {
                let switched = {
                    let mut current = self.token_tenant.lock().unwrap();
                    let switched = *current != tenant;
                    *current = tenant.clone();
                    switched
                };
                if switched {
                    self.token.clear().await;
                }
                let env = self.hints().await.env;
                self.token.get(|| fetch_token(tenant.clone(), env)).await?
            }
        };
        let api = Api {
            http: self.http.clone()?,
            base,
            token: token.token.clone(),
            login: login(tenant.as_deref()),
        };
        Ok((api, token))
    }

    /// The cluster: from settings, remembered, or searched by the API server's host.
    async fn find(&self) -> Result<Found, ProviderError> {
        if let Some(found) = self.found.lock().unwrap().clone() {
            return Ok(found);
        }
        let found = match settings_ref(self.ctx.settings.aks.as_ref()) {
            Some(cref) => Found {
                cref,
                subscription_name: None,
                tenant: None,
            },
            None => self.search().await?,
        };
        *self.found.lock().unwrap() = Some(found.clone());
        Ok(found)
    }

    async fn search(&self) -> Result<Found, ProviderError> {
        let host = cloud::server_host(&self.ctx.server)
            .ok_or_else(|| ProviderError::Other(format!("{} isn't a URL", self.ctx.server)))?;
        let hint = self.hints().await.tenant;
        let (api, token) = self.api(None).await?;
        let (found, searched) = search_with(&api, &token, &host).await?;
        if let Some((cref, name)) = found {
            return Ok(Found {
                cref,
                subscription_name: name,
                tenant: None,
            });
        }
        let mut searched_total = searched;
        if let Some(tenant) = hint.filter(|t| token.tenant.as_ref() != Some(t)) {
            let (api, token) = self.api(Some(tenant.clone())).await?;
            let (found, searched) = search_with(&api, &token, &host).await?;
            searched_total += searched;
            if let Some((cref, name)) = found {
                return Ok(Found {
                    cref,
                    subscription_name: name,
                    tenant: Some(tenant),
                });
            }
        }
        Err(ProviderError::NotFound(format!(
            "No AKS cluster in the {searched_total} Azure subscriptions you can read has the API \
             server {host}. Set \"updates\": {{\"clusters\": {{\"{}\": {{\"aks\": \
             {{\"subscription\": …, \"resource_group\": …, \"name\": …}}}}}}}} in settings.json.",
            self.ctx.context
        )))
    }

    async fn snapshot(&self, api: &Api, found: &Found) -> Result<Snapshot, ProviderError> {
        let path = found.cref.path();
        let resource = found.cref.resource();
        let cluster_action = Action::new(
            "Microsoft.ContainerService/managedClusters/read",
            resource.clone(),
        );
        let profile_action = Action::new(
            "Microsoft.ContainerService/managedClusters/upgradeProfiles/read",
            resource,
        );
        let profile_path = format!("{path}/upgradeProfiles/default");
        let (cluster, profile) = futures::join!(
            api.get::<ManagedCluster>(&path, API_VERSION, &cluster_action, "the AKS cluster"),
            api.get::<UpgradeProfile>(
                &profile_path,
                API_VERSION,
                &profile_action,
                "the AKS upgrade profile"
            ),
        );
        let cluster = cluster?;
        let mut notes = Vec::new();
        let profile = profile
            .inspect_err(|err| {
                notes.push(Note {
                    warning: true,
                    title: "Couldn't read the versions AKS offers".into(),
                    text: err.to_string(),
                    command: None,
                    url: None,
                })
            })
            .ok();
        Ok(Snapshot {
            cluster,
            profile,
            subscription_name: found.subscription_name.clone(),
            notes,
        })
    }

    async fn read(&self) -> Result<Status, ProviderError> {
        let found = self.find().await?;
        let (api, _) = self.api(found.tenant.clone()).await?;
        let result = self.snapshot(&api, &found).await;
        if let Err(err) = &result {
            match err {
                ProviderError::Credentials { .. } => self.token.clear().await,
                // Moved or deleted: search again next time.
                ProviderError::NotFound(_) => *self.found.lock().unwrap() = None,
                _ => {}
            }
        }
        let snapshot = result?;
        let status = status(&found.cref, &snapshot);
        *self.last.lock().unwrap() = Some(Arc::new(snapshot));
        Ok(status)
    }

    fn plan(
        &self,
        status: &Status,
        scope: &Scope,
        version: &str,
        cluster: &str,
    ) -> Result<Plan, String> {
        let found = self
            .found
            .lock()
            .unwrap()
            .clone()
            .ok_or("Read the cluster first.")?;
        let last = self.last.lock().unwrap().clone();
        plan(
            &found.cref,
            status,
            scope,
            version,
            cluster,
            last.as_deref(),
        )
    }

    async fn start(&self, plan: Plan) -> Result<String, ProviderError> {
        let field = |name: &str| {
            plan.request
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ProviderError::Other("This isn't an AKS plan.".into()))
        };
        let (kind, path, version) = (field("kind")?, field("path")?, field("version")?);
        let action = Action::new(field("permission")?, field("resource")?);
        let found = self.find().await?;
        let (api, _) = self.api(found.tenant.clone()).await?;
        let read = Action::new(
            action.permission.replace("/write", "/read"),
            action.resource.clone(),
        );
        let fetched: Value = api
            .get(&path, API_VERSION, &read, "the AKS resource")
            .await?;
        let (body, etag) = match kind.as_str() {
            "control-plane" => control_plane_body(fetched, &version),
            "agent-pool" => agent_pool_body(fetched, &version),
            _ => return Err(ProviderError::Other("This isn't an AKS plan.".into())),
        }
        .map_err(ProviderError::Other)?;
        let result = api.put(&path, &body, etag.as_deref(), &action).await;
        if let Err(ProviderError::Credentials { .. }) = &result {
            self.token.clear().await;
        }
        let answer = result?;
        let state = answer["properties"]["provisioningState"]
            .as_str()
            .unwrap_or("accepted");
        Ok(format!(
            "AKS accepted the update of {} to {version} (provisioning state {state}).",
            action.resource
        ))
    }
}

/// Searches the token's subscriptions (its default one first) for the cluster with `host`.
/// Answers it (with the subscription's name) and how many subscriptions were searched.
async fn search_with(
    api: &Api,
    token: &AzToken,
    host: &str,
) -> Result<(Option<(ClusterRef, Option<String>)>, usize), ProviderError> {
    let mut subscriptions: Vec<_> = api
        .subscriptions()
        .await?
        .into_iter()
        .filter(|s| s.state.as_deref().is_none_or(|state| state == "Enabled"))
        .collect();
    subscriptions.sort_by_key(|s| Some(&s.subscription_id) != token.subscription.as_ref());
    let count = subscriptions.len();
    let found = futures::stream::iter(subscriptions)
        .map(|subscription| async move {
            // A subscription Kubyl may not read clusters in is skipped.
            let clusters = api.clusters_in(&subscription.subscription_id).await.ok()?;
            let cluster = find_by_host(&clusters, host)?;
            Some((parse_resource_id(&cluster.id)?, subscription.display_name))
        })
        .buffer_unordered(SEARCH_AT_ONCE)
        .filter_map(|found| async move { found })
        .boxed()
        .next()
        .await;
    Ok((found, count))
}

impl UpdateProvider for Aks {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Aks
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
