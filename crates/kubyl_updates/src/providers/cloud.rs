//! Shared by the cloud providers: their CLIs for credentials, HTTP, error mapping, and the
//! kubeconfig's exec plugin (where the cluster, region and profile come from).
//!
//! CLIs (`aws`, `gcloud`, `az`) run on Tokio like exec plugins do (`kubyl_kube::auth::exec`):
//! the login shell's `PATH`, no stdin (never interactive), a timeout and no console window on
//! Windows. Their stdout holds credentials and never goes into errors or logs; stderr does (it
//! says why a login expired). HTTP goes through the `reqwest` that `openidconnect` brings
//! (rustls). Only a request's method and path are logged, at debug level. Nothing here retries.

use std::ffi::OsString;
use std::fmt;
use std::future::Future;
use std::process::Stdio;
use std::time::Duration;

use http::{HeaderMap, HeaderValue, Method};
use jiff::Timestamp;
use kube::config::Kubeconfig;
use openidconnect::reqwest;
use secrecy::{ExposeSecret as _, SecretString};
use serde::de::DeserializeOwned;
use url::Url;

use super::CloudContext;
use crate::provider::ProviderError;

// ---------------------------------------------------------------------------------------------
// CLIs

/// How long a CLI may take (a token refresh, an SSO role's credentials).
pub const CLI_TIMEOUT: Duration = Duration::from_secs(20);

/// A cloud CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cli {
    pub program: &'static str,
    pub name: &'static str,
    pub install_url: &'static str,
}

pub const AWS: Cli = Cli {
    program: "aws",
    name: "AWS CLI",
    install_url: "https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html",
};

pub const GCLOUD: Cli = Cli {
    program: "gcloud",
    name: "Google Cloud CLI",
    install_url: "https://cloud.google.com/sdk/docs/install",
};

pub const AZ: Cli = Cli {
    program: "az",
    name: "Azure CLI",
    install_url: "https://learn.microsoft.com/cli/azure/install-azure-cli",
};

/// Environment variables for a CLI, from the kubeconfig's exec plugin. Values may be secrets
/// (static keys in `env:`), so `Debug` shows only the names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CliEnv(Vec<(String, String)>);

impl CliEnv {
    pub fn new(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(vars.into_iter().collect())
    }

    /// The value of a variable (the last one wins, like a shell).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The variables whose names pass `keep`.
    pub fn filter(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self(self.0.iter().filter(|(n, _)| keep(n)).cloned().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(n, _)| n.as_str())
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
}

impl fmt::Debug for CliEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

/// Why a CLI run failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// Not installed (not in the login shell's `PATH`).
    NotFound,
    TimedOut,
    /// It ran and failed. Only stderr is kept: stdout may hold a credential.
    Failed {
        code: Option<i32>,
        stderr: String,
    },
    Io(String),
}

impl CliError {
    /// The provider error for a failure the provider didn't recognize: `command` is what ran
    /// (shown), `fix` what the user can run to fix it.
    pub fn to_provider_error(&self, cli: Cli, command: &str, fix: Option<String>) -> ProviderError {
        match self {
            CliError::NotFound => not_installed(cli),
            CliError::TimedOut => ProviderError::Unavailable(format!(
                "`{command}` didn't finish within {} s.",
                CLI_TIMEOUT.as_secs()
            )),
            CliError::Failed { code, stderr } => {
                let summary = stderr_summary(stderr);
                ProviderError::Credentials {
                    message: if summary.is_empty() {
                        format!(
                            "`{command}` failed ({}).",
                            code.map(|c| format!("exit code {c}"))
                                .unwrap_or_else(|| "killed by a signal".into())
                        )
                    } else {
                        format!("`{command}` failed: {summary}")
                    },
                    command: fix,
                }
            }
            CliError::Io(err) => {
                ProviderError::Unavailable(format!("Couldn't run {}: {err}", cli.program))
            }
        }
    }
}

/// A CLI that isn't installed: which one, and where to get it.
pub fn not_installed(cli: Cli) -> ProviderError {
    ProviderError::Unavailable(format!(
        "The {} (`{}`) isn't installed or not in your PATH. Install it from {}, sign in, and \
         re-check.",
        cli.name, cli.program, cli.install_url
    ))
}

/// A CLI's stderr in one line, without its prefixes (`aws: [ERROR]:`,
/// `ERROR: (gcloud.auth.print-access-token)`), at most 400 characters.
pub fn stderr_summary(stderr: &str) -> String {
    let mut text = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
    for prefix in ["aws: [ERROR]:", "ERROR:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim_start().to_string();
        }
    }
    if text.starts_with("(gcloud.")
        && let Some(end) = text.find(") ")
    {
        text = text[end + 2..].to_string();
    }
    if text.chars().count() > 400 {
        text = text.chars().take(399).collect::<String>() + "…";
    }
    text
}

/// Runs `cli args…` and returns its stdout.
pub async fn run(cli: Cli, args: &[&str], env: &CliEnv) -> Result<Vec<u8>, CliError> {
    run_program(cli.program, args, env, CLI_TIMEOUT).await
}

async fn run_program(
    program: &str,
    args: &[&str],
    env: &CliEnv,
    timeout: Duration,
) -> Result<Vec<u8>, CliError> {
    // The first call asks the login shell (usually prefetched at startup): not on a runtime
    // thread.
    let path = tokio::task::spawn_blocking(kubyl_kube::auth::shell_env::path)
        .await
        .ok()
        .flatten();
    let mut cmd = command(program, path.as_ref());
    cmd.args(args);
    if let Some(path) = &path {
        cmd.env("PATH", path);
    }
    for (name, value) in env.iter() {
        cmd.env(name, value);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Arguments name profiles and clusters, never secrets; still, only the subcommand is logged.
    tracing::debug!(
        program,
        subcommand = args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
        "running a cloud CLI"
    );
    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return Err(CliError::TimedOut),
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CliError::NotFound);
        }
        Ok(Err(err)) => return Err(CliError::Io(err.to_string())),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        return Err(CliError::Failed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

/// The command to run, resolved against the login shell's `PATH` like exec plugins are. On
/// Windows, `.cmd`/`.bat` shims (`gcloud`, `az`) run through `cmd /C`.
fn command(program: &str, path: Option<&OsString>) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let has_extension = std::path::Path::new(program).extension().is_some();
        if !has_extension && let Some(path) = path {
            for dir in std::env::split_paths(path) {
                let exe = dir.join(format!("{program}.exe"));
                if exe.is_file() {
                    return tokio::process::Command::new(exe);
                }
                for ext in ["cmd", "bat"] {
                    let shim = dir.join(format!("{program}.{ext}"));
                    if shim.is_file() {
                        let mut cmd = tokio::process::Command::new("cmd");
                        cmd.arg("/C").arg(shim);
                        return cmd;
                    }
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        if !program.contains('/')
            && let Some(path) = path
        {
            for dir in std::env::split_paths(path) {
                let candidate = dir.join(program);
                if candidate.is_file() {
                    return tokio::process::Command::new(candidate);
                }
            }
        }
    }
    tokio::process::Command::new(program)
}

// ---------------------------------------------------------------------------------------------
// The kubeconfig's exec plugin

/// The exec credential plugin of a context's user: where the providers find the cluster,
/// region and profile. `env` may hold secrets (see [`CliEnv`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecInfo {
    pub command: String,
    pub args: Vec<String>,
    pub env: CliEnv,
}

impl ExecInfo {
    /// The command's file name without directory and extension (`aws`, `kubelogin`).
    pub fn program(&self) -> String {
        let base = self
            .command
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&self.command);
        let base = [".exe", ".cmd", ".bat"]
            .iter()
            .find_map(|ext| base.strip_suffix(ext))
            .unwrap_or(base);
        base.to_lowercase()
    }

    /// The value of the first of `names` given as `--name value` or `--name=value`.
    pub fn flag(&self, names: &[&str]) -> Option<String> {
        for name in names {
            let mut args = self.args.iter();
            while let Some(arg) = args.next() {
                if arg == name {
                    if let Some(value) = args.next().filter(|v| !v.starts_with('-')) {
                        return Some(value.clone());
                    }
                } else if let Some(value) = arg
                    .strip_prefix(name)
                    .and_then(|rest| rest.strip_prefix('='))
                {
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    /// Whether a boolean flag is set (`--flag` or `--flag=true`).
    pub fn has_flag(&self, name: &str) -> bool {
        self.args.iter().any(|arg| {
            arg == name
                || arg
                    .strip_prefix(name)
                    .and_then(|rest| rest.strip_prefix('='))
                    .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        })
    }
}

/// The exec plugin of `context`'s user (or of `user`, when given) in a kubeconfig.
pub fn exec_info(config: &Kubeconfig, context: &str, user: Option<&str>) -> Option<ExecInfo> {
    let user = match user {
        Some(user) => user.to_string(),
        None => config
            .contexts
            .iter()
            .find(|c| c.name == context)?
            .context
            .as_ref()?
            .user
            .clone()?,
    };
    let exec = config
        .auth_infos
        .iter()
        .find(|u| u.name == user)?
        .auth_info
        .as_ref()?
        .exec
        .as_ref()?;
    let env = exec
        .env
        .iter()
        .flatten()
        .filter_map(|var| Some((var.get("name")?.clone(), var.get("value")?.clone())));
    Some(ExecInfo {
        command: exec.command.clone()?,
        args: exec.args.clone().unwrap_or_default(),
        env: CliEnv::new(env),
    })
}

/// Reads the context's exec plugin from its kubeconfig file (on a blocking thread). `None` when
/// the file can't be read or the user has no exec plugin.
pub async fn load_exec_info(ctx: &CloudContext) -> Option<ExecInfo> {
    let path = ctx.kubeconfig.clone();
    let context = ctx.context.clone();
    let user = ctx.user.clone();
    tokio::task::spawn_blocking(move || {
        let config = Kubeconfig::read_from(&path)
            .inspect_err(|err| tracing::debug!("couldn't read the kubeconfig: {err}"))
            .ok()?;
        exec_info(&config, &context, user.as_deref())
    })
    .await
    .ok()
    .flatten()
}

/// The host of a server URL (`https://abc.gr7.eu-west-1.eks.amazonaws.com:443` →
/// `abc.gr7.eu-west-1.eks.amazonaws.com`), lowercase.
pub fn server_host(server: &str) -> Option<String> {
    Url::parse(server)
        .ok()?
        .host_str()
        .map(|h| h.trim_end_matches('.').to_lowercase())
}

// ---------------------------------------------------------------------------------------------
// Credentials in memory

/// Refresh credentials this long before they expire.
pub const EXPIRY_MARGIN: Duration = Duration::from_secs(5 * 60);

/// Until when a credential is used: [`EXPIRY_MARGIN`] before it expires, or `fallback` from
/// `now` when it doesn't say.
pub fn fresh_until(expires: Option<Timestamp>, fallback: Duration, now: Timestamp) -> Timestamp {
    match expires {
        Some(at) => at - EXPIRY_MARGIN,
        None => now + fallback,
    }
}

/// One credential, fetched once and reused until it's due. Held in memory only.
pub struct Cache<T> {
    slot: tokio::sync::Mutex<Option<(T, Timestamp)>>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            slot: tokio::sync::Mutex::new(None),
        }
    }
}

impl<T: Clone> Cache<T> {
    /// The cached value, or a fresh one from `fetch` (which says until when it's good).
    /// Concurrent callers wait for one fetch.
    pub async fn get<F, Fut>(&self, fetch: F) -> Result<T, ProviderError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(T, Timestamp), ProviderError>>,
    {
        let mut slot = self.slot.lock().await;
        if let Some((value, until)) = slot.as_ref()
            && Timestamp::now() < *until
        {
            return Ok(value.clone());
        }
        let (value, until) = fetch().await?;
        *slot = Some((value.clone(), until));
        Ok(value)
    }

    /// Drops the value (the API rejected it).
    pub async fn clear(&self) {
        self.slot.lock().await.take();
    }
}

/// A bearer token (GKE, AKS).
#[derive(Clone)]
pub struct BearerToken(pub SecretString);

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerToken([REDACTED])")
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP

/// A request's timeout (each read or write is one request).
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// An HTTPS client for the cloud APIs.
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new() -> Result<Self, ProviderError> {
        reqwest::ClientBuilder::new()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            // Never follow a redirect with credentials attached.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("kubyl/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|err| {
                ProviderError::Unavailable(format!("Couldn't set up an HTTPS client: {err}"))
            })
    }

    /// Sends one request (never retried here). Network failures are `Unavailable`; HTTP errors
    /// come back as a [`Response`] for the provider to read.
    pub async fn send(&self, request: Request) -> Result<Response, ProviderError> {
        tracing::debug!(method = %request.method, path = request.url.path(), "cloud API request");
        let host = request.url.host_str().unwrap_or_default().to_string();
        let mut builder = self
            .client
            .request(request.method, request.url)
            .headers(request.headers);
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let unreachable = |err: reqwest::Error| {
            let what = if err.is_timeout() {
                format!("timed out after {} s", HTTP_TIMEOUT.as_secs())
            } else {
                err.without_url().to_string()
            };
            ProviderError::Unavailable(format!("Couldn't reach {host}: {what}"))
        };
        let response = builder.send().await.map_err(unreachable)?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(unreachable)?.to_vec();
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// A request. `Debug` shows only the method and path (headers hold credentials).
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Vec<u8>>,
}

impl Request {
    pub fn new(method: Method, url: Url) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::ACCEPT,
            HeaderValue::from_static("application/json"),
        );
        Self {
            method,
            url,
            headers,
            body: None,
        }
    }

    /// A JSON body.
    pub fn json(mut self, body: &serde_json::Value) -> Self {
        self.headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        self.body = Some(serde_json::to_vec(body).unwrap_or_default());
        self
    }

    /// `Authorization: Bearer …`, marked sensitive.
    pub fn bearer(mut self, token: &BearerToken) -> Self {
        if let Ok(mut value) = HeaderValue::from_str(&format!("Bearer {}", token.0.expose_secret()))
        {
            value.set_sensitive(true);
            self.headers.insert(http::header::AUTHORIZATION, value);
        }
        self
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        if let Ok(value) = HeaderValue::from_str(value) {
            self.headers.insert(name, value);
        }
        self
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Request({} {})", self.method, self.url.path())
    }
}

/// A response.
#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as `T`. Errors say where the JSON broke, never what it held.
    pub fn json<T: DeserializeOwned>(&self, what: &str) -> Result<T, ProviderError> {
        parse_json(&self.body, what)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// Parses JSON; errors say where it broke, never what it held.
pub fn parse_json<T: DeserializeOwned>(body: &[u8], what: &str) -> Result<T, ProviderError> {
    serde_json::from_slice(body).map_err(|err| {
        ProviderError::Other(format!(
            "Couldn't read {what} (line {}, column {}).",
            err.line(),
            err.column()
        ))
    })
}

/// What a request needs, for errors: the IAM permission and the resource
/// (`eks:DescribeCluster` on `cluster prod-eu-west-1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub permission: String,
    pub resource: String,
}

impl Action {
    pub fn new(permission: impl Into<String>, resource: impl Into<String>) -> Self {
        Self {
            permission: permission.into(),
            resource: resource.into(),
        }
    }
}

/// A cloud API's error, as each provider reads its error bodies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiError {
    /// `AccessDeniedException`, `PERMISSION_DENIED`, `AuthorizationFailed`.
    pub code: Option<String>,
    pub message: String,
    /// The permission the error names, when it does.
    pub permission: Option<String>,
    /// The credentials were rejected (expired, invalid), not a permission.
    pub credentials: bool,
}

/// The provider error for a failed request: 401 (or `credentials`) → `Credentials` with the
/// `login` command, 403 → `Forbidden` naming the permission, 404 → `NotFound`, 429/5xx →
/// `Unavailable`, anything else the server's message.
pub fn error_for(
    status: u16,
    error: ApiError,
    action: &Action,
    cloud: &str,
    login: Option<String>,
) -> ProviderError {
    let message = if error.message.is_empty() {
        match &error.code {
            Some(code) => format!("{code} (HTTP {status})"),
            None => format!("HTTP {status}"),
        }
    } else {
        error.message.clone()
    };
    if error.credentials || status == 401 {
        return ProviderError::Credentials {
            message: format!("{cloud} didn't accept your credentials: {message}"),
            command: login,
        };
    }
    match status {
        403 => ProviderError::Forbidden {
            verb: error
                .permission
                .unwrap_or_else(|| action.permission.clone()),
            resource: action.resource.clone(),
        },
        404 => ProviderError::NotFound(format!("{} ({message})", action.resource)),
        408 | 429 | 500..=599 => {
            ProviderError::Unavailable(format!("{cloud} API: {message} (HTTP {status})"))
        }
        _ => ProviderError::Other(format!("{cloud} API: {message}")),
    }
}

/// Percent-encodes everything but RFC 3986's unreserved characters (`A-Z a-z 0-9 - _ . ~`), as
/// SigV4 and URL paths built from names need.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A URL of `base` with `path` (already encoded) and `query` pairs (encoded here).
pub fn url(base: &Url, path: &str, query: &[(&str, &str)]) -> Result<Url, ProviderError> {
    let mut text = base.as_str().trim_end_matches('/').to_string();
    text.push_str(path);
    if !query.is_empty() {
        text.push('?');
        let pairs: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect();
        text.push_str(&pairs.join("&"));
    }
    Url::parse(&text).map_err(|err| ProviderError::Other(format!("Invalid API URL: {err}")))
}

/// A number or a string JSON timestamp: epoch seconds (AWS), RFC 3339, or a date.
pub fn timestamp(value: &serde_json::Value) -> Option<Timestamp> {
    match value {
        serde_json::Value::Number(n) => {
            let seconds = n.as_f64()?;
            Timestamp::from_millisecond((seconds * 1000.0).round() as i64).ok()
        }
        serde_json::Value::String(s) => s.parse::<Timestamp>().ok().or_else(|| {
            s.parse::<jiff::civil::Date>()
                .ok()?
                .to_zoned(jiff::tz::TimeZone::UTC)
                .ok()
                .map(|z| z.timestamp())
        }),
        _ => None,
    }
}

/// `2026-07-23`.
pub fn date(at: Timestamp) -> String {
    at.strftime("%Y-%m-%d").to_string()
}

#[cfg(test)]
pub(crate) mod mock {
    //! A tiny HTTP/1.1 server on loopback that answers from recorded responses, to run a
    //! provider's request path end to end without the network.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    /// One recorded request.
    #[derive(Clone, Debug)]
    pub struct Seen {
        pub method: String,
        /// Path and query.
        pub target: String,
        /// Lowercase names.
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Seen {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }
    }

    /// A route: method, path (with the query when it must match exactly), status, body.
    pub type Route = (&'static str, String, u16, String);

    /// Serves `routes` until the test ends. Returns the base URL and the requests seen.
    pub async fn serve(routes: Vec<Route>) -> (url::Url, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let routes = Arc::new(routes);
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let mut lines = head.lines();
                    let mut first = lines.next().unwrap_or_default().split(' ');
                    let method = first.next().unwrap_or_default().to_string();
                    let target = first.next().unwrap_or_default().to_string();
                    let headers: Vec<(String, String)> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(n, v)| (n.trim().to_lowercase(), v.trim().to_string()))
                        .collect();
                    let length = headers
                        .iter()
                        .find(|(n, _)| n == "content-length")
                        .and_then(|(_, v)| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    let mut body = buf[head_end..].to_vec();
                    while body.len() < length {
                        let n = socket.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&chunk[..n]);
                    }
                    let path = target.split('?').next().unwrap_or_default();
                    let route = routes
                        .iter()
                        .find(|(m, p, ..)| *m == method && *p == target)
                        .or_else(|| routes.iter().find(|(m, p, ..)| *m == method && p == path));
                    let (status, text) = match route {
                        Some((_, _, status, text)) => (*status, text.clone()),
                        None => (404, r#"{"message":"no such route"}"#.to_string()),
                    };
                    log.lock().unwrap().push(Seen {
                        method,
                        target,
                        headers,
                        body,
                    });
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    socket.write_all(response.as_bytes()).await.ok();
                    socket.shutdown().await.ok();
                });
            }
        });
        (base, seen)
    }

    /// A fixture of `tests/fixtures/<dir>/<name>`.
    pub fn fixture(dir: &str, name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(dir)
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KUBECONFIG: &str = r#"
apiVersion: v1
kind: Config
clusters:
- name: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
  cluster: {server: "https://ABCDEF0123456789.gr7.eu-west-1.eks.amazonaws.com"}
contexts:
- name: prod
  context:
    cluster: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
    user: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
users:
- name: arn:aws:eks:eu-west-1:111122223333:cluster/prod-eu-west-1
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1beta1
      command: aws
      args: ["--region", "eu-west-1", "eks", "get-token", "--cluster-name=prod-eu-west-1"]
      env:
      - name: AWS_PROFILE
        value: prod
      - name: AWS_SECRET_ACCESS_KEY
        value: sekrit-value
"#;

    #[test]
    fn reads_the_exec_plugin_of_a_context() {
        let config = Kubeconfig::from_yaml(KUBECONFIG).unwrap();
        let exec = exec_info(&config, "prod", None).unwrap();
        assert_eq!(exec.program(), "aws");
        assert_eq!(exec.flag(&["--region"]).as_deref(), Some("eu-west-1"));
        assert_eq!(
            exec.flag(&["--cluster-name"]).as_deref(),
            Some("prod-eu-west-1")
        );
        assert_eq!(exec.flag(&["--profile"]), None);
        assert_eq!(exec.env.get("AWS_PROFILE"), Some("prod"));
        // Env values never show in Debug.
        let debug = format!("{exec:?}");
        assert!(!debug.contains("sekrit-value"), "{debug}");
        assert!(debug.contains("AWS_SECRET_ACCESS_KEY"));
        assert!(exec_info(&config, "missing", None).is_none());
        assert!(exec_info(&config, "prod", Some("nobody")).is_none());
    }

    #[test]
    fn program_names_and_flags() {
        let exec = ExecInfo {
            command: r"C:\Program Files\Azure\kubelogin.EXE".into(),
            args: vec!["get-token".into(), "--verbose".into(), "--x".into()],
            env: CliEnv::default(),
        };
        // Windows paths and extensions (case of the extension is kept by the OS; the lookup
        // lowercases).
        assert_eq!(
            ExecInfo {
                command: r"C:\bin\aws.exe".into(),
                ..exec.clone()
            }
            .program(),
            "aws"
        );
        assert_eq!(
            ExecInfo {
                command: "/opt/homebrew/bin/gke-gcloud-auth-plugin".into(),
                ..exec.clone()
            }
            .program(),
            "gke-gcloud-auth-plugin"
        );
        // A flag followed by another flag has no value.
        assert_eq!(exec.flag(&["--verbose"]), None);
        assert!(exec.has_flag("--verbose"));
        assert!(!exec.has_flag("--quiet"));
    }

    #[test]
    fn stderr_summaries() {
        assert_eq!(
            stderr_summary(
                "\naws: [ERROR]: Error loading SSO Token: Token for corp does not exist\n"
            ),
            "Error loading SSO Token: Token for corp does not exist"
        );
        assert_eq!(
            stderr_summary(
                "ERROR: (gcloud.auth.print-access-token) You do not currently have an active \
                 account selected.\nPlease run:\n\n  $ gcloud auth login\n"
            ),
            "You do not currently have an active account selected. Please run: $ gcloud auth login"
        );
        assert_eq!(stderr_summary(&"x".repeat(1000)).chars().count(), 400);
    }

    #[test]
    fn maps_http_errors() {
        let action = Action::new("eks:UpdateClusterVersion", "cluster prod-eu-west-1");
        let error = |message: &str| ApiError {
            message: message.into(),
            ..ApiError::default()
        };
        assert_eq!(
            error_for(403, error("denied"), &action, "AWS", None),
            ProviderError::Forbidden {
                verb: "eks:UpdateClusterVersion".into(),
                resource: "cluster prod-eu-west-1".into()
            }
        );
        assert!(matches!(
            error_for(404, error("No cluster found"), &action, "AWS", None),
            ProviderError::NotFound(m) if m.contains("prod-eu-west-1") && m.contains("No cluster found")
        ));
        assert!(matches!(
            error_for(401, error("expired"), &action, "Google Cloud", Some("gcloud auth login".into())),
            ProviderError::Credentials { command: Some(c), .. } if c == "gcloud auth login"
        ));
        assert!(matches!(
            error_for(503, error(""), &action, "AWS", None),
            ProviderError::Unavailable(m) if m.contains("503")
        ));
        assert!(matches!(
            error_for(409, error("An update is already in progress"), &action, "AWS", None),
            ProviderError::Other(m) if m.contains("already in progress")
        ));
    }

    #[test]
    fn caches_until_shortly_before_expiry() {
        let now: Timestamp = "2026-09-26T12:00:00Z".parse().unwrap();
        let expires: Timestamp = "2026-09-26T13:00:00Z".parse().unwrap();
        assert_eq!(
            fresh_until(Some(expires), Duration::from_secs(60), now),
            "2026-09-26T12:55:00Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(
            fresh_until(None, Duration::from_secs(60), now),
            "2026-09-26T12:01:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[tokio::test]
    async fn cache_fetches_once_and_clears() {
        let cache: Cache<u32> = Cache::default();
        let far = Timestamp::now() + Duration::from_secs(3600);
        assert_eq!(cache.get(|| async { Ok((1, far)) }).await.unwrap(), 1);
        assert_eq!(cache.get(|| async { Ok((2, far)) }).await.unwrap(), 1);
        cache.clear().await;
        assert_eq!(cache.get(|| async { Ok((3, far)) }).await.unwrap(), 3);
        // A value that's already due is fetched again.
        cache.clear().await;
        let past = Timestamp::now() - Duration::from_secs(1);
        assert_eq!(cache.get(|| async { Ok((4, past)) }).await.unwrap(), 4);
        assert_eq!(cache.get(|| async { Ok((5, far)) }).await.unwrap(), 5);
    }

    #[test]
    fn debug_never_shows_tokens() {
        let token = BearerToken(SecretString::from("ya29.secret-token"));
        let request = Request::new(
            Method::GET,
            Url::parse("https://container.googleapis.com/v1/projects/p").unwrap(),
        )
        .bearer(&token);
        for text in [format!("{token:?}"), format!("{request:?}")] {
            assert!(!text.contains("secret-token"), "{text}");
        }
        assert!(format!("{:?}", request.headers).contains("Sensitive"));
    }

    #[test]
    fn urls_and_encoding() {
        assert_eq!(encode("a b/c~d_e.f-g"), "a%20b%2Fc~d_e.f-g");
        let base = Url::parse("https://eks.eu-west-1.amazonaws.com").unwrap();
        let url = url(
            &base,
            "/addons/supported-versions",
            &[("addonName", "vpc-cni"), ("kubernetesVersion", "1.31")],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://eks.eu-west-1.amazonaws.com/addons/supported-versions?addonName=vpc-cni&kubernetesVersion=1.31"
        );
        assert_eq!(
            server_host("https://ABC.gr7.eu-west-1.eks.amazonaws.com:443").as_deref(),
            Some("abc.gr7.eu-west-1.eks.amazonaws.com")
        );
    }

    #[test]
    fn timestamps() {
        let epoch = timestamp(&serde_json::json!(1.7e9)).unwrap();
        assert_eq!(epoch.as_second(), 1_700_000_000);
        assert_eq!(
            date(timestamp(&serde_json::json!("2026-07-23T00:00:00Z")).unwrap()),
            "2026-07-23"
        );
        assert_eq!(
            date(timestamp(&serde_json::json!("2026-07-23")).unwrap()),
            "2026-07-23"
        );
        assert!(timestamp(&serde_json::json!(null)).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_clis_without_stdin_with_a_timeout() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let script = |name: &str, body: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        let env = CliEnv::new([("KUBYL_TEST".to_string(), "value".to_string())]);

        // stdout comes back; env is passed; stdin is closed (read gets EOF at once).
        let ok = script("ok.sh", "read line; echo \"out:$KUBYL_TEST:$line\"");
        let out = run_program(&ok, &[], &env, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out).trim(), "out:value:");

        let failing = script(
            "fail.sh",
            "echo secret-stdout; echo 'token expired' >&2; exit 255",
        );
        assert_eq!(
            run_program(&failing, &[], &env, Duration::from_secs(5)).await,
            Err(CliError::Failed {
                code: Some(255),
                stderr: "token expired".into()
            })
        );

        let slow = script("slow.sh", "sleep 5");
        assert_eq!(
            run_program(&slow, &[], &env, Duration::from_millis(200)).await,
            Err(CliError::TimedOut)
        );

        assert_eq!(
            run_program("kubyl-no-such-cli", &[], &env, Duration::from_secs(5)).await,
            Err(CliError::NotFound)
        );
        let error = CliError::NotFound.to_provider_error(AWS, "aws configure", None);
        assert!(
            matches!(&error, ProviderError::Unavailable(m) if m.contains("AWS CLI") && m.contains("getting-started-install")),
            "{error:?}"
        );
    }
}
