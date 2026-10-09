//! The user's `helm` (decision 1): found through the login shell's `PATH` (or `helm.path`),
//! probed once (`helm version --short`, 3.13 or newer), and run on Tokio.
//!
//! Every command reaches the cluster with `--kubeconfig <file> --kube-context <context>`; for a
//! context that signs in through Kubyl the bearer token goes into `HELM_KUBETOKEN` (never an
//! argument, never a file). Values go to `helm` on stdin (`-f -`). Helm's output can hold
//! rendered Secrets and values: nothing here logs stdout, stdin or the environment, and stderr
//! is scrubbed of token shapes before it reaches a message.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kubyl_kube_core::cli::CliTarget;
pub use kubyl_kube_core::cli::{CliEnv, EnvPolicy};

use crate::decode::Driver;
use secrecy::{ExposeSecret as _, SecretString};
use tokio::io::{
    AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};
use tokio::sync::{mpsc, oneshot};

/// The oldest `helm` Kubyl runs: `--dry-run=server` arrived in 3.13.
pub const MIN_VERSION: Version = Version {
    major: 3,
    minor: 13,
    patch: 0,
};

/// A `helm` version (`v3.14.4+g81c902a` → 3.14.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// Parses `helm version --short` (`v3.14.4+g81c902a`, `v4.0.0`; Helm 2's
    /// `Client: v2.17.0+ga690bad`).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix("Client:").unwrap_or(text).trim();
        let text = text.trim_start_matches('v');
        let core = text.split(['+', '-', ' ']).next().unwrap_or_default();
        let mut parts = core.split('.').map(|p| p.parse::<u32>());
        Some(Self {
            major: parts.next()?.ok()?,
            minor: parts.next()?.ok()?,
            patch: parts.next().and_then(Result::ok).unwrap_or(0),
        })
    }

    /// Helm 4 renamed `--atomic` to `--rollback-on-failure`.
    pub fn is_v4(&self) -> bool {
        self.major >= 4
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// What `helm env` says about Helm's own files (the CLI and Kubyl share them, decision 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HelmEnv {
    /// `repositories.yaml`: the HTTP repositories (with their credentials, in plain text).
    pub repository_config: Option<PathBuf>,
    pub repository_cache: Option<PathBuf>,
    /// The OCI registry logins (Docker's credential store or `config.json`).
    pub registry_config: Option<PathBuf>,
}

impl HelmEnv {
    /// Parses `helm env` (`KEY="value"` lines; Helm 3 and 4 print the same).
    pub fn parse(text: &str) -> Self {
        let mut env = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            if value.is_empty() {
                continue;
            }
            match key.trim() {
                "HELM_REPOSITORY_CONFIG" => env.repository_config = Some(value.into()),
                "HELM_REPOSITORY_CACHE" => env.repository_cache = Some(value.into()),
                "HELM_REGISTRY_CONFIG" => env.registry_config = Some(value.into()),
                _ => {}
            }
        }
        env
    }
}

/// A usable `helm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmInfo {
    pub path: PathBuf,
    pub version: Version,
    /// As `helm version --short` printed it.
    pub version_text: String,
    pub env: HelmEnv,
    /// The `PATH` `helm` runs with (the login shell's, so exec plugins are found).
    pub search_path: Option<OsString>,
    /// What `helm` and its plugins inherit from this process (everything, by default).
    /// Shared: keeps [`Probe`]'s variants close in size.
    pub cli_env: Arc<CliEnv>,
}

/// What looking for `helm` found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Ready(HelmInfo),
    /// No `helm` in the `PATH` (or at `helm.path`).
    Missing {
        configured: Option<String>,
    },
    /// A `helm` older than [`MIN_VERSION`] (Helm 2 included).
    TooOld {
        path: PathBuf,
        version: String,
    },
    /// `helm version` failed.
    Broken {
        path: PathBuf,
        error: String,
    },
}

impl Probe {
    pub fn info(&self) -> Option<&HelmInfo> {
        match self {
            Probe::Ready(info) => Some(info),
            _ => None,
        }
    }

    /// Why Kubyl's Helm writes are off, for a note (`None` when `helm` is usable).
    pub fn problem(&self) -> Option<String> {
        match self {
            Probe::Ready(_) => None,
            Probe::Missing {
                configured: Some(path),
            } => Some(format!("helm isn't at {path} (helm.path in settings).")),
            Probe::Missing { configured: None } => {
                Some("helm isn't installed (or not in your login shell's PATH).".into())
            }
            Probe::TooOld { version, .. } => Some(format!(
                "helm {version} is too old: Kubyl needs {MIN_VERSION} or newer."
            )),
            Probe::Broken { path, error } => {
                Some(format!("{} doesn't run: {error}", path.display()))
            }
        }
    }
}

/// How to install `helm` on this platform.
pub fn install_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew install helm"
    } else if cfg!(windows) {
        "winget install Helm.Helm"
    } else {
        "curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash"
    }
}

/// Where `command` is: itself when it's a path that exists, else the first match in `path`
/// (on Windows also `.exe`, `.cmd` and `.bat`).
pub fn resolve(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if command.contains('/') || command.contains('\\') {
        let expanded = match command.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(rest))
                .unwrap_or_else(|| PathBuf::from(command)),
            None => PathBuf::from(command),
        };
        return expanded.is_file().then_some(expanded);
    }
    let path: OsString = path
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"))?;
    for dir in std::env::split_paths(&path) {
        if cfg!(windows) {
            for ext in ["exe", "cmd", "bat"] {
                let candidate = dir.join(format!("{command}.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        let candidate = dir.join(command);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

/// Finds and probes `helm` (on Tokio). `configured` is `helm.path`; `search_path` the login
/// shell's `PATH`.
pub async fn probe(configured: Option<&str>, search_path: Option<OsString>) -> Probe {
    probe_with(configured, search_path, CliEnv::default()).await
}

/// [`probe`] with an environment policy: `helm` runs with it, here and in [`run`] (through
/// [`HelmInfo::cli_env`]).
pub async fn probe_with(
    configured: Option<&str>,
    search_path: Option<OsString>,
    cli_env: CliEnv,
) -> Probe {
    let configured = configured.map(str::trim).filter(|c| !c.is_empty());
    let Some(path) = resolve(configured.unwrap_or("helm"), search_path.as_deref()) else {
        return Probe::Missing {
            configured: configured.map(str::to_string),
        };
    };
    let base = Invocation::read(["version", "--short"]).timeout(Duration::from_secs(20));
    let output = match run_raw(
        &path,
        None,
        None,
        search_path.as_deref(),
        &cli_env,
        base,
        None,
        None,
    )
    .await
    {
        Ok(output) => output,
        Err(err) => {
            // Helm 2 fails without Tiller ("could not find tiller"); still "too old".
            if err.detail.to_lowercase().contains("tiller") {
                return Probe::TooOld {
                    path,
                    version: "2".into(),
                };
            }
            return Probe::Broken {
                path,
                error: err.message,
            };
        }
    };
    let version_text = output.stdout.trim().to_string();
    let Some(version) = Version::parse(&version_text) else {
        return Probe::Broken {
            path,
            error: format!("unexpected version output: {version_text}"),
        };
    };
    if version < MIN_VERSION {
        return Probe::TooOld {
            path,
            version: version.to_string(),
        };
    }
    let env = run_raw(
        &path,
        None,
        None,
        search_path.as_deref(),
        &cli_env,
        Invocation::read(["env"]).timeout(Duration::from_secs(20)),
        None,
        None,
    )
    .await
    .map(|o| HelmEnv::parse(&o.stdout))
    .unwrap_or_default();
    Probe::Ready(HelmInfo {
        path,
        version,
        version_text,
        env,
        search_path,
        cli_env: Arc::new(cli_env),
    })
}

/// One `helm` command.
#[derive(Clone)]
pub struct Invocation {
    pub args: Vec<String>,
    /// Written to `helm`'s stdin (values for `-f -`, a password for `--password-stdin`).
    pub stdin: Option<SecretString>,
    pub timeout: Duration,
    /// Talks to the cluster: gets `--kubeconfig`, `--kube-context` and the token.
    pub cluster: bool,
    /// Where the release is stored (`HELM_DRIVER`): never what the user's shell says, so a
    /// release Kubyl installs is where Kubyl looks for it.
    pub driver: Driver,
}

impl fmt::Debug for Invocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invocation")
            .field("args", &self.args)
            .field("stdin", &self.stdin.as_ref().map(|_| "…"))
            .field("timeout", &self.timeout)
            .field("cluster", &self.cluster)
            .field("driver", &self.driver)
            .finish()
    }
}

impl Invocation {
    /// A command that doesn't reach the cluster (repositories, charts, `version`).
    pub fn read<S: Into<String>>(args: impl IntoIterator<Item = S>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            stdin: None,
            timeout: Duration::from_secs(120),
            cluster: false,
            driver: Driver::Secret,
        }
    }

    /// A command against the cluster.
    pub fn cluster<S: Into<String>>(args: impl IntoIterator<Item = S>) -> Self {
        Self {
            cluster: true,
            ..Self::read(args)
        }
    }

    /// An install, upgrade, rollback or uninstall that changes the cluster (not a dry run).
    pub fn is_write(&self) -> bool {
        self.cluster
            && self.args.first().is_some_and(|a| {
                matches!(a.as_str(), "install" | "upgrade" | "rollback" | "uninstall")
            })
            && !self.args.iter().any(|a| a.starts_with("--dry-run"))
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn stdin(mut self, text: Option<SecretString>) -> Self {
        self.stdin = text;
        self
    }

    /// The release's storage driver (the default is Helm's: Secrets).
    pub fn driver(mut self, driver: Driver) -> Self {
        self.driver = driver;
        self
    }
}

/// `HELM_DRIVER` for `driver`.
fn driver_env(driver: Driver) -> &'static str {
    match driver {
        Driver::Secret => "secret",
        Driver::ConfigMap => "configmap",
    }
}

/// Whether `key` is a variable of the user's environment that would change which cluster
/// `helm` reaches or how (`HELM_KUBE*`: token, API server, CA, TLS checks, impersonation),
/// where it stores releases (`HELM_DRIVER`) or the default namespace. Kubyl's own flags and
/// variables decide those.
pub fn is_scrubbed_env(key: &OsStr) -> bool {
    let key = key.to_string_lossy();
    key.starts_with("HELM_KUBE") || key == "HELM_NAMESPACE" || key == "HELM_DRIVER"
}

/// What a command printed.
pub struct Output {
    /// Can hold rendered Secrets and values: never log it.
    pub stdout: String,
    /// Scrubbed of token shapes.
    pub stderr: String,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output")
            .field("stdout", &format!("{} bytes", self.stdout.len()))
            .field("stderr", &self.stderr)
            .finish()
    }
}

/// What went wrong, as a user reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// RBAC refused something (`… is forbidden: User … cannot <verb> resource …`).
    Forbidden,
    /// `cannot re-use a name that is still in use`.
    NameInUse,
    /// `another operation (install/upgrade/rollback) is in progress`: a stuck `pending-*` release.
    InProgress,
    /// Kubyl's timeout or Helm's (`timed out waiting for the condition`, `context deadline exceeded`).
    Timeout,
    /// The release or chart doesn't exist.
    NotFound,
    /// The values don't match the chart's `values.schema.json`.
    Schema,
    /// The user cancelled.
    Cancelled,
    /// The context signs in through Kubyl and there's no sign-in to hand `helm`.
    SignInRequired,
    /// `helm` couldn't start.
    Spawn,
    Other,
}

/// A failed `helm` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmError {
    pub kind: ErrorKind,
    /// What to show: Helm's message mapped to what to do, scrubbed of token shapes.
    pub message: String,
    /// Helm's own last lines (scrubbed), for the details.
    pub detail: String,
}

impl fmt::Display for HelmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HelmError {}

impl HelmError {
    fn new(kind: ErrorKind, message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: detail.into(),
        }
    }
}

/// Removes token shapes from Helm's stderr (a safety net: Helm doesn't print tokens, but a
/// kubeconfig exec plugin or a repository URL might).
pub fn scrub(text: &str) -> String {
    let scrubbed = kubyl_resources_core::redact::scrub_text(text).into_owned();
    // `https://user:password@host` in repository URLs.
    let mut out = String::with_capacity(scrubbed.len());
    let mut rest = scrubbed.as_str();
    while let Some(ix) = rest.find("://") {
        let (head, tail) = rest.split_at(ix + 3);
        out.push_str(head);
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/' || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        let authority = &tail[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***@");
                out.push_str(&authority[at + 1..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Maps Helm's stderr to a message that says what to do. `stderr` is already scrubbed.
pub fn map_error(stderr: &str) -> HelmError {
    let text = stderr.trim();
    // Helm prints `Error: …` last; earlier lines are warnings.
    let last = text
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with("Error:"))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or_default()
        .trim()
        .trim_start_matches("Error:")
        .trim()
        .to_string();
    let lower = text.to_lowercase();
    let detail = text.to_string();
    if lower.contains("another operation (install/upgrade/rollback) is in progress") {
        return HelmError::new(
            ErrorKind::InProgress,
            "Another Helm operation on this release is in progress, or one was interrupted and left the release pending. Check the release's status: a stuck release needs a rollback to its last deployed revision (or an uninstall, if it never deployed).",
            detail,
        );
    }
    // Helm 3 says "re-use", Helm 4 "reuse".
    if lower.contains("cannot re-use a name that is still in use")
        || lower.contains("cannot reuse a name that is still in use")
    {
        return HelmError::new(
            ErrorKind::NameInUse,
            "A release with this name already exists in the namespace. Pick another name, or upgrade the existing release.",
            detail,
        );
    }
    if let Some(forbidden) = forbidden(text) {
        return HelmError::new(ErrorKind::Forbidden, forbidden, detail);
    }
    if lower.contains("values don't meet the specifications of the schema")
        || lower.contains("values.schema.json")
    {
        return HelmError::new(
            ErrorKind::Schema,
            format!("The values don't match the chart's schema: {last}"),
            detail,
        );
    }
    if lower.contains("timed out waiting for the condition")
        || lower.contains("context deadline exceeded")
        || lower.contains("timeout waiting for")
    {
        return HelmError::new(
            ErrorKind::Timeout,
            format!(
                "Helm timed out: {last}. The release may be left pending or failed: check its status and history."
            ),
            detail,
        );
    }
    if lower.contains("release: not found")
        || lower.contains("release not found")
        || lower.contains("not found in")
        || lower.contains("failed to fetch")
        || lower.contains("no chart version found")
        || lower.contains("no chart name found")
    {
        return HelmError::new(ErrorKind::NotFound, last.clone(), detail);
    }
    HelmError::new(
        ErrorKind::Other,
        if last.is_empty() {
            "helm failed without a message.".to_string()
        } else {
            last
        },
        detail,
    )
}

/// `secrets is forbidden: User "jane" cannot create resource "secrets" in API group "" in the
/// namespace "shop"` → which verb on which resource where.
fn forbidden(text: &str) -> Option<String> {
    let ix = text.find("is forbidden: ")?;
    let rest = &text[ix..];
    let cannot = rest.find("cannot ")?;
    let after = &rest[cannot + "cannot ".len()..];
    let verb = after.split_whitespace().next()?.to_string();
    let resource = after
        .split("resource \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap_or("objects")
        .to_string();
    let group = after
        .split("in API group \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .filter(|g| !g.is_empty())
        .map(|g| format!(".{g}"))
        .unwrap_or_default();
    let scope = after
        .split("in the namespace \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .map(|ns| format!("in {ns}"))
        .unwrap_or_else(|| "cluster-wide".into());
    Some(format!(
        "Forbidden: you can't {verb} {resource}{group} {scope}. Ask for a role that can {verb} {resource}{group}, or install into a namespace you manage."
    ))
}

/// Progress lines from `helm`'s stderr while it runs (scrubbed).
pub type Progress = mpsc::UnboundedSender<String>;

/// Runs `helm` with `invocation` against `target`. Dropping the future kills a read or a dry
/// run; a write keeps running (killing it midway would leave the release pending). `cancel`
/// interrupts `helm` the way Ctrl-C does, so Helm can record the release's state.
pub async fn run(
    helm: &HelmInfo,
    target: Option<&CliTarget>,
    invocation: Invocation,
    progress: Option<Progress>,
    cancel: Option<oneshot::Receiver<()>>,
) -> Result<Output, HelmError> {
    let token = match target {
        Some(target) if invocation.cluster => match target.token().await {
            Ok(token) => token,
            // Signed out, on the desktop's own credentials: `helm` tries the kubeconfig's
            // credentials (its author's) and reports what the server says.
            Err(_) if !target.sign_in_required() => None,
            // A scoped handle never acts as the kubeconfig's author.
            Err(_) => {
                return Err(HelmError::new(
                    ErrorKind::SignInRequired,
                    "Sign in to this cluster first: helm runs with Kubyl's sign-in.",
                    String::new(),
                ));
            }
        },
        _ => None,
    };
    run_with_token(helm, target, token, invocation, progress, cancel).await
}

/// [`run`] with the token already fetched (`None`: the kubeconfig's own credentials).
pub async fn run_with_token(
    helm: &HelmInfo,
    target: Option<&CliTarget>,
    token: Option<SecretString>,
    invocation: Invocation,
    progress: Option<Progress>,
    cancel: Option<oneshot::Receiver<()>>,
) -> Result<Output, HelmError> {
    // Without a target `helm` would use the kubeconfig and current context Kubyl inherited:
    // possibly another cluster than the one the user confirmed.
    if invocation.cluster && target.is_none() {
        return Err(HelmError::new(
            ErrorKind::Other,
            "Kubyl doesn't know this cluster's kubeconfig context anymore: reopen the dialog.",
            String::new(),
        ));
    }
    run_raw(
        &helm.path,
        target.filter(|_| invocation.cluster),
        token,
        helm.search_path.as_deref(),
        helm.cli_env.as_ref(),
        invocation,
        progress,
        cancel,
    )
    .await
}

/// The environment and arguments for `target` (what [`run`] adds).
pub fn cluster_args(target: &CliTarget) -> Vec<String> {
    vec![
        "--kubeconfig".into(),
        target.kubeconfig.to_string_lossy().into_owned(),
        "--kube-context".into(),
        target.context.clone(),
    ]
}

/// How long Helm gets to record the release's state after an interrupt before it's killed.
const GRACE: Duration = Duration::from_secs(15);

/// How long to wait for the rest of `helm`'s output once it exited or was stopped: an exec
/// plugin (a grandchild) may still hold the pipes open. Short in unit tests, which wait on it
/// under `TestHost`'s stall limit.
const DRAIN: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(3)
};

#[allow(clippy::too_many_arguments)]
async fn run_raw(
    program: &Path,
    target: Option<&CliTarget>,
    token: Option<SecretString>,
    search_path: Option<&OsStr>,
    cli_env: &CliEnv,
    invocation: Invocation,
    progress: Option<Progress>,
    cancel: Option<oneshot::Receiver<()>>,
) -> Result<Output, HelmError> {
    let write = invocation.is_write();
    let mut command = tokio::process::Command::new(program);
    cli_env.apply(command.as_std_mut());
    // Kubyl's own `--kube-*` flags and variables decide which cluster, how and where releases
    // are stored; the user's shell variables don't.
    for (key, _) in std::env::vars_os() {
        if is_scrubbed_env(&key) {
            command.env_remove(&key);
        }
    }
    command
        .args(&invocation.args)
        .stdin(if invocation.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        // A write killed midway leaves the release pending: when Kubyl quits during one, `helm`
        // finishes on its own (see `stderr_pipe`). Reads and dry runs stop with Kubyl.
        .kill_on_drop(!write)
        // Never Helm's colors or debug output (debug prints the rendered manifests).
        .env("HELM_DEBUG", "false")
        .env("NO_COLOR", "1")
        .env("HELM_DRIVER", driver_env(invocation.driver));
    if let Some(path) = search_path {
        command.env("PATH", path);
    }
    if let Some(target) = target {
        command.args(cluster_args(target));
        if let Some(token) = &token {
            command.env("HELM_KUBETOKEN", token.expose_secret());
            if let Some(server) = &target.server {
                command.env("HELM_KUBEAPISERVER", server);
            }
        }
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let spawn_error = |err: std::io::Error| {
        HelmError::new(
            ErrorKind::Spawn,
            format!("couldn't run {}: {err}", program.display()),
            String::new(),
        )
    };
    let stderr_pipe = stderr_pipe(&mut command, write).map_err(spawn_error)?;
    let mut child = command.spawn().map_err(spawn_error)?;
    // The command holds our copy of a write-end pipe: reading `helm`'s stderr only ends once
    // it's closed.
    drop(command);
    let stdin_task = match (invocation.stdin, child.stdin.take()) {
        // Helm reads stdin to the end before it starts; a large values file can't block us.
        (Some(stdin), Some(mut pipe)) => Some(tokio::spawn(async move {
            pipe.write_all(stdin.expose_secret().as_bytes()).await.ok();
            pipe.shutdown().await.ok();
        })),
        _ => None,
    };
    let stderr: Pin<Box<dyn AsyncRead + Send>> = match stderr_pipe {
        Some(pipe) => pipe,
        None => Box::pin(child.stderr.take().expect("piped")),
    };
    let mut stdout = child.stdout.take().expect("piped");
    // The readers fill shared buffers, so what arrived is kept when they're stopped early.
    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(String::new()));
    let read_out = tokio::spawn({
        let buf = out_buf.clone();
        async move {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = stdout.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                buf.lock()
                    .expect("stdout buffer")
                    .extend_from_slice(&chunk[..n]);
            }
        }
    });
    let read_err = tokio::spawn({
        let buf = err_buf.clone();
        async move {
            // The progress sender lives in this task: stopping it ends the progress stream.
            // Bytes, not `lines()`: a line that isn't UTF-8 (a plugin's output) must not end the
            // reading, or `helm` would block (writes) or die (reads) on a full or closed pipe.
            let mut reader = BufReader::new(stderr);
            let mut raw = Vec::new();
            loop {
                raw.clear();
                match reader.read_until(b'\n', &mut raw).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&raw);
                let line = scrub(text.trim_end_matches(['\n', '\r']));
                if let Some(progress) = &progress {
                    progress.send(line.clone()).ok();
                }
                let mut all = buf.lock().expect("stderr buffer");
                all.push_str(&line);
                all.push('\n');
            }
        }
    });
    let pid = child.id();
    let cancel = async move {
        match cancel {
            Some(rx) => {
                if rx.await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
            None => std::future::pending::<()>().await,
        }
    };
    let outcome = tokio::select! {
        status = child.wait() => Ok(status),
        _ = tokio::time::sleep(invocation.timeout) => Err(ErrorKind::Timeout),
        _ = cancel => Err(ErrorKind::Cancelled),
    };
    let status = match outcome {
        Ok(status) => status,
        Err(kind) => {
            // Give Helm time to record the release as failed, then stop it.
            let grace = if interrupt(pid) {
                GRACE
            } else {
                Duration::ZERO
            };
            if tokio::time::timeout(grace, child.wait()).await.is_err() {
                child.kill().await.ok();
            }
            tokio::join!(drain(read_out, DRAIN), drain(read_err, DRAIN));
            if let Some(task) = stdin_task {
                task.abort();
            }
            let stderr = std::mem::take(&mut *err_buf.lock().expect("stderr buffer"));
            let message = match kind {
                ErrorKind::Cancelled => {
                    "Cancelled. Helm may have left the release pending or failed: check its status."
                        .to_string()
                }
                _ => format!(
                    "helm didn't finish within {}s. The release may be left pending or failed: check its status.",
                    invocation.timeout.as_secs()
                ),
            };
            return Err(HelmError::new(kind, message, stderr));
        }
    };
    tokio::join!(drain(read_out, DRAIN), drain(read_err, DRAIN));
    if let Some(task) = stdin_task {
        task.abort();
    }
    let status = status.map_err(|err| {
        HelmError::new(
            ErrorKind::Spawn,
            format!("helm failed: {err}"),
            String::new(),
        )
    })?;
    let stdout = String::from_utf8_lossy(&std::mem::take(
        &mut *out_buf.lock().expect("stdout buffer"),
    ))
    .into_owned();
    let stderr = std::mem::take(&mut *err_buf.lock().expect("stderr buffer"));
    if status.success() {
        Ok(Output { stdout, stderr })
    } else {
        Err(map_error(&stderr))
    }
}

/// Waits up to `limit` for a reader to reach the end of its pipe, then stops it (a grandchild
/// that still holds the pipe would keep it, and the progress stream, open forever).
async fn drain(mut task: tokio::task::JoinHandle<()>, limit: Duration) {
    if tokio::time::timeout(limit, &mut task).await.is_err() {
        task.abort();
    }
}

/// Where `helm`'s stderr goes. Reads and dry runs: a plain pipe.
///
/// Writes must survive Kubyl quitting (a write killed midway leaves the release pending, and
/// `--atomic` never rolls back). Go programs die of SIGPIPE when they write to fd 1 or 2 after
/// the reader is gone, even when SIGPIPE is ignored, and Helm prints warnings to stderr while it
/// works. So on Unix `helm` inherits the pipe's read end as well: the pipe always has a reader,
/// and writes to it never fail (up to the pipe's buffer, which Helm's few lines don't fill).
/// Its stdout only gets the result after the release is recorded. Writes also run in their own
/// process group, so a Ctrl-C in the terminal that started Kubyl doesn't reach them.
#[cfg(unix)]
fn stderr_pipe(
    command: &mut tokio::process::Command,
    write: bool,
) -> std::io::Result<Option<Pin<Box<dyn AsyncRead + Send>>>> {
    use std::os::fd::{AsRawFd as _, OwnedFd};
    if !write {
        command.stderr(Stdio::piped());
        return Ok(None);
    }
    let (reader, writer) = std::io::pipe()?;
    let fd = reader.as_raw_fd();
    command.stderr(Stdio::from(writer)).process_group(0);
    // SAFETY: `fcntl` is async-signal-safe and only clears close-on-exec on a descriptor the
    // forked child holds (it was open in the parent when it forked).
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let receiver = tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(reader))?;
    Ok(Some(Box::pin(receiver)))
}

/// Elsewhere, writing to a closed pipe fails without killing `helm`: a plain pipe.
#[cfg(not(unix))]
fn stderr_pipe(
    command: &mut tokio::process::Command,
    _write: bool,
) -> std::io::Result<Option<Pin<Box<dyn AsyncRead + Send>>>> {
    command.stderr(Stdio::piped());
    Ok(None)
}

/// Ctrl-C for `helm`, so it can record the release's state. Unix only (SIGINT); returns whether
/// it was sent. Elsewhere `helm` is killed right away.
fn interrupt(pid: Option<u32>) -> bool {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        // SAFETY: plain signal to the child we spawned (still ours: not yet reaped).
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
        return true;
    }
    let _ = pid;
    false
}

/// What Kubyl knows about the user's `helm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliState {
    Probing,
    Done(Arc<Probe>),
}

/// The user's `helm`, probed on Tokio (again when `helm.path` changes or the user asks). A plain
/// service on a [`Host`](kubyl_base::Host); `kubyl_helm::cli::HelmCli` hosts it in GPUI.
pub struct HelmCliCore {
    state: CliState,
    /// `helm.path` from the settings.
    configured: Option<String>,
    /// What `helm` inherits from this process.
    cli_env: CliEnv,
    probing: Option<kubyl_base::TaskHandle>,
}

impl kubyl_base::Service for HelmCliCore {
    type Event = std::convert::Infallible;
    type Effect = std::convert::Infallible;
}

impl HelmCliCore {
    /// Not probed yet: call [`Self::reprobe`].
    pub fn new(configured: Option<String>) -> Self {
        Self {
            state: CliState::Probing,
            configured,
            cli_env: CliEnv::default(),
            probing: None,
        }
    }

    /// [`Self::new`] with an environment policy for `helm` and its plugins.
    pub fn with_cli_env(mut self, cli_env: CliEnv) -> Self {
        self.cli_env = cli_env;
        self
    }

    pub fn cli_env(&self) -> &CliEnv {
        &self.cli_env
    }

    /// Changes the environment policy and looks for `helm` again with it. A no-op when it's
    /// the one in use.
    pub fn set_cli_env(&mut self, cli_env: CliEnv, host: &mut dyn kubyl_base::Host<Self>) {
        if cli_env != self.cli_env {
            self.cli_env = cli_env;
            self.reprobe(host);
        }
    }

    pub fn state(&self) -> &CliState {
        &self.state
    }

    /// The probe's answer, once there is one.
    pub fn probe(&self) -> Option<&Probe> {
        match &self.state {
            CliState::Done(probe) => Some(probe),
            CliState::Probing => None,
        }
    }

    /// The usable `helm`, if any.
    pub fn info(&self) -> Option<&HelmInfo> {
        self.probe()?.info()
    }

    /// `helm.path` changed: look again when it's another one.
    pub fn set_configured(
        &mut self,
        configured: Option<String>,
        host: &mut dyn kubyl_base::Host<Self>,
    ) {
        if configured != self.configured {
            self.configured = configured;
            self.reprobe(host);
        }
    }

    /// Looks for `helm` again (after installing it, or changing `helm.path`) in the login
    /// shell's `PATH`.
    pub fn reprobe(&mut self, host: &mut dyn kubyl_base::Host<Self>) {
        self.probe_in(None, host);
    }

    /// [`Self::reprobe`] with `search_path` instead of the login shell's `PATH` (tests).
    pub fn probe_in(
        &mut self,
        search_path: Option<OsString>,
        host: &mut dyn kubyl_base::Host<Self>,
    ) {
        use kubyl_base::HostExt as _;
        self.state = CliState::Probing;
        let configured = self.configured.clone();
        let cli_env = self.cli_env.clone();
        self.probing = Some(host.spawn(
            async move {
                let path = match search_path {
                    Some(path) => Some(path),
                    None => tokio::task::spawn_blocking(kubyl_kube_core::auth::shell_env::path)
                        .await
                        .ok()
                        .flatten(),
                };
                probe_with(configured.as_deref(), path, cli_env).await
            },
            |this, probe, host| {
                if let Probe::Ready(info) = &probe {
                    tracing::info!(helm = %info.version, path = %info.path.display(), "found helm");
                }
                this.state = CliState::Done(Arc::new(probe));
                host.notify();
            },
        ));
        host.notify();
    }

    /// Sets the probe's answer (tests, screenshots of the missing state).
    pub fn set_probe(&mut self, probe: Probe, host: &mut dyn kubyl_base::Host<Self>) {
        self.probing = None;
        self.state = CliState::Done(Arc::new(probe));
        host.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        let v = Version::parse("v3.14.4+g81c902a").unwrap();
        assert_eq!(v.to_string(), "3.14.4");
        assert!(v >= MIN_VERSION);
        assert!(Version::parse("v3.12.3+g3a31588").unwrap() < MIN_VERSION);
        assert!(Version::parse("v4.3.0+gbec5b06").unwrap().is_v4());
        assert_eq!(Version::parse("v4.0.0-rc.1").unwrap().major, 4);
        assert!(Version::parse("garbage").is_none());
    }

    #[test]
    fn helm_env_is_read() {
        let env = HelmEnv::parse(
            "HELM_BIN=\"helm\"\nHELM_REGISTRY_CONFIG=\"/h/registry/config.json\"\nHELM_REPOSITORY_CACHE=\"/h/cache\"\nHELM_REPOSITORY_CONFIG=\"/h/repositories.yaml\"\nHELM_KUBETOKEN=\"\"\n",
        );
        assert_eq!(
            env.repository_config.as_deref(),
            Some(Path::new("/h/repositories.yaml"))
        );
        assert_eq!(env.repository_cache.as_deref(), Some(Path::new("/h/cache")));
        assert!(env.registry_config.is_some());
    }

    #[test]
    fn errors_say_what_to_do() {
        let e =
            map_error("Error: INSTALLATION FAILED: cannot re-use a name that is still in use\n");
        assert_eq!(e.kind, ErrorKind::NameInUse);
        let e = map_error(
            "Error: INSTALLATION FAILED: release name check failed: cannot reuse a name that is still in use\n",
        );
        assert_eq!(e.kind, ErrorKind::NameInUse);
        let e = map_error(
            "Error: UPGRADE FAILED: another operation (install/upgrade/rollback) is in progress\n",
        );
        assert_eq!(e.kind, ErrorKind::InProgress);
        assert!(e.message.contains("roll"));
        let e = map_error(
            "Error: INSTALLATION FAILED: secrets is forbidden: User \"jane\" cannot create resource \"secrets\" in API group \"\" in the namespace \"shop\"\n",
        );
        assert_eq!(e.kind, ErrorKind::Forbidden);
        assert_eq!(
            e.message,
            "Forbidden: you can't create secrets in shop. Ask for a role that can create secrets, or install into a namespace you manage."
        );
        let e = map_error(
            "Error: query: failed to query with labels: deployments.apps is forbidden: User \"jane\" cannot list resource \"deployments\" in API group \"apps\" at the cluster scope\n",
        );
        assert!(
            e.message.contains("list deployments.apps cluster-wide"),
            "{}",
            e.message
        );
        let e = map_error("Error: UPGRADE FAILED: timed out waiting for the condition\n");
        assert_eq!(e.kind, ErrorKind::Timeout);
        let e = map_error(
            "walk.go:75: found symbolic link\nError: values don't meet the specifications of the schema(s) in the following chart(s):\ndemo:\n- replicaCount: Invalid type. Expected: integer, given: string\n",
        );
        assert_eq!(e.kind, ErrorKind::Schema);
        let e = map_error("Error: release: not found\n");
        assert_eq!(e.kind, ErrorKind::NotFound);
        assert_eq!(map_error("").message, "helm failed without a message.");
    }

    #[test]
    fn stderr_is_scrubbed() {
        let text = scrub(
            "Error: looks like \"https://alice:s3cret@charts.example.com/stable\" is not a valid chart repository\nAuthorization: Bearer eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJqYW5lIn0.c2lnbmF0dXJl",
        );
        assert!(!text.contains("s3cret"), "{text}");
        assert!(!text.contains("eyJzdWIiOiJqYW5lIn0"), "{text}");
        assert!(text.contains("***@charts.example.com/stable"));
    }

    #[test]
    fn programs_resolve_from_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("helm");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(resolve("helm", Some(&path)), Some(program.clone()));
        assert_eq!(
            resolve(program.to_str().unwrap(), None),
            Some(program.clone())
        );
        assert_eq!(resolve("nope", Some(&path)), None);
    }
}
