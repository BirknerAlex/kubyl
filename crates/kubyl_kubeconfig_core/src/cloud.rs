//! Account-wide cluster discovery through the cloud CLIs the user already has (phase 25): list
//! every cluster of every AWS profile, Azure subscription and Google Cloud project, then add
//! the chosen ones with the CLIs' own `get-credentials` commands.
//!
//! The CLIs are the only thing that talks to the clouds: `aws configure list-profiles` +
//! `aws eks list-clusters/describe-cluster` per region, `az account list` + `az aks list`,
//! `gcloud projects list` + `gcloud container clusters list`. Kubyl does no OAuth of its own and
//! persists nothing: accounts and clusters live in memory while the dialog is open, and the
//! kubeconfig the CLIs write goes into a private temp folder until the user saves it.
//!
//! Like Kubyl's other CLI use (phase 13): found through the login shell's `PATH`, never
//! interactive (no stdin, prompts off), a timeout per command, no console window on Windows.
//! A missing CLI, an expired session (`aws sso login`) or a denied permission becomes a
//! [`Failure`] that says what to run or which permission is missing.
//!
//! [`CliRunner`] is the seam for tests: [`SystemRunner`] runs the real programs (tests put fake
//! `aws`/`az`/`gcloud` scripts first on its search path), anything else can stand in.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt as _;
use futures::future::BoxFuture;
use serde_json::Value;

/// How long one CLI command may take.
pub const TIMEOUT: Duration = Duration::from_secs(60);
/// How many accounts, regions and clusters are asked at once.
const CONCURRENCY: usize = 6;

/// A cloud and its CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    Aws,
    Azure,
    Google,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Aws, Provider::Azure, Provider::Google];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Aws => "AWS EKS",
            Provider::Azure => "Azure AKS",
            Provider::Google => "Google GKE",
        }
    }

    /// The program that does the work.
    pub fn cli(self) -> &'static str {
        match self {
            Provider::Aws => "aws",
            Provider::Azure => "az",
            Provider::Google => "gcloud",
        }
    }

    /// What an account is called here.
    pub fn account_label(self) -> &'static str {
        match self {
            Provider::Aws => "profile",
            Provider::Azure => "subscription",
            Provider::Google => "project",
        }
    }

    /// Where to get the CLI.
    pub fn install_url(self) -> &'static str {
        match self {
            Provider::Aws => {
                "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html"
            }
            Provider::Azure => "https://learn.microsoft.com/cli/azure/install-azure-cli",
            Provider::Google => "https://cloud.google.com/sdk/docs/install",
        }
    }

    /// The command that signs in, for messages.
    pub fn sign_in(self, account: Option<&str>) -> String {
        match (self, account) {
            (Provider::Aws, Some(profile)) => format!("aws sso login --profile {profile}"),
            (Provider::Aws, None) => "aws sso login".into(),
            (Provider::Azure, _) => "az login".into(),
            (Provider::Google, _) => "gcloud auth login".into(),
        }
    }
}

/// A profile, subscription or project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub provider: Provider,
    /// What the CLI takes as the account: the profile name, the subscription id, the project id.
    pub id: String,
    /// What it's called (a subscription's or project's display name; the profile name).
    pub label: String,
    /// More to tell accounts apart: the AWS account id, the Azure tenant.
    pub detail: String,
}

/// A cluster found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cluster {
    pub provider: Provider,
    pub account: String,
    pub name: String,
    /// The AWS region, the GKE location (region or zone), the AKS location.
    pub region: String,
    /// The AKS resource group.
    pub group: Option<String>,
    pub version: String,
    /// `ACTIVE`, `RUNNING`, `Running`…
    pub status: String,
}

impl Cluster {
    /// Identifies the cluster across scans.
    pub fn key(&self) -> String {
        format!(
            "{:?}/{}/{}/{}",
            self.provider, self.account, self.region, self.name
        )
    }
}

/// Why a command didn't give what was asked, with what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    NotInstalled(Provider),
    /// The session is missing or expired: `command` signs in.
    SignIn {
        command: String,
        why: String,
    },
    /// The account isn't allowed to do it.
    Permission {
        permission: String,
    },
    /// Google: the Kubernetes Engine API is off in the project (the project simply has no GKE).
    ApiDisabled {
        project: String,
    },
    Timeout,
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::NotInstalled(p) => write!(
                f,
                "`{}` isn't installed, or isn't in your login shell's PATH. Install it ({}), then scan again.",
                p.cli(),
                p.install_url()
            ),
            Failure::SignIn { command, why } => {
                write!(f, "{why} Run `{command}`, then scan again.")
            }
            Failure::Permission { permission } => write!(
                f,
                "Missing permission: {permission}. Ask for a role that allows it."
            ),
            Failure::ApiDisabled { project } => write!(
                f,
                "The Kubernetes Engine API is off in project {project}: `gcloud services enable container.googleapis.com --project {project}` turns it on."
            ),
            Failure::Timeout => write!(f, "The command didn't finish in {} s.", TIMEOUT.as_secs()),
            Failure::Other(message) => f.write_str(message),
        }
    }
}

impl Failure {
    /// Whether this is only "nothing here" rather than something wrong (a project without GKE).
    pub fn is_benign(&self) -> bool {
        matches!(self, Failure::ApiDisabled { .. })
    }
}

/// A command's output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Why a command couldn't run at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    NotInstalled,
    Timeout,
    Io(String),
}

/// Runs a CLI. `env` is added to the process's environment.
pub trait CliRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
        env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<RunOutput, RunError>>;
}

/// Runs the real programs: found in `search_path` (default the login shell's `PATH`), no stdin,
/// prompts and pagers off, a timeout.
pub struct SystemRunner {
    pub timeout: Duration,
    /// Where programs are looked for; `None`: the login shell's `PATH` (tests point it at a
    /// folder of fake CLIs).
    pub search_path: Option<OsString>,
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self {
            timeout: TIMEOUT,
            search_path: None,
        }
    }
}

impl SystemRunner {
    /// The PATH to search and run with. The login shell that gives it blocks, so it runs on the
    /// blocking pool (once; the concurrent scans wait for the same answer).
    async fn path(&self) -> Option<std::ffi::OsString> {
        match &self.search_path {
            Some(path) => Some(path.clone()),
            None => tokio::task::spawn_blocking(kubyl_kube_core::auth::shell_env::path)
                .await
                .ok()
                .flatten(),
        }
    }

    fn find(path: &std::ffi::OsStr, program: &str) -> Option<PathBuf> {
        for dir in std::env::split_paths(path) {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(windows)]
            for ext in ["exe", "cmd", "bat"] {
                let candidate = dir.join(format!("{program}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

impl CliRunner for SystemRunner {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [String],
        env: &'a [(String, String)],
    ) -> BoxFuture<'a, Result<RunOutput, RunError>> {
        Box::pin(async move {
            let shell_path = self.path().await;
            let path = shell_path
                .as_deref()
                .and_then(|path| Self::find(path, program))
                .ok_or(RunError::NotInstalled)?;
            let mut command = tokio::process::Command::new(path);
            command
                .args(args)
                // Never interactive: no stdin, and the CLIs' prompts and pagers off.
                .stdin(std::process::Stdio::null())
                .env("AWS_PAGER", "")
                .env("CLOUDSDK_CORE_DISABLE_PROMPTS", "1")
                .env("CLOUDSDK_CORE_DISABLE_USAGE_REPORTING", "true")
                .env("AZURE_CORE_NO_COLOR", "true")
                .env("AZURE_CORE_ONLY_SHOW_ERRORS", "true")
                .env("AZURE_CORE_COLLECT_TELEMETRY", "false")
                .envs(env.iter().map(|(k, v)| (k, v)))
                .kill_on_drop(true);
            if let Some(shell_path) = &shell_path {
                command.env("PATH", shell_path);
            }
            #[cfg(windows)]
            {
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            let output = tokio::time::timeout(self.timeout, command.output())
                .await
                .map_err(|_| RunError::Timeout)?
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => RunError::NotInstalled,
                    _ => RunError::Io(e.to_string()),
                })?;
            Ok(RunOutput {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Errors

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("the command failed without a message");
    // Without terminal colors, and not a whole traceback.
    let mut clean = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if !c.is_control() {
            clean.push(c);
        }
    }
    clean.chars().take(300).collect()
}

/// The text after `marker` up to a quote, space or end (a permission name).
fn after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let rest = rest.trim_start_matches([' ', '\'', '"', ':']);
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ',' | ')'))
        .unwrap_or(rest.len());
    Some(&rest[..end]).filter(|s| !s.is_empty())
}

/// What a failed command's stderr means. `account` names the profile, subscription or project
/// it ran for.
pub fn classify(provider: Provider, account: &str, stderr: &str) -> Failure {
    let lower = stderr.to_lowercase();
    let sign_in = |why: &str| Failure::SignIn {
        command: provider.sign_in(Some(account)),
        why: why.to_string(),
    };
    match provider {
        Provider::Aws => {
            if lower.contains("aws sso login")
                || lower.contains("sso session")
                || lower.contains("error loading sso token")
                || lower.contains("token has expired")
            {
                return sign_in("The SSO session expired.");
            }
            if lower.contains("unable to locate credentials")
                || lower.contains("nocredentialproviders")
                || lower.contains("could not be found") && lower.contains("profile")
            {
                return Failure::SignIn {
                    command: format!("aws configure --profile {account}"),
                    why: format!("No credentials for profile {account}."),
                };
            }
            if lower.contains("expiredtoken")
                || lower.contains("security token included in the request is expired")
                || lower.contains("credentials have expired")
            {
                return sign_in("The credentials expired.");
            }
            if lower.contains("not authorized to perform")
                || lower.contains("accessdenied")
                || lower.contains("unauthorizedoperation")
            {
                let permission = after(stderr, "not authorized to perform:")
                    .or_else(|| after(stderr, "not authorized to perform"))
                    .unwrap_or("the action the CLI named")
                    .to_string();
                return Failure::Permission { permission };
            }
        }
        Provider::Azure => {
            if lower.contains("az login")
                || lower.contains("aadsts")
                || lower.contains("interactive authentication is needed")
                || lower.contains("refresh token has expired")
            {
                return sign_in("Azure needs a sign-in (none yet, or it expired).");
            }
            if lower.contains("authorizationfailed")
                || lower.contains("does not have authorization")
            {
                let permission = after(stderr, "perform action")
                    .unwrap_or("Microsoft.ContainerService/managedClusters/read")
                    .to_string();
                return Failure::Permission { permission };
            }
        }
        Provider::Google => {
            if lower.contains("gcloud auth login")
                || lower.contains("reauthentication")
                || lower.contains("do not currently have an active account")
                || lower.contains("invalid_grant")
            {
                return sign_in("Google Cloud needs a sign-in (none yet, or it expired).");
            }
            if lower.contains("has not been used in project")
                || lower.contains("api is not enabled")
                || lower.contains("is disabled") && lower.contains("container.googleapis.com")
                || lower.contains("not enabled on project")
            {
                return Failure::ApiDisabled {
                    project: account.to_string(),
                };
            }
            if lower.contains("permission") && (lower.contains("denied") || lower.contains("403")) {
                let permission = after(stderr, "permission")
                    .filter(|p| p.contains('.'))
                    .unwrap_or("container.clusters.list")
                    .to_string();
                return Failure::Permission { permission };
            }
        }
    }
    Failure::Other(first_line(stderr))
}

fn run_failure(provider: Provider, error: RunError) -> Failure {
    match error {
        RunError::NotInstalled => Failure::NotInstalled(provider),
        RunError::Timeout => Failure::Timeout,
        RunError::Io(message) => {
            Failure::Other(format!("Couldn't run `{}`: {message}", provider.cli()))
        }
    }
}

/// Runs a command and returns its stdout, or what went wrong.
async fn output(
    runner: &dyn CliRunner,
    provider: Provider,
    account: &str,
    args: &[&str],
    env: &[(String, String)],
) -> Result<String, Failure> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = runner
        .run(provider.cli(), &args, env)
        .await
        .map_err(|e| run_failure(provider, e))?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(classify(provider, account, &out.stderr))
    }
}

fn json(text: &str) -> Result<Value, Failure> {
    serde_json::from_str(text)
        .map_err(|_| Failure::Other(format!("Unexpected output: {}", first_line(text))))
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// Accounts

/// The accounts of a cloud: AWS profiles, Azure subscriptions (enabled ones), Google projects
/// (active ones).
pub async fn accounts(provider: Provider, runner: &dyn CliRunner) -> Result<Vec<Account>, Failure> {
    match provider {
        Provider::Aws => {
            let text = output(runner, provider, "", &["configure", "list-profiles"], &[]).await?;
            Ok(text
                .lines()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| Account {
                    provider,
                    id: p.to_string(),
                    label: p.to_string(),
                    detail: String::new(),
                })
                .collect())
        }
        Provider::Azure => {
            let text = output(
                runner,
                provider,
                "",
                &["account", "list", "--output", "json"],
                &[],
            )
            .await?;
            let list = json(&text)?;
            Ok(list
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter(|s| {
                    s.get("state")
                        .and_then(Value::as_str)
                        .is_none_or(|st| st == "Enabled")
                })
                .map(|s| Account {
                    provider,
                    id: string(s, "id"),
                    label: string(s, "name"),
                    detail: string(s, "tenantId"),
                })
                .filter(|a| !a.id.is_empty())
                .collect())
        }
        Provider::Google => {
            let text = output(
                runner,
                provider,
                "",
                &["projects", "list", "--format=json"],
                &[],
            )
            .await?;
            let list = json(&text)?;
            Ok(list
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter(|p| {
                    p.get("lifecycleState")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s == "ACTIVE")
                })
                .map(|p| Account {
                    provider,
                    id: string(p, "projectId"),
                    label: string(p, "name"),
                    detail: string(p, "projectNumber"),
                })
                .filter(|a| !a.id.is_empty())
                .collect())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Clusters

/// Something that went wrong in a part of an account (a region), without ending the scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The region, or empty for the account.
    pub scope: String,
    pub failure: Failure,
}

/// What one account holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    pub account: Account,
    pub clusters: Vec<Cluster>,
    pub problems: Vec<Problem>,
}

impl Scan {
    fn failed(account: Account, failure: Failure) -> Self {
        Scan {
            account,
            clusters: Vec::new(),
            problems: vec![Problem {
                scope: String::new(),
                failure,
            }],
        }
    }
}

/// Every cluster of one account.
pub async fn scan(runner: &dyn CliRunner, account: &Account) -> Scan {
    match account.provider {
        Provider::Aws => scan_aws(runner, account).await,
        Provider::Azure => scan_azure(runner, account).await,
        Provider::Google => scan_google(runner, account).await,
    }
}

async fn scan_google(runner: &dyn CliRunner, account: &Account) -> Scan {
    let args = [
        "container",
        "clusters",
        "list",
        "--project",
        &account.id,
        "--format=json",
    ];
    let text = match output(runner, Provider::Google, &account.id, &args, &[]).await {
        Ok(text) => text,
        Err(failure) => return Scan::failed(account.clone(), failure),
    };
    let list = match json(&text) {
        Ok(list) => list,
        Err(failure) => return Scan::failed(account.clone(), failure),
    };
    let clusters = list
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|c| Cluster {
            provider: Provider::Google,
            account: account.id.clone(),
            name: string(c, "name"),
            region: {
                let location = string(c, "location");
                if location.is_empty() {
                    string(c, "zone")
                } else {
                    location
                }
            },
            group: None,
            version: string(c, "currentMasterVersion"),
            status: string(c, "status"),
        })
        .filter(|c| !c.name.is_empty())
        .collect();
    Scan {
        account: account.clone(),
        clusters,
        problems: Vec::new(),
    }
}

async fn scan_azure(runner: &dyn CliRunner, account: &Account) -> Scan {
    let args = [
        "aks",
        "list",
        "--subscription",
        &account.id,
        "--output",
        "json",
    ];
    let text = match output(runner, Provider::Azure, &account.id, &args, &[]).await {
        Ok(text) => text,
        Err(failure) => return Scan::failed(account.clone(), failure),
    };
    let list = match json(&text) {
        Ok(list) => list,
        Err(failure) => return Scan::failed(account.clone(), failure),
    };
    let clusters = list
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|c| {
            let power = c
                .pointer("/powerState/code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let provisioning = string(c, "provisioningState");
            Cluster {
                provider: Provider::Azure,
                account: account.id.clone(),
                name: string(c, "name"),
                region: string(c, "location"),
                group: Some(string(c, "resourceGroup")).filter(|g| !g.is_empty()),
                version: string(c, "kubernetesVersion"),
                // A stopped cluster says so; else how provisioning went.
                status: if power.is_empty() || power == "Running" {
                    provisioning
                } else {
                    power.to_string()
                },
            }
        })
        .filter(|c| !c.name.is_empty())
        .collect();
    Scan {
        account: account.clone(),
        clusters,
        problems: Vec::new(),
    }
}

/// The region the profile is configured with, if any.
async fn aws_default_region(runner: &dyn CliRunner, profile: &str) -> Option<String> {
    let text = output(
        runner,
        Provider::Aws,
        profile,
        &["configure", "get", "region", "--profile", profile],
        &[],
    )
    .await
    .ok()?;
    let region = text.trim().to_string();
    (!region.is_empty()).then_some(region)
}

async fn scan_aws(runner: &dyn CliRunner, account: &Account) -> Scan {
    let profile = account.id.as_str();
    let default_region = aws_default_region(runner, profile).await;
    // Signed in? (and the account id, to tell profiles of one account apart)
    let identity = match output(
        runner,
        Provider::Aws,
        profile,
        &[
            "sts",
            "get-caller-identity",
            "--profile",
            profile,
            "--output",
            "json",
        ],
        &[],
    )
    .await
    {
        Ok(text) => text,
        Err(failure) => return Scan::failed(account.clone(), failure),
    };
    let mut account = account.clone();
    if let Ok(identity) = json(&identity) {
        account.detail = string(&identity, "Account");
    }
    let mut problems = Vec::new();
    // Which regions: the enabled ones when the profile may ask, else its own.
    let first = default_region
        .clone()
        .unwrap_or_else(|| "us-east-1".to_string());
    let regions: Vec<String> = match output(
        runner,
        Provider::Aws,
        profile,
        &[
            "ec2",
            "describe-regions",
            "--profile",
            profile,
            "--region",
            &first,
            "--output",
            "json",
        ],
        &[],
    )
    .await
    {
        Ok(text) => json(&text)
            .ok()
            .and_then(|v| {
                v.get("Regions").and_then(Value::as_array).map(|regions| {
                    regions
                        .iter()
                        .filter(|r| {
                            r.get("OptInStatus").and_then(Value::as_str) != Some("not-opted-in")
                        })
                        .map(|r| string(r, "RegionName"))
                        .filter(|r| !r.is_empty())
                        .collect()
                })
            })
            .unwrap_or_default(),
        Err(_) => match &default_region {
            Some(region) => vec![region.clone()],
            None => {
                return Scan::failed(
                    account,
                    Failure::Other(format!(
                        "Profile {profile} has no default region and may not list regions (ec2:DescribeRegions): set one with `aws configure set region eu-west-1 --profile {profile}`."
                    )),
                );
            }
        },
    };
    let results: Vec<(String, Result<Vec<Cluster>, Failure>)> = futures::stream::iter(regions)
        .map(|region| {
            let account = account.clone();
            async move {
                let found = aws_region(runner, &account, &region).await;
                (region, found)
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut clusters = Vec::new();
    for (region, result) in results {
        match result {
            Ok(found) => clusters.extend(found),
            Err(failure) => problems.push(Problem {
                scope: region,
                failure,
            }),
        }
    }
    clusters.sort_by(|a, b| (&a.region, &a.name).cmp(&(&b.region, &b.name)));
    problems.sort_by(|a, b| a.scope.cmp(&b.scope));
    Scan {
        account,
        clusters,
        problems,
    }
}

/// The clusters of one region, described one by one (version and status).
async fn aws_region(
    runner: &dyn CliRunner,
    account: &Account,
    region: &str,
) -> Result<Vec<Cluster>, Failure> {
    let profile = account.id.as_str();
    let text = output(
        runner,
        Provider::Aws,
        profile,
        &[
            "eks",
            "list-clusters",
            "--region",
            region,
            "--profile",
            profile,
            "--output",
            "json",
        ],
        &[],
    )
    .await?;
    let names: Vec<String> = json(&text)?
        .get("clusters")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    futures::stream::iter(names)
        .map(|name| async move {
            let described = output(
                runner,
                Provider::Aws,
                profile,
                &[
                    "eks",
                    "describe-cluster",
                    "--name",
                    &name,
                    "--region",
                    region,
                    "--profile",
                    profile,
                    "--output",
                    "json",
                ],
                &[],
            )
            .await
            .and_then(|text| json(&text));
            // A cluster that can't be described is still listed, without version and status.
            let cluster = described
                .ok()
                .and_then(|v| v.get("cluster").cloned())
                .unwrap_or(Value::Null);
            Cluster {
                provider: Provider::Aws,
                account: profile.to_string(),
                name,
                region: region.to_string(),
                group: None,
                version: string(&cluster, "version"),
                status: string(&cluster, "status"),
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Ok)
        .collect()
}

/// Everything a cloud has: its accounts (or why they can't be listed) and each one's scan, in
/// the order they finish. `on_scan` is called as each account is done.
pub async fn discover(
    provider: Provider,
    runner: &dyn CliRunner,
    mut on_accounts: impl FnMut(&[Account]),
    mut on_scan: impl FnMut(Scan),
) -> Result<(), Failure> {
    let accounts = accounts(provider, runner).await?;
    on_accounts(&accounts);
    let mut scans = futures::stream::iter(accounts)
        .map(|account| async move { scan(runner, &account).await })
        .buffer_unordered(CONCURRENCY);
    while let Some(scan) = scans.next().await {
        on_scan(scan);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Adding clusters

/// The command that adds `cluster` to the kubeconfig `file` (the CLIs merge into an existing
/// file), like the CLI's own get-credentials. Returns (program, args, env).
pub fn credentials_command(
    cluster: &Cluster,
    file: &Path,
) -> (String, Vec<String>, Vec<(String, String)>) {
    let file = file.display().to_string();
    match cluster.provider {
        Provider::Aws => (
            "aws".into(),
            vec![
                "eks".into(),
                "update-kubeconfig".into(),
                "--name".into(),
                cluster.name.clone(),
                "--region".into(),
                cluster.region.clone(),
                "--kubeconfig".into(),
                file,
                "--profile".into(),
                cluster.account.clone(),
            ],
            Vec::new(),
        ),
        Provider::Google => (
            "gcloud".into(),
            vec![
                "container".into(),
                "clusters".into(),
                "get-credentials".into(),
                cluster.name.clone(),
                "--location".into(),
                cluster.region.clone(),
                "--project".into(),
                cluster.account.clone(),
            ],
            vec![("KUBECONFIG".into(), file)],
        ),
        Provider::Azure => (
            "az".into(),
            vec![
                "aks".into(),
                "get-credentials".into(),
                "--name".into(),
                cluster.name.clone(),
                "--resource-group".into(),
                cluster.group.clone().unwrap_or_default(),
                "--file".into(),
                file,
                "--subscription".into(),
                cluster.account.clone(),
            ],
            Vec::new(),
        ),
    }
}

/// What adding clusters ended in.
#[derive(Debug, PartialEq, Eq)]
pub struct Added {
    /// The kubeconfig the CLIs wrote (their exec plugin entries, no secrets of Kubyl's), when at
    /// least one cluster was added.
    pub kubeconfig: Option<String>,
    pub added: Vec<Cluster>,
    pub failed: Vec<(Cluster, Failure)>,
}

/// Runs each cluster's get-credentials into `file` (a path in a private folder), one after the
/// other: they all write the same file.
pub async fn add_clusters(runner: &dyn CliRunner, clusters: &[Cluster], file: &Path) -> Added {
    let mut added = Vec::new();
    let mut failed = Vec::new();
    for cluster in clusters {
        let (program, args, env) = credentials_command(cluster, file);
        let result = runner.run(&program, &args, &env).await;
        match result {
            Ok(out) if out.success => added.push(cluster.clone()),
            Ok(out) => failed.push((
                cluster.clone(),
                classify(cluster.provider, &cluster.account, &out.stderr),
            )),
            Err(error) => failed.push((cluster.clone(), run_failure(cluster.provider, error))),
        }
    }
    let kubeconfig = if added.is_empty() {
        None
    } else {
        std::fs::read_to_string(file).ok()
    };
    Added {
        kubeconfig,
        added,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_becomes_what_to_do() {
        let aws = |s: &str| classify(Provider::Aws, "prod", s);
        assert!(
            matches!(aws("Error loading SSO Token: Token for https://x/start does not exist"), Failure::SignIn { command, .. } if command == "aws sso login --profile prod")
        );
        assert!(
            matches!(aws("Unable to locate credentials. You can configure credentials by running \"aws configure\"."), Failure::SignIn { command, .. } if command == "aws configure --profile prod")
        );
        assert!(matches!(
            aws(
                "An error occurred (ExpiredToken) when calling the ListClusters operation: The security token included in the request is expired"
            ),
            Failure::SignIn { .. }
        ));
        assert_eq!(
            aws(
                "User: arn:aws:iam::1:user/a is not authorized to perform: eks:ListClusters on resource: x"
            ),
            Failure::Permission {
                permission: "eks:ListClusters".into()
            }
        );
        assert_eq!(
            aws("Could not connect to the endpoint URL"),
            Failure::Other("Could not connect to the endpoint URL".into())
        );

        let az = |s: &str| classify(Provider::Azure, "sub", s);
        assert!(
            matches!(az("AADSTS70043: The refresh token has expired due to inactivity."), Failure::SignIn { command, .. } if command == "az login")
        );
        assert!(matches!(
            az(
                "Interactive authentication is needed. Please run: az login --scope https://management.core.windows.net//.default"
            ),
            Failure::SignIn { .. }
        ));

        let gcloud = |s: &str| classify(Provider::Google, "proj", s);
        assert!(
            matches!(gcloud("Reauthentication required. Please run: gcloud auth login"), Failure::SignIn { command, .. } if command == "gcloud auth login")
        );
        assert_eq!(
            gcloud(
                "ERROR: (gcloud.container.clusters.list) ResponseError: code=403, message=Kubernetes Engine API has not been used in project proj before or it is disabled."
            ),
            Failure::ApiDisabled {
                project: "proj".into()
            }
        );
        assert_eq!(
            gcloud(
                "ERROR: (gcloud.container.clusters.list) ResponseError: code=403, message=Required \"container.clusters.list\" permission(s) for \"projects/proj\"."
            ),
            Failure::Permission {
                permission: "container.clusters.list".into()
            }
        );
    }

    #[test]
    fn messages_are_one_clean_line() {
        let message = classify(
            Provider::Aws,
            "p",
            "\u{1b}[31mSomething odd\u{1b}[0m happened\nTraceback (most recent call last):\n  File x",
        );
        assert_eq!(message, Failure::Other("Something odd happened".into()));
        let long = classify(Provider::Aws, "p", &"x".repeat(1000));
        let Failure::Other(text) = long else { panic!() };
        assert_eq!(text.len(), 300);
        assert_eq!(
            classify(Provider::Aws, "p", ""),
            Failure::Other("the command failed without a message".into())
        );
    }

    #[test]
    fn credentials_commands_match_the_single_cluster_import() {
        let file = Path::new("/t/config");
        let (program, args, env) = credentials_command(
            &Cluster {
                provider: Provider::Aws,
                account: "prod".into(),
                name: "a".into(),
                region: "eu-west-1".into(),
                group: None,
                version: String::new(),
                status: String::new(),
            },
            file,
        );
        assert_eq!(program, "aws");
        // The same arguments the "Import from a cloud CLI" dialog runs, plus the profile.
        assert_eq!(
            args.join(" "),
            "eks update-kubeconfig --name a --region eu-west-1 --kubeconfig /t/config --profile prod"
        );
        assert!(env.is_empty());
        let (_, args, env) = credentials_command(
            &Cluster {
                provider: Provider::Google,
                account: "p".into(),
                name: "a".into(),
                region: "europe-west4".into(),
                group: None,
                version: String::new(),
                status: String::new(),
            },
            file,
        );
        assert_eq!(
            args.join(" "),
            "container clusters get-credentials a --location europe-west4 --project p"
        );
        assert_eq!(env, [("KUBECONFIG".to_string(), "/t/config".to_string())]);
        let (_, args, _) = credentials_command(
            &Cluster {
                provider: Provider::Azure,
                account: "s".into(),
                name: "a".into(),
                region: "x".into(),
                group: Some("rg".into()),
                version: String::new(),
                status: String::new(),
            },
            file,
        );
        assert_eq!(
            args.join(" "),
            "aks get-credentials --name a --resource-group rg --file /t/config --subscription s"
        );
    }

    #[test]
    fn provider_text() {
        assert_eq!(
            Provider::Aws.sign_in(Some("p")),
            "aws sso login --profile p"
        );
        assert_eq!(Provider::Aws.sign_in(None), "aws sso login");
        assert_eq!(Provider::Azure.account_label(), "subscription");
        let c = Cluster {
            provider: Provider::Aws,
            account: "p".into(),
            name: "n".into(),
            region: "r".into(),
            group: None,
            version: String::new(),
            status: String::new(),
        };
        assert_ne!(
            c.key(),
            Cluster {
                region: "r2".into(),
                ..c.clone()
            }
            .key()
        );
    }
}
