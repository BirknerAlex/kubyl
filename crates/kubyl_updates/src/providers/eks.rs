//! Amazon EKS (`updates-eks`).
//!
//! The cluster, region and profile come from the kubeconfig's exec plugin (`aws eks get-token
//! --cluster-name X --region R --profile P`, `aws-iam-authenticator token -i X`), its env
//! (`AWS_PROFILE`, `AWS_REGION`), the cluster ARN in the context names or the API server's
//! host, overridden by `updates.clusters.<id>.eks`. Credentials come from `aws configure
//! export-credentials` (or `aws sts assume-role` for `--role-arn`) and requests are signed with
//! SigV4 ([`sigv4`]). EKS updates the control plane one minor at a time; node groups and
//! add-ons follow on their own.

pub mod api;
pub mod credentials;
pub mod sigv4;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::join_all;
use jiff::Timestamp;
use serde_json::{Value, json};
use url::Url;

use self::api::{Addon, AddonVersion, Api, Cluster, ClusterVersion, Insight, Nodegroup, Update};
use self::credentials::AwsCredentials;
use super::CloudContext;
use super::cloud::{self, Action, Cache, CliEnv, ExecInfo, Http, encode};
use crate::check::{Check, CheckStatus, Detail, Fix};
use crate::model::{
    AddOn, Current, HistoryEntry, Note, Plan, Pool, PoolKind, PoolState, Progress, ProviderKind,
    Risk, Scope, Status, Support, Target, TargetKind, Writes,
};
use crate::provider::{ProviderError, ProviderFuture, UpdateProvider};
use crate::settings::EksSettings;
use crate::version::{self, Version};

/// The id of the control plane pool.
pub const CONTROL_PLANE: &str = "control-plane";
/// EKS's Kubernetes versions and their support.
const DOCS_VERSIONS: &str =
    "https://docs.aws.amazon.com/eks/latest/userguide/kubernetes-versions.html";
const DOCS_INSIGHTS: &str =
    "https://docs.aws.amazon.com/eks/latest/userguide/cluster-insights.html";
/// Add-on versions change rarely: read them again after this long.
const ADDON_VERSIONS_FOR: Duration = Duration::from_secs(60 * 60);
/// Updates Kubyl describes per read at most (finished ones are remembered).
const DESCRIBE_UPDATES: usize = 30;
/// Support ending sooner than this is a warning.
const SUPPORT_WARNING_DAYS: i64 = 90;

/// The EKS provider of a cluster. Reads [`cloud::EKS_ENDPOINT`] once.
pub fn provider(ctx: CloudContext) -> Arc<dyn UpdateProvider> {
    Arc::new(build(ctx, cloud::env_lookup, None))
}

/// The provider with the environment read by `lookup` and, in tests, preset credentials.
fn build(
    ctx: CloudContext,
    lookup: impl Fn(&str) -> Option<String>,
    preset: Option<AwsCredentials>,
) -> Eks {
    let base = cloud::endpoint_override(cloud::EKS_ENDPOINT, lookup);
    Eks {
        inner: Arc::new(Inner::new(ctx, None, base, preset)),
    }
}

// ---------------------------------------------------------------------------------------------
// Which cluster

/// What the kubeconfig's exec plugin says about the cluster.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hints {
    pub cluster: Option<String>,
    /// `aws eks get-token --cluster-id` (EKS local clusters on Outposts: an id).
    pub cluster_id: Option<String>,
    pub region: Option<String>,
    pub profile: Option<String>,
    pub role_arn: Option<String>,
    /// The plugin's `AWS_*` variables (`AWS_PROFILE`, `AWS_CONFIG_FILE`…), for the CLI.
    pub env: CliEnv,
}

/// Reads `aws eks get-token` and `aws-iam-authenticator token` arguments and env.
pub fn hints(exec: &ExecInfo) -> Hints {
    let mut hints = Hints::default();
    match exec.program().as_str() {
        "aws" => {
            hints.cluster = exec.flag(&["--cluster-name"]);
            hints.cluster_id = exec.flag(&["--cluster-id"]);
            hints.region = exec.flag(&["--region"]);
            hints.profile = exec.flag(&["--profile"]);
            hints.role_arn = exec.flag(&["--role-arn"]);
        }
        "aws-iam-authenticator" => {
            hints.cluster = exec.flag(&["-i", "--cluster-id"]);
            hints.role_arn = exec.flag(&["-r", "--role"]);
            hints.region = exec.flag(&["--region"]);
        }
        _ => {}
    }
    let env = |name: &str| {
        exec.env
            .get(name)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    hints.profile = hints.profile.or_else(|| env("AWS_PROFILE"));
    hints.region = hints
        .region
        .or_else(|| env("AWS_REGION"))
        .or_else(|| env("AWS_DEFAULT_REGION"));
    hints.env = exec.env.filter(|name| name.starts_with("AWS_"));
    hints
}

/// An EKS cluster ARN (`arn:aws:eks:eu-west-1:111122223333:cluster/prod`): partition, region,
/// name.
pub fn parse_arn(text: &str) -> Option<(String, String, String)> {
    let mut parts = text.splitn(6, ':');
    if parts.next()? != "arn" {
        return None;
    }
    let partition = parts.next()?;
    if parts.next()? != "eks" {
        return None;
    }
    let region = parts.next()?;
    let _account = parts.next()?;
    let name = parts.next()?.strip_prefix("cluster/")?;
    (!region.is_empty() && !name.is_empty())
        .then(|| (partition.to_string(), region.to_string(), name.to_string()))
}

/// The region and API domain of an EKS API server
/// (`https://<id>.<suffix>.<region>.eks.amazonaws.com`).
pub fn server_region(server: &str) -> Option<(String, String)> {
    let host = cloud::server_host(server)?;
    let (rest, domain) = if let Some(rest) = host.strip_suffix(".eks.amazonaws.com") {
        (rest, "amazonaws.com")
    } else {
        (
            host.strip_suffix(".eks.amazonaws.com.cn")?,
            "amazonaws.com.cn",
        )
    };
    let region = rest.rsplit('.').next().filter(|r| r.contains('-'))?;
    Some((region.to_string(), domain.to_string()))
}

/// The cluster Kubyl talks to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EksTarget {
    pub cluster: String,
    pub region: String,
    pub profile: Option<String>,
    pub role_arn: Option<String>,
    /// `amazonaws.com` (`amazonaws.com.cn` in China).
    pub domain: String,
    /// The exec plugin's `AWS_*` variables.
    pub env: CliEnv,
}

impl EksTarget {
    /// `https://eks.eu-west-1.amazonaws.com`.
    pub fn endpoint(&self) -> String {
        format!("https://eks.{}.{}", self.region, self.domain)
    }

    /// `Amazon EKS (AWS API · profile prod · eu-west-1)`.
    pub fn label(&self) -> String {
        let mut parts = vec!["AWS API".to_string()];
        if let Some(profile) = &self.profile {
            parts.push(format!("profile {profile}"));
        }
        if let Some(role) = &self.role_arn {
            parts.push(format!("role {}", role.rsplit('/').next().unwrap_or(role)));
        }
        parts.push(self.region.clone());
        format!("{} ({})", ProviderKind::Eks.label(), parts.join(" · "))
    }

    /// ` --region eu-west-1 --profile prod`, for commands to copy.
    pub fn cli_suffix(&self) -> String {
        let mut suffix = format!(" --region {}", self.region);
        if let Some(profile) = &self.profile {
            suffix.push_str(&format!(" --profile {profile}"));
        }
        suffix
    }

    /// The command that renews expired credentials.
    pub fn login(&self) -> Option<String> {
        self.profile
            .as_deref()
            .map(|p| credentials::sso_login(Some(p)))
    }
}

/// Picks cluster, region and profile: settings first, then the exec plugin, the cluster ARN in
/// the context or cluster entry name, and the API server's host.
pub fn resolve(
    settings: Option<&EksSettings>,
    hints: &Hints,
    context: &str,
    cluster_entry: &str,
    server: &str,
) -> Result<EksTarget, ProviderError> {
    let arn = [cluster_entry, context].into_iter().find_map(parse_arn);
    let from_server = server_region(server);
    let setting = |f: fn(&EksSettings) -> &Option<String>| {
        settings
            .and_then(|s| f(s).clone())
            .filter(|v| !v.is_empty())
    };
    let cluster = setting(|s| &s.cluster)
        .or_else(|| hints.cluster.clone())
        .or_else(|| arn.as_ref().map(|(_, _, name)| name.clone()))
        .or_else(|| hints.cluster_id.clone());
    let region = setting(|s| &s.region)
        .or_else(|| hints.region.clone())
        .or_else(|| arn.as_ref().map(|(_, region, _)| region.clone()))
        .or_else(|| from_server.as_ref().map(|(region, _)| region.clone()));
    let missing = |what: &str| {
        ProviderError::Other(format!(
            "Kubyl can't tell the EKS {what} of this cluster from its kubeconfig. Set \
             \"updates\": {{\"clusters\": {{\"{context}\": {{\"eks\": {{\"cluster\": …, \
             \"region\": …, \"profile\": …}}}}}}}} in settings.json."
        ))
    };
    let cluster = cluster.ok_or_else(|| missing("cluster name"))?;
    let region = region.ok_or_else(|| missing("region"))?;
    let china = from_server
        .as_ref()
        .is_some_and(|(_, d)| d == "amazonaws.com.cn")
        || arn.as_ref().is_some_and(|(p, _, _)| p == "aws-cn")
        || region.starts_with("cn-");
    Ok(EksTarget {
        cluster,
        region,
        profile: setting(|s| &s.profile).or_else(|| hints.profile.clone()),
        role_arn: hints.role_arn.clone(),
        domain: if china {
            "amazonaws.com.cn"
        } else {
            "amazonaws.com"
        }
        .into(),
        env: hints.env.clone(),
    })
}

// ---------------------------------------------------------------------------------------------
// What EKS reports

/// Everything one read fetched.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cluster: Cluster,
    pub nodegroups: Vec<Nodegroup>,
    pub addons: Vec<Addon>,
    /// Every version of each installed add-on, by add-on name.
    pub addon_versions: HashMap<String, Vec<AddonVersion>>,
    /// `None`: Kubyl couldn't list EKS versions.
    pub versions: Option<Vec<ClusterVersion>>,
    /// The cluster's updates, newest first.
    pub updates: Vec<Update>,
    /// Running updates of node groups, by node group name.
    pub nodegroup_updates: HashMap<String, Update>,
    /// What Kubyl couldn't read.
    pub notes: Vec<Note>,
}

/// The pool id of a node group.
pub fn nodegroup_pool_id(name: &str) -> String {
    format!("nodegroup/{name}")
}

fn minor(text: &str) -> Option<String> {
    Version::parse(text).map(|v| v.minor_string())
}

fn same_minor(a: &str, b: &str) -> bool {
    minor(a).is_some() && minor(a) == minor(b)
}

/// The targets EKS offers from `current`: the next minor (recommended), newer ones blocked
/// (one minor at a time). Without EKS's version list, the next minor is only "available".
/// `busy` blocks everything (an update runs).
pub fn targets(
    current: &str,
    versions: Option<&[ClusterVersion]>,
    busy: Option<&str>,
) -> Vec<Target> {
    let Some(cur) = Version::parse(current) else {
        return Vec::new();
    };
    let mut newer: Vec<Version> = match versions {
        Some(versions) => versions
            .iter()
            .filter(|v| v.support_status() != Some("UNSUPPORTED"))
            .filter_map(|v| Version::parse(&v.cluster_version))
            .filter(|v| v.minor_key() > cur.minor_key())
            .collect(),
        None => version::next_minor(current)
            .and_then(|v| Version::parse(&v))
            .into_iter()
            .collect(),
    };
    newer.sort_by_key(|v| std::cmp::Reverse(v.minor_key()));
    newer.dedup_by_key(|v| v.minor_key());
    let next = format!("{}.{}", cur.major, cur.minor + 1);
    newer
        .iter()
        .map(|v| {
            let one_step = cur.minors_to(v) == 1;
            let kind = match (one_step, versions.is_some()) {
                (true, true) => TargetKind::Recommended,
                (true, false) => TargetKind::Available,
                (false, _) => TargetKind::Blocked,
            };
            let mut target = Target::new(v.minor_string(), kind);
            target.minor = true;
            target.url = Some(DOCS_VERSIONS.into());
            if !one_step {
                target.blocked.push(format!(
                    "EKS updates one minor at a time ({} → {next} first)",
                    cur.minor_string()
                ));
            } else if let Some(reason) = busy {
                target.kind = TargetKind::Blocked;
                target.blocked.push(reason.to_string());
            }
            target
        })
        .collect()
}

/// The support window of the cluster's version.
pub fn support(
    info: Option<&ClusterVersion>,
    support_type: Option<&str>,
    now: Timestamp,
) -> Option<Support> {
    let info = info?;
    let standard = cloud::timestamp(&info.end_of_standard_support_date);
    let extended = cloud::timestamp(&info.end_of_extended_support_date);
    let days_left = |at: Timestamp| at.duration_since(now).as_hours() / 24;
    match info.support_status() {
        Some("EXTENDED_SUPPORT") => Some(Support {
            text: match extended {
                Some(at) => format!("extended support until {}", cloud::date(at)),
                None => "extended support".into(),
            },
            warning: true,
        }),
        Some("UNSUPPORTED") => Some(Support {
            text: "no longer supported by EKS".into(),
            warning: true,
        }),
        _ => {
            let at = standard?;
            let mut text = format!("standard support until {}", cloud::date(at));
            match (support_type, extended) {
                (Some("EXTENDED"), Some(ext)) => {
                    text.push_str(&format!(", extended until {}", cloud::date(ext)))
                }
                (Some("STANDARD"), _) => text.push_str(", then EKS updates it to the next version"),
                _ => {}
            }
            Some(Support {
                text,
                warning: days_left(at) < SUPPORT_WARNING_DAYS,
            })
        }
    }
}

/// Whether `addon_version` lists `cluster_version` as compatible.
pub fn supports(versions: &[AddonVersion], addon_version: &str, cluster_version: &str) -> bool {
    versions
        .iter()
        .filter(|v| v.addon_version == addon_version)
        .flat_map(|v| &v.compatibilities)
        .any(|c| same_minor(&c.cluster_version, cluster_version))
}

/// The add-on version for a cluster version: EKS's default, else the newest compatible one.
pub fn recommended_version(versions: &[AddonVersion], cluster_version: &str) -> Option<String> {
    let compatible = || {
        versions.iter().filter(|v| {
            v.compatibilities
                .iter()
                .any(|c| same_minor(&c.cluster_version, cluster_version))
        })
    };
    compatible()
        .find(|v| {
            v.compatibilities
                .iter()
                .any(|c| same_minor(&c.cluster_version, cluster_version) && c.default_version)
        })
        .or_else(|| {
            compatible().max_by(|a, b| version::compare(&a.addon_version, &b.addon_version))
        })
        .map(|v| v.addon_version.clone())
}

/// An add-on row: compatibility with and the recommended version for `cluster_version`.
pub fn addon_row(addon: &Addon, versions: Option<&[AddonVersion]>, cluster_version: &str) -> AddOn {
    let installed = addon.addon_version.clone().unwrap_or_default();
    let recommended = versions.and_then(|vs| recommended_version(vs, cluster_version));
    AddOn {
        name: addon.addon_name.clone(),
        compatible: versions.map(|vs| supports(vs, &installed, cluster_version)),
        updatable: addon.status.as_deref() == Some("ACTIVE")
            && recommended.as_ref().is_some_and(|r| *r != installed),
        recommended,
        status: addon.status.clone(),
        version: installed,
    }
}

fn pool_state(status: Option<&str>) -> PoolState {
    match status.unwrap_or_default() {
        "UPDATING" => PoolState::Updating,
        "CREATING" | "PENDING" => PoolState::Queued,
        "FAILED" | "DEGRADED" | "CREATE_FAILED" | "DELETE_FAILED" => PoolState::Degraded,
        _ => PoolState::Idle,
    }
}

/// `maxUnavailable 1`, `maxUnavailable 33%`.
fn surge(ng: &Nodegroup) -> Option<String> {
    let config = ng.update_config.as_ref()?;
    let mut text = match (config.max_unavailable, config.max_unavailable_percentage) {
        (Some(n), _) => format!("maxUnavailable {n}"),
        (None, Some(p)) => format!("maxUnavailable {p}%"),
        (None, None) => return None,
    };
    if config.update_strategy.as_deref() == Some("MINIMAL") {
        text.push_str(" · minimal");
    }
    Some(text)
}

fn custom_ami(ng: &Nodegroup) -> bool {
    ng.ami_type.as_deref() == Some("CUSTOM")
}

/// A node group row.
pub fn nodegroup_pool(ng: &Nodegroup, control_plane: &str, running: Option<&Update>) -> Pool {
    let mut pool = Pool::new(
        nodegroup_pool_id(&ng.nodegroup_name),
        ng.nodegroup_name.clone(),
        PoolKind::NodePool,
    );
    pool.version = ng.version.clone();
    pool.nodes = ng
        .scaling_config
        .as_ref()
        .and_then(|s| s.desired_size)
        .map(|n| n as usize);
    pool.surge = surge(ng);
    pool.state = pool_state(ng.status.as_deref());
    let mut message = Vec::new();
    if let Some(release) = &ng.release_version {
        message.push(format!("AMI {release}"));
    }
    if !ng.instance_types.is_empty() {
        message.push(ng.instance_types.join(", "));
    }
    if let Some(capacity) = &ng.capacity_type {
        message.push(capacity.to_lowercase().replace('_', "-"));
    }
    if custom_ami(ng) {
        message.push("custom AMI: update its launch template".into());
    }
    if let Some(issue) = ng.health.iter().flat_map(|h| &h.issues).next() {
        message.push(
            issue
                .message
                .clone()
                .or(issue.code.clone())
                .unwrap_or_default(),
        );
    }
    pool.message = (!message.is_empty()).then(|| message.join(" · "));
    if let Some(update) = running {
        pool.state = PoolState::Updating;
        pool.target_version = update
            .param("Version")
            .or(update.param("ReleaseVersion"))
            .map(str::to_string);
    }
    let behind = ng
        .version
        .as_deref()
        .zip(Version::parse(control_plane))
        .and_then(|(v, cp)| Some(Version::parse(v)?.minor_key() < cp.minor_key()))
        .unwrap_or(false);
    pool.updatable = behind && !custom_ami(ng) && ng.status.as_deref() == Some("ACTIVE");
    pool
}

fn history(updates: &[Update]) -> Vec<HistoryEntry> {
    updates
        .iter()
        .filter(|u| u.kind == "VersionUpdate")
        .filter_map(|u| {
            Some(HistoryEntry {
                version: u.param("Version")?.to_string(),
                state: match u.status.as_str() {
                    "Successful" => "Completed".into(),
                    "InProgress" => "Partial".into(),
                    other => other.to_string(),
                },
                started: u.created(),
                completed: None,
                verified: false,
                accepted_risks: None,
            })
        })
        .collect()
}

/// The status of a snapshot.
pub fn status(target: &EksTarget, snap: &Snapshot, now: Timestamp) -> Status {
    let cluster = &snap.cluster;
    let version = cluster.version.clone().unwrap_or_default();
    let cluster_status = cluster.status.as_deref().unwrap_or("ACTIVE");
    let running = snap.updates.iter().find(|u| u.running());
    let busy = match running {
        Some(update) => Some(format!(
            "EKS is running an update of this cluster ({}); it runs one at a time",
            update.kind
        )),
        None if cluster_status != "ACTIVE" => Some(format!(
            "The cluster is {}: EKS updates ACTIVE clusters",
            cluster_status.to_lowercase()
        )),
        None => None,
    };
    let targets = targets(&version, snap.versions.as_deref(), busy.as_deref());
    let version_info = snap
        .versions
        .iter()
        .flatten()
        .find(|v| same_minor(&v.cluster_version, &version));
    let support_type = cluster
        .upgrade_policy
        .as_ref()
        .and_then(|p| p.support_type.as_deref());

    // Add-ons are checked against the next minor (the only one EKS starts), else the current.
    let next = version::next_minor(&version).unwrap_or_default();
    let addon_target = if targets.iter().any(|t| t.version == next) {
        next.clone()
    } else {
        minor(&version).unwrap_or_default()
    };

    let mut pools = Vec::new();
    let mut control_plane = Pool::new(CONTROL_PLANE, "Control plane", PoolKind::ControlPlane);
    control_plane.version = cluster.version.clone();
    control_plane.message = cluster
        .platform_version
        .as_ref()
        .map(|p| format!("platform {p}"));
    control_plane.state = pool_state(cluster.status.as_deref());
    if let Some(update) = running.filter(|u| u.kind == "VersionUpdate") {
        control_plane.state = PoolState::Updating;
        control_plane.target_version = update.param("Version").map(str::to_string);
    }
    control_plane.updatable = busy.is_none();
    pools.push(control_plane);
    for ng in &snap.nodegroups {
        pools.push(nodegroup_pool(
            ng,
            &version,
            snap.nodegroup_updates.get(&ng.nodegroup_name),
        ));
    }

    let addons = snap
        .addons
        .iter()
        .map(|a| {
            addon_row(
                a,
                snap.addon_versions.get(&a.addon_name).map(Vec::as_slice),
                &addon_target,
            )
        })
        .collect();

    let progress = match running {
        Some(update) if update.kind == "VersionUpdate" => Some(Progress {
            target: update.param("Version").unwrap_or_default().to_string(),
            percent: None,
            message: format!(
                "EKS is updating the control plane to {} (update {})",
                update.param("Version").unwrap_or("a new version"),
                update.id
            ),
            started: update.created(),
            failing: update.error(),
        }),
        _ => snap
            .nodegroup_updates
            .iter()
            .filter(|(_, u)| u.running())
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(name, update)| Progress {
                target: update.param("Version").unwrap_or_default().to_string(),
                percent: None,
                message: format!("EKS is updating node group {name} (update {})", update.id),
                started: update.created(),
                failing: update.error(),
            }),
    };

    let mut notes = Vec::new();
    for issue in cluster.health.iter().flat_map(|h| &h.issues) {
        notes.push(Note {
            warning: true,
            title: "Cluster health".into(),
            text: format!(
                "{}: {}",
                issue.code.as_deref().unwrap_or("Issue"),
                issue.message.as_deref().unwrap_or_default()
            ),
            command: None,
            url: None,
        });
    }
    if let Some(update) = running.filter(|u| u.kind != "VersionUpdate") {
        notes.push(Note {
            warning: false,
            title: "Update running".into(),
            text: format!("EKS is applying a {} (update {}).", update.kind, update.id),
            command: None,
            url: None,
        });
    }
    if let Some(failed) = snap
        .updates
        .iter()
        .find(|u| u.kind == "VersionUpdate" && !u.running())
        .filter(|u| u.status == "Failed")
    {
        notes.push(Note {
            warning: true,
            title: "The last version update failed".into(),
            text: format!(
                "Update {} to {}: {}",
                failed.id,
                failed.param("Version").unwrap_or("?"),
                failed.error().unwrap_or_else(|| "no details".into())
            ),
            command: None,
            url: None,
        });
    }
    if snap.versions.is_none()
        && let Some(next) = targets.first()
    {
        notes.push(Note {
            warning: false,
            title: "EKS versions".into(),
            text: format!(
                "Kubyl couldn't list EKS versions, so it assumes {} is next.",
                next.version
            ),
            command: None,
            url: Some(DOCS_VERSIONS.into()),
        });
    }
    notes.extend(snap.notes.iter().cloned());

    Status {
        provider: target.label(),
        current: Current {
            version,
            kubernetes: None,
            platform: cluster.platform_version.clone(),
            channel: None,
            channels: Vec::new(),
            cluster_id: cluster.arn.clone(),
            support: support(version_info, support_type, now),
            conditions: Vec::new(),
        },
        targets,
        history: history(&snap.updates),
        progress,
        components: Vec::new(),
        pools,
        addons,
        notes,
        writes: Writes {
            control_plane: true,
            pools: !snap.nodegroups.is_empty(),
            channel: false,
            addons: !snap.addons.is_empty(),
            reason: None,
        },
        docs: vec![("EKS Kubernetes versions".into(), DOCS_VERSIONS.into())],
    }
}

// ---------------------------------------------------------------------------------------------
// Pre-flight checks

/// Whether each add-on supports `target`: a warning names the version to update to.
/// `versions` is `None` for an add-on whose versions Kubyl couldn't read.
pub fn addon_check(
    eks: &EksTarget,
    addons: &[(String, String, Option<Vec<AddonVersion>>)],
    target: &str,
) -> Check {
    let title = "EKS add-ons";
    if addons.is_empty() {
        return Check::new(
            "eks-addons",
            title,
            CheckStatus::Info,
            "No EKS add-ons are installed.",
        );
    }
    let mut details = Vec::new();
    let mut compatible = 0;
    for (name, installed, versions) in addons {
        let Some(versions) = versions else {
            details.push(Detail::new(
                CheckStatus::Unknown,
                format!("{name} {installed}: Kubyl couldn't read its versions"),
            ));
            continue;
        };
        if supports(versions, installed, target) {
            compatible += 1;
            details.push(Detail::new(
                CheckStatus::Pass,
                format!("{name} {installed} supports {target}"),
            ));
            continue;
        }
        let mut detail = Detail::new(
            CheckStatus::Warn,
            format!("{name} {installed} isn't listed for {target}"),
        );
        if let Some(recommended) = recommended_version(versions, target) {
            detail = detail
                .with_sub(format!("EKS recommends {recommended} for {target}"))
                .fix(Fix::Copy {
                    label: "Copy update command".into(),
                    text: format!(
                        "aws eks update-addon --cluster-name {} --addon-name {name} \
                         --addon-version {recommended} --resolve-conflicts PRESERVE{}",
                        eks.cluster,
                        eks.cli_suffix()
                    ),
                });
        }
        details.push(detail);
    }
    let status = details
        .iter()
        .map(|d| d.status)
        .min()
        .unwrap_or(CheckStatus::Pass);
    Check::new(
        "eks-addons",
        title,
        status,
        format!("{compatible} of {} add-ons support {target}", addons.len()),
    )
    .details(details)
}

/// EKS's upgrade-readiness insights for `target`.
pub fn insights_check(insights: Result<Vec<Insight>, ProviderError>, target: &str) -> Check {
    let title = "EKS upgrade insights";
    let insights = match insights {
        Ok(insights) => insights,
        Err(ProviderError::Forbidden { verb, .. }) => {
            return Check::new(
                "eks-insights",
                title,
                CheckStatus::Unknown,
                format!("Kubyl can't read EKS insights (missing {verb})."),
            );
        }
        Err(err) => {
            return Check::new(
                "eks-insights",
                title,
                CheckStatus::Unknown,
                format!("Kubyl couldn't read EKS insights: {err}"),
            );
        }
    };
    let relevant: Vec<&Insight> = insights
        .iter()
        .filter(|i| {
            i.category
                .as_deref()
                .is_none_or(|c| c == "UPGRADE_READINESS")
                && i.kubernetes_version
                    .as_deref()
                    .is_none_or(|v| same_minor(v, target))
        })
        .collect();
    if relevant.is_empty() {
        return Check::new(
            "eks-insights",
            title,
            CheckStatus::Info,
            format!("EKS has no upgrade insights for {target} yet (it refreshes them daily)."),
        );
    }
    let mut counts: [usize; 4] = [0; 4];
    let details: Vec<Detail> = relevant
        .iter()
        .map(|insight| {
            let state = insight
                .insight_status
                .as_ref()
                .and_then(|s| s.status.as_deref());
            let status = match state {
                Some("ERROR") => CheckStatus::Fail,
                Some("WARNING") => CheckStatus::Warn,
                Some("PASSING") => CheckStatus::Pass,
                _ => CheckStatus::Unknown,
            };
            counts[match status {
                CheckStatus::Fail => 0,
                CheckStatus::Warn => 1,
                CheckStatus::Pass => 2,
                _ => 3,
            }] += 1;
            let mut detail = Detail::new(status, insight.name.clone());
            if let Some(reason) = insight
                .insight_status
                .as_ref()
                .and_then(|s| s.reason.clone())
                .or(insight.description.clone())
            {
                detail = detail.with_sub(reason);
            }
            detail
        })
        .collect();
    let status = details
        .iter()
        .map(|d| d.status)
        .min()
        .unwrap_or(CheckStatus::Pass);
    let mut summary = Vec::new();
    for (n, label) in counts
        .iter()
        .zip(["error", "warning", "passing", "unknown"])
    {
        match (n, label) {
            (0, _) => {}
            (1, l) | (_, l @ "passing") | (_, l @ "unknown") => summary.push(format!("{n} {l}")),
            (n, l) => summary.push(format!("{n} {l}s")),
        }
    }
    Check::new(
        "eks-insights",
        title,
        status,
        format!("EKS upgrade insights for {target}: {}", summary.join(", ")),
    )
    .details(details)
    .fix(Fix::Url {
        label: "EKS cluster insights".into(),
        url: DOCS_INSIGHTS.into(),
    })
}

// ---------------------------------------------------------------------------------------------
// Writes

/// The write a plan runs, kept in [`Plan::request`].
fn request(
    operation: &str,
    path: String,
    body: Value,
    permission: &str,
    resource: String,
) -> Value {
    json!({
        "operation": operation,
        "path": path,
        "body": body,
        "permission": permission,
        "resource": resource,
    })
}

/// What a write would do. `version` is the Kubernetes minor (control plane, node group) or an
/// add-on version (`v1.19.0-eksbuild.1`; else the add-on's recommended one).
pub fn plan(
    eks: &EksTarget,
    status: &Status,
    scope: &Scope,
    version: &str,
    cluster: &str,
    snapshot: Option<&Snapshot>,
) -> Result<Plan, String> {
    let current = status.current.version.clone();
    let name = encode(&eks.cluster);
    let resource = format!("cluster {}", eks.cluster);
    match scope {
        Scope::ControlPlane => {
            let to = minor(version).ok_or_else(|| format!("{version} isn't a version"))?;
            let target = status
                .targets
                .iter()
                .find(|t| same_minor(&t.version, &to))
                .ok_or_else(|| format!("EKS doesn't offer {to} for {cluster}."))?;
            if !target.startable() {
                return Err(target.blocked.join("; "));
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update the control plane of {cluster} to {to}"),
                from: current.clone(),
                to: to.clone(),
                kind_label: "minor update".into(),
                changes: vec![format!(
                    "UpdateClusterVersion {} → {to} (like aws eks update-cluster-version \
                     --name {} --kubernetes-version {to}{})",
                    eks.cluster,
                    eks.cluster,
                    eks.cli_suffix()
                )],
                risks: Vec::new(),
                notes: vec![
                    format!(
                        "Node groups keep {current} until you update them; add-ons keep \
                         their versions."
                    ),
                    "EKS refuses the update while an upgrade insight fails; Kubyl never forces \
                     it."
                    .into(),
                    "This can't be undone: EKS can't downgrade a control plane.".into(),
                ],
                irreversible: true,
                request: request(
                    "UpdateClusterVersion",
                    format!("/clusters/{name}/updates"),
                    json!({ "version": to }),
                    "eks:UpdateClusterVersion",
                    resource,
                ),
            })
        }
        Scope::Pool(id) => {
            let ng_name = id
                .strip_prefix("nodegroup/")
                .ok_or_else(|| format!("{id} isn't an EKS node group"))?;
            let pool = status
                .pools
                .iter()
                .find(|p| &p.id == id)
                .ok_or_else(|| format!("{cluster} has no node group {ng_name}"))?;
            if let Some(ng) =
                snapshot.and_then(|s| s.nodegroups.iter().find(|n| n.nodegroup_name == ng_name))
                && custom_ami(ng)
            {
                return Err(format!(
                    "Node group {ng_name} runs a custom AMI from its launch template: update \
                     the launch template instead."
                ));
            }
            let to = minor(version).ok_or_else(|| format!("{version} isn't a version"))?;
            let to_key = version::minor_of(&to);
            if to_key > version::minor_of(&current) {
                return Err(format!(
                    "Update the control plane to {to} first: a node group can't be newer than \
                     the control plane ({current})."
                ));
            }
            let from = pool.version.clone().unwrap_or_default();
            if to_key < version::minor_of(&from) {
                return Err(format!("EKS can't downgrade node group {ng_name} to {to}."));
            }
            let mut notes = vec![format!(
                "EKS replaces the nodes{}: pods are drained and PodDisruptionBudgets are \
                 respected (Kubyl never forces).",
                pool.surge
                    .as_ref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default()
            )];
            if let Some(template) = snapshot
                .and_then(|s| s.nodegroups.iter().find(|n| n.nodegroup_name == ng_name))
                .and_then(|n| n.launch_template.as_ref())
            {
                notes.push(format!(
                    "Launch template {} (version {}) stays; EKS picks the AMI for {to}.",
                    template
                        .name
                        .as_deref()
                        .or(template.id.as_deref())
                        .unwrap_or("?"),
                    template.version.as_deref().unwrap_or("?")
                ));
            }
            notes.push("This can't be undone: EKS can't downgrade a node group.".into());
            let same = same_minor(&from, &to);
            Ok(Plan {
                scope: scope.clone(),
                title: if same {
                    format!("Update node group {ng_name} to the latest {to} AMI")
                } else {
                    format!("Update node group {ng_name} to {to}")
                },
                from,
                to: to.clone(),
                kind_label: if same {
                    "AMI update".into()
                } else {
                    "node group update".into()
                },
                changes: vec![format!(
                    "UpdateNodegroupVersion {ng_name} → {to} (like aws eks \
                     update-nodegroup-version --cluster-name {} --nodegroup-name {ng_name} \
                     --kubernetes-version {to}{})",
                    eks.cluster,
                    eks.cli_suffix()
                )],
                risks: Vec::new(),
                notes,
                irreversible: true,
                request: request(
                    "UpdateNodegroupVersion",
                    format!(
                        "/clusters/{name}/node-groups/{}/update-version",
                        encode(ng_name)
                    ),
                    json!({ "version": to }),
                    "eks:UpdateNodegroupVersion",
                    format!("node group {ng_name} of {resource}"),
                ),
            })
        }
        Scope::AddOn(addon_name) => {
            let addon = status
                .addons
                .iter()
                .find(|a| &a.name == addon_name)
                .ok_or_else(|| format!("{cluster} has no add-on {addon_name}"))?;
            let to = if version.starts_with('v') && version.contains("eksbuild") {
                version.to_string()
            } else {
                addon.recommended.clone().ok_or_else(|| {
                    format!("Kubyl doesn't know which version of {addon_name} to install.")
                })?
            };
            if to == addon.version {
                return Err(format!("{addon_name} already runs {to}."));
            }
            let mut risks = Vec::new();
            if let Some(versions) = snapshot.and_then(|s| s.addon_versions.get(addon_name))
                && !supports(versions, &to, &current)
            {
                risks.push(Risk {
                    name: format!("{to} isn't listed for {current}"),
                    message: format!(
                        "EKS doesn't list {to} as compatible with the cluster's current version \
                         {current}. Update the control plane first unless you know it works."
                    ),
                    url: None,
                });
            }
            Ok(Plan {
                scope: scope.clone(),
                title: format!("Update add-on {addon_name} to {to}"),
                from: addon.version.clone(),
                to: to.clone(),
                kind_label: "add-on update".into(),
                changes: vec![format!(
                    "UpdateAddon {addon_name} {} → {to}, resolveConflicts PRESERVE (like aws eks \
                     update-addon --cluster-name {} --addon-name {addon_name} --addon-version \
                     {to} --resolve-conflicts PRESERVE{})",
                    addon.version,
                    eks.cluster,
                    eks.cli_suffix()
                )],
                risks,
                notes: vec![
                    "PRESERVE keeps settings you changed on the add-on's objects.".into(),
                    "This can't be undone: EKS can't downgrade an add-on.".into(),
                ],
                irreversible: true,
                request: request(
                    "UpdateAddon",
                    format!("/clusters/{name}/addons/{}/update", encode(addon_name)),
                    json!({ "addonVersion": to, "resolveConflicts": "PRESERVE" }),
                    "eks:UpdateAddon",
                    format!("add-on {addon_name} of {resource}"),
                ),
            })
        }
        Scope::AllPools => Err("On EKS, update node groups one at a time.".into()),
        Scope::Channel(_) => Err("EKS has no update channels.".into()),
    }
}

// ---------------------------------------------------------------------------------------------
// The provider

struct Eks {
    inner: Arc<Inner>,
}

struct Inner {
    ctx: CloudContext,
    http: Result<Http, ProviderError>,
    /// The API endpoint when overridden ([`cloud::EKS_ENDPOINT`], tests), or why the override
    /// is invalid.
    base: Result<Option<Url>, ProviderError>,
    target: Mutex<Option<EksTarget>>,
    credentials: Cache<AwsCredentials>,
    /// Credentials given up front (tests).
    preset: Option<AwsCredentials>,
    /// Updates that finished, by id (they don't change any more).
    finished: Mutex<HashMap<String, Update>>,
    addon_versions: Mutex<HashMap<String, (Timestamp, Vec<AddonVersion>)>>,
    last: Mutex<Option<Arc<Snapshot>>>,
}

/// A secondary read that failed: a note, and the read goes on.
fn note_for(what: &str, err: &ProviderError) -> Note {
    Note {
        warning: true,
        title: format!("Couldn't read {what}"),
        text: err.to_string(),
        command: match err {
            ProviderError::Credentials { command, .. } => command.clone(),
            _ => None,
        },
        url: None,
    }
}

impl Inner {
    fn new(
        ctx: CloudContext,
        target: Option<EksTarget>,
        base: Result<Option<Url>, ProviderError>,
        preset: Option<AwsCredentials>,
    ) -> Self {
        Self {
            ctx,
            http: Http::new(),
            base,
            target: Mutex::new(target),
            credentials: Cache::default(),
            preset,
            finished: Mutex::default(),
            addon_versions: Mutex::default(),
            last: Mutex::default(),
        }
    }

    async fn target(&self) -> Result<EksTarget, ProviderError> {
        if let Some(target) = self.target.lock().unwrap().clone() {
            return Ok(target);
        }
        let exec = cloud::load_exec_info(&self.ctx).await;
        let hints = exec.as_ref().map(hints).unwrap_or_default();
        let target = resolve(
            self.ctx.settings.eks.as_ref(),
            &hints,
            &self.ctx.context,
            &self.ctx.cluster_entry,
            &self.ctx.server,
        )?;
        *self.target.lock().unwrap() = Some(target.clone());
        Ok(target)
    }

    async fn api(&self) -> Result<(EksTarget, Api), ProviderError> {
        let target = self.target().await?;
        let base = match &self.base {
            Ok(Some(base)) => base.clone(),
            Ok(None) => Url::parse(&target.endpoint())
                .map_err(|err| ProviderError::Other(format!("Invalid EKS endpoint: {err}")))?,
            Err(err) => return Err(err.clone()),
        };
        let credentials = match &self.preset {
            Some(preset) => preset.clone(),
            None => self.credentials.get(|| credentials::fetch(&target)).await?,
        };
        let api = Api {
            http: self.http.clone()?,
            base,
            region: target.region.clone(),
            cluster: target.cluster.clone(),
            credentials,
            login: target.login(),
        };
        Ok((target, api))
    }

    /// Drops credentials the API rejected, so the next read runs the CLI again.
    async fn forget_rejected<T>(&self, result: &Result<T, ProviderError>) {
        if let Err(ProviderError::Credentials { .. }) = result {
            self.credentials.clear().await;
        }
    }

    async fn addon_versions(
        &self,
        api: &Api,
        name: &str,
    ) -> Result<Vec<AddonVersion>, ProviderError> {
        if let Some((at, versions)) = self.addon_versions.lock().unwrap().get(name)
            && Timestamp::now() < *at + ADDON_VERSIONS_FOR
        {
            return Ok(versions.clone());
        }
        let versions = api.addon_versions(name).await?;
        self.addon_versions
            .lock()
            .unwrap()
            .insert(name.to_string(), (Timestamp::now(), versions.clone()));
        Ok(versions)
    }

    /// Describes the updates in `ids` that aren't known to be finished; running ones come back
    /// fresh every time.
    async fn updates(&self, api: &Api, ids: &[String], nodegroup: Option<&str>) -> Vec<Update> {
        let (mut known, unknown): (Vec<Update>, Vec<&String>) = {
            let finished = self.finished.lock().unwrap();
            let mut known = Vec::new();
            let mut unknown = Vec::new();
            for id in ids {
                match finished.get(id) {
                    Some(update) => known.push(update.clone()),
                    None => unknown.push(id),
                }
            }
            (known, unknown)
        };
        let described = join_all(
            unknown
                .into_iter()
                .take(DESCRIBE_UPDATES)
                .map(|id| api.describe_update(id, nodegroup)),
        )
        .await;
        let mut finished = self.finished.lock().unwrap();
        for update in described.into_iter().flatten() {
            if !update.running() {
                finished.insert(update.id.clone(), update.clone());
            }
            known.push(update);
        }
        known.sort_by_key(|u| std::cmp::Reverse(u.created()));
        known
    }

    async fn snapshot(&self, api: &Api) -> Result<Snapshot, ProviderError> {
        let cluster = api.describe_cluster().await?;
        let mut notes = Vec::new();

        let nodegroups = async {
            let names = api.nodegroup_names().await?;
            let described = join_all(names.iter().map(|n| api.describe_nodegroup(n))).await;
            let mut first_error = None;
            let mut nodegroups = Vec::new();
            for result in described {
                match result {
                    Ok(ng) => nodegroups.push(ng),
                    Err(err) => {
                        first_error.get_or_insert(err);
                    }
                }
            }
            Ok::<_, ProviderError>((nodegroups, first_error))
        };
        let addons = async {
            let names = api.addon_names().await?;
            let described = join_all(names.iter().map(|n| api.describe_addon(n))).await;
            Ok::<_, ProviderError>(described.into_iter().flatten().collect::<Vec<_>>())
        };
        let updates = async {
            let ids = api.update_ids(None).await?;
            Ok::<_, ProviderError>(self.updates(api, &ids, None).await)
        };
        let (nodegroups, addons, versions, updates) =
            futures::join!(nodegroups, addons, api.cluster_versions(), updates);

        let nodegroups = match nodegroups {
            Ok((nodegroups, error)) => {
                if let Some(err) = error {
                    notes.push(note_for("some node groups", &err));
                }
                nodegroups
            }
            Err(err) => {
                notes.push(note_for("the node groups", &err));
                Vec::new()
            }
        };
        let addons = addons.unwrap_or_else(|err| {
            notes.push(note_for("the add-ons", &err));
            Vec::new()
        });
        let versions = match versions {
            Ok(versions) => Some(versions),
            Err(ProviderError::Forbidden { .. }) => None,
            Err(err) => {
                notes.push(note_for("the EKS versions", &err));
                None
            }
        };
        let updates = updates.unwrap_or_else(|err| {
            notes.push(note_for("the cluster's updates", &err));
            Vec::new()
        });

        let mut addon_versions = HashMap::new();
        let fetched = join_all(addons.iter().map(|a| async {
            (
                a.addon_name.clone(),
                self.addon_versions(api, &a.addon_name).await,
            )
        }))
        .await;
        for (name, result) in fetched {
            match result {
                Ok(versions) => {
                    addon_versions.insert(name, versions);
                }
                Err(err) if addon_versions.is_empty() => {
                    notes.push(note_for("the add-on versions", &err));
                }
                Err(_) => {}
            }
        }

        let mut nodegroup_updates = HashMap::new();
        for ng in nodegroups
            .iter()
            .filter(|n| n.status.as_deref() == Some("UPDATING"))
        {
            let Ok(ids) = api.update_ids(Some(&ng.nodegroup_name)).await else {
                continue;
            };
            if let Some(update) = self
                .updates(api, &ids, Some(&ng.nodegroup_name))
                .await
                .into_iter()
                .find(Update::running)
            {
                nodegroup_updates.insert(ng.nodegroup_name.clone(), update);
            }
        }

        Ok(Snapshot {
            cluster,
            nodegroups,
            addons,
            addon_versions,
            versions,
            updates,
            nodegroup_updates,
            notes,
        })
    }

    async fn read(&self) -> Result<Status, ProviderError> {
        let (target, api) = self.api().await?;
        let result = self.snapshot(&api).await;
        self.forget_rejected(&result).await;
        let snapshot = result?;
        let status = status(&target, &snapshot, Timestamp::now());
        *self.last.lock().unwrap() = Some(Arc::new(snapshot));
        Ok(status)
    }

    async fn preflight(&self, status: Status, target: String) -> Vec<Check> {
        let Some(to) = minor(&target) else {
            return Vec::new();
        };
        let (eks, api) = match self.api().await {
            Ok(api) => api,
            Err(err) => {
                return vec![Check::new(
                    "eks-addons",
                    "EKS add-ons",
                    CheckStatus::Unknown,
                    format!("Kubyl couldn't reach the EKS API: {err}"),
                )];
            }
        };
        let addons = join_all(status.addons.iter().map(|a| async {
            (
                a.name.clone(),
                a.version.clone(),
                self.addon_versions(&api, &a.name).await.ok(),
            )
        }))
        .await;
        let insights = api.insights(&to).await;
        self.forget_rejected(&insights).await;
        vec![
            addon_check(&eks, &addons, &to),
            insights_check(insights, &to),
        ]
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
        let last = self.last.lock().unwrap().clone();
        plan(&target, status, scope, version, cluster, last.as_deref())
    }

    async fn start(&self, plan: Plan) -> Result<String, ProviderError> {
        let field = |name: &str| {
            plan.request
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| ProviderError::Other("This isn't an EKS plan.".into()))
        };
        let (operation, path) = (field("operation")?, field("path")?);
        let action = Action::new(field("permission")?, field("resource")?);
        let body = plan.request.get("body").cloned().unwrap_or(Value::Null);
        let (_, api) = self.api().await?;
        let result = api.write(&path, &body, &action).await;
        self.forget_rejected(&result).await;
        let update = result?;
        Ok(format!(
            "EKS accepted {operation} ({} → {}): update {} is {}.",
            plan.from, plan.to, update.id, update.status
        ))
    }
}

impl UpdateProvider for Eks {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Eks
    }

    fn read(&self) -> ProviderFuture<Result<Status, ProviderError>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.read().await })
    }

    fn preflight_extras(&self, status: &Status, target: &str) -> ProviderFuture<Vec<Check>> {
        let inner = self.inner.clone();
        let (status, target) = (status.clone(), target.to_string());
        Box::pin(async move { inner.preflight(status, target).await })
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
