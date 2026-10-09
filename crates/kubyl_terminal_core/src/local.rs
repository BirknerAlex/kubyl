//! A local shell with a cluster context (phase 25): a PTY running the user's shell, with
//! `KUBECONFIG` pointing at a kubeconfig that holds only that context.
//!
//! - [`kubeconfig_text`] makes the one-context kubeconfig from the file that defines the
//!   context: relative file paths (CA, client certificate and key, token file, exec plugin) are
//!   made absolute, since the copy lives in another folder.
//! - Contexts that sign in through Kubyl (OIDC, OpenShift OAuth: [`CliTarget::token`]) can't
//!   sign in from a kubeconfig. Their user is replaced by an exec plugin that prints the token
//!   of the `KUBYL_KUBE_TOKEN` environment variable of the shell, so the token is only ever in
//!   the shell's environment: never in arguments (`ps` shows those), never in a file, never in
//!   a log. It is the sign-in at the time the tab opened, so it expires like the sign-in does;
//!   open a new tab then.
//! - Everything else (client certificates, static tokens, exec plugins) is copied as it is:
//!   the user's own credentials, in a private folder (mode 0700, file 0600) that goes when the
//!   tab closes ([`ShellKubeconfig`]'s `Drop`) and is swept at startup after a crash
//!   ([`sweep_stale`]).
//! - [`run`] drives the PTY on Tokio with the same channels as [`crate::exec::run`], so the
//!   terminal view treats both alike.
//!
//! A local shell is the user's own `kubectl`: it bypasses Kubyl's read-only and PROD
//! protection. The UI says so and asks for the typed cluster name on PROD.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::task::Poll;
use std::time::Duration;

use anyhow::Context as _;
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use kube::config::Kubeconfig;
use kubyl_kube_core::cli::CliTarget;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use secrecy::ExposeSecret as _;
use serde_json::{Value, json};

use crate::exec::Ending;

/// The environment variable that carries Kubyl's token for the exec plugin of the one-context
/// kubeconfig.
pub const TOKEN_ENV: &str = "KUBYL_KUBE_TOKEN";

/// Folder (in the temp dir) that holds the kubeconfigs of open local shells; one per user,
/// since `/tmp` is shared on Linux.
fn root() -> PathBuf {
    #[cfg(unix)]
    let user = unsafe { libc::getuid() }.to_string();
    #[cfg(not(unix))]
    let user = String::from("user");
    std::env::temp_dir().join(format!("kubyl-local-shells-{user}"))
}

/// Makes `path` readable by its owner only.
fn private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// How long a shell gets to exit after its output ended.
const EXIT_GRACE: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------------------------
// The one-context kubeconfig

/// A kubeconfig with only `context` from `source`, as JSON (which `kubectl` reads).
///
/// `kubyl_token`: the context signs in through Kubyl; its user becomes the exec plugin that
/// prints [`TOKEN_ENV`]. `namespace` replaces the context's namespace (the one the tab was
/// opened in).
pub fn kubeconfig_text(
    source: &Path,
    context: &str,
    namespace: Option<&str>,
    kubyl_token: bool,
) -> anyhow::Result<String> {
    let config =
        Kubeconfig::read_from(source).with_context(|| format!("reading {}", source.display()))?;
    let base = source.parent().unwrap_or_else(|| Path::new("."));
    let named = config
        .contexts
        .iter()
        .find(|c| c.name == context)
        .and_then(|c| c.context.clone())
        .with_context(|| format!("{context} isn't in {}", source.display()))?;
    let cluster = config
        .clusters
        .iter()
        .find(|c| c.name == named.cluster)
        .and_then(|c| c.cluster.clone())
        .with_context(|| format!("the cluster of {context} isn't in the file"))?;
    let mut cluster = serde_json::to_value(&cluster)?;
    absolutize(&mut cluster, &["certificate-authority"], base);

    let user = match (&named.user, kubyl_token) {
        (_, true) => Some(("kubyl".to_string(), exec_user())),
        (Some(name), false) => {
            let auth = config
                .auth_infos
                .iter()
                .find(|u| &u.name == name)
                .and_then(|u| u.auth_info.clone())
                .with_context(|| format!("the user of {context} isn't in the file"))?;
            let mut auth = serde_json::to_value(&auth)?;
            absolutize(
                &mut auth,
                &["client-certificate", "client-key", "tokenFile"],
                base,
            );
            if let Some(command) = auth.pointer_mut("/exec/command")
                && let Some(text) = command.as_str()
                // Only commands with a directory are relative to the file; others come from PATH.
                && text.contains(['/', '\\'])
                && Path::new(text).is_relative()
            {
                *command = json!(base.join(text).to_string_lossy());
            }
            Some((name.clone(), auth))
        }
        (None, false) => None,
    };

    let mut context_json = json!({ "cluster": named.cluster });
    if let Some((name, _)) = &user {
        context_json["user"] = json!(name);
    }
    if let Some(namespace) = namespace.or(named.namespace.as_deref()) {
        context_json["namespace"] = json!(namespace);
    }
    let users: Vec<Value> = user
        .into_iter()
        .map(|(name, user)| json!({ "name": name, "user": user }))
        .collect();
    let out = json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": context,
        "clusters": [{ "name": named.cluster, "cluster": cluster }],
        "contexts": [{ "name": context, "context": context_json }],
        "users": users,
    });
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Makes the relative paths in `keys` of `value` absolute against `base`.
fn absolutize(value: &mut Value, keys: &[&str], base: &Path) {
    for key in keys {
        if let Some(path) = value.get_mut(*key)
            && let Some(text) = path.as_str()
            && Path::new(text).is_relative()
        {
            *path = json!(base.join(text).to_string_lossy());
        }
    }
}

/// The user that prints [`TOKEN_ENV`] as the credential. The token is never in the kubeconfig:
/// the plugin reads it from the environment `kubectl` passes on.
fn exec_user() -> Value {
    #[cfg(not(windows))]
    let (command, args) = (
        "sh",
        vec![
            "-c".to_string(),
            format!(
                "printf '{{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{{\"token\":\"%s\"}}}}' \"${TOKEN_ENV}\""
            ),
        ],
    );
    #[cfg(windows)]
    let (command, args) = (
        "powershell",
        vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            format!(
                "[Console]::Out.Write('{{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{{\"token\":\"' + $env:{TOKEN_ENV} + '\"}}}}')"
            ),
        ],
    );
    json!({ "exec": {
        "apiVersion": "client.authentication.k8s.io/v1",
        "command": command,
        "args": args,
        "interactiveMode": "Never",
        "provideClusterInfo": false,
    }})
}

/// A private folder with the shell's kubeconfig in it; both go when this is dropped.
pub struct ShellKubeconfig {
    dir: tempfile::TempDir,
    file: PathBuf,
}

impl ShellKubeconfig {
    /// Writes `text` (mode 0600 in a 0700 folder on Unix; the folder is `<temp>/kubyl-local-shells-<uid>/<pid>-…`).
    pub fn write(text: &str) -> std::io::Result<Self> {
        let root = root();
        std::fs::create_dir_all(&root)?;
        private(&root)?;
        let dir = tempfile::Builder::new()
            .prefix(&format!("{}-", std::process::id()))
            .tempdir_in(&root)?;
        private(dir.path())?;
        let file = dir.path().join("config");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options.open(&file)?.write_all(text.as_bytes())?;
        Ok(Self { dir, file })
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

impl fmt::Debug for ShellKubeconfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShellKubeconfig")
            .field("file", &self.file)
            .finish()
    }
}

/// Removes the folders of local shells that Kubyl left behind (a crash, a kill): those of
/// other processes that are gone, and any older than a week.
pub fn sweep_stale() {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return;
    };
    let ours = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .split('-')
            .next()
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(7 * 24 * 3600));
        if pid != ours && (old || !process_alive(pid)) {
            std::fs::remove_dir_all(entry.path()).ok();
        }
    }
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Signal 0 only checks; EPERM means it exists (another user's).
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_: u32) -> bool {
    // Without a cheap check, leave them to the age limit.
    true
}

// ---------------------------------------------------------------------------------------------
// The shell

/// The shell to run: the setting, else `$SHELL`, else `%COMSPEC%`, else `/bin/sh`.
pub fn default_shell(configured: Option<&str>) -> String {
    pick_shell(
        configured,
        std::env::var("SHELL").ok().as_deref(),
        std::env::var("COMSPEC").ok().as_deref(),
    )
}

fn pick_shell(configured: Option<&str>, shell: Option<&str>, comspec: Option<&str>) -> String {
    [configured, shell, comspec]
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "powershell.exe".into()
            } else {
                "/bin/sh".into()
            }
        })
}

/// The environment of the shell: `base` (this process's) with `KUBECONFIG` pointing at the
/// one-context file, the login shell's `PATH` (a GUI app starts with a minimal one), a terminal
/// type, and the token for contexts that sign in through Kubyl. Anything that would send
/// `kubectl` elsewhere, and a stale token variable, is dropped first.
pub fn shell_env(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    kubeconfig: &Path,
    context: &str,
    path: Option<OsString>,
    token: Option<&str>,
) -> Vec<(OsString, OsString)> {
    let same = |a: &OsStr, b: &str| {
        if cfg!(windows) {
            a.to_string_lossy().eq_ignore_ascii_case(b)
        } else {
            a == OsStr::new(b)
        }
    };
    let mut env: Vec<(OsString, OsString)> = base
        .into_iter()
        .filter(|(name, _)| {
            ![
                "KUBECONFIG",
                TOKEN_ENV,
                "KUBYL_CONTEXT",
                "PATH",
                "TERM",
                "COLORTERM",
            ]
            .iter()
            .any(|n| same(name, n))
        })
        .collect();
    env.push(("KUBECONFIG".into(), kubeconfig.as_os_str().to_owned()));
    env.push(("KUBYL_CONTEXT".into(), context.into()));
    env.push(("TERM".into(), "xterm-256color".into()));
    env.push(("COLORTERM".into(), "truecolor".into()));
    if let Some(path) = path {
        env.push(("PATH".into(), path));
    } else if let Some(path) = std::env::var_os("PATH") {
        env.push(("PATH".into(), path));
    }
    if let Some(token) = token {
        env.push((TOKEN_ENV.into(), token.into()));
    }
    env
}

/// Everything needed to start a local shell. Its kubeconfig goes when it is dropped.
pub struct LaunchPlan {
    pub program: String,
    pub args: Vec<String>,
    env: Vec<(OsString, OsString)>,
    pub cwd: Option<PathBuf>,
    kubeconfig: ShellKubeconfig,
}

impl LaunchPlan {
    /// A plan from the one-context kubeconfig `text` ([`kubeconfig_text`]): writes it to a
    /// private folder and sets up the environment of the process around it. `token` is the
    /// Kubyl sign-in for [`TOKEN_ENV`], `shell` the `terminal.local_shell` setting.
    pub fn build(
        text: &str,
        context: &str,
        token: Option<&str>,
        path: Option<OsString>,
        shell: Option<&str>,
    ) -> std::io::Result<Self> {
        let kubeconfig = ShellKubeconfig::write(text)?;
        let env = shell_env(std::env::vars_os(), kubeconfig.path(), context, path, token);
        Ok(Self {
            program: default_shell(shell),
            args: Vec::new(),
            env,
            cwd: dirs_home(),
            kubeconfig,
        })
    }

    pub fn kubeconfig(&self) -> &Path {
        self.kubeconfig.path()
    }

    /// The names (never the values) of the environment, for tests and diagnostics.
    pub fn env_names(&self) -> Vec<String> {
        self.env
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect()
    }

    /// The value of one variable, for tests.
    pub fn env_value(&self, name: &str) -> Option<&OsStr> {
        self.env
            .iter()
            .find(|(n, _)| n == OsStr::new(name))
            .map(|(_, v)| v.as_os_str())
    }
}

/// Only names: the environment holds the token.
impl fmt::Debug for LaunchPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchPlan")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &self.env_names())
            .field("cwd", &self.cwd)
            .field("kubeconfig", &self.kubeconfig)
            .finish()
    }
}

/// What to open.
pub struct Request {
    pub target: CliTarget,
    /// The namespace the tab was opened in.
    pub namespace: Option<String>,
    /// `terminal.local_shell`.
    pub shell: Option<String>,
    /// The login shell's `PATH`.
    pub path: Option<OsString>,
}

/// Prepares a shell: reads the context's kubeconfig, asks Kubyl for the token when the context
/// signs in through it, and writes the one-context file. Run it on Tokio (it reads files and
/// may refresh a token).
pub async fn prepare(request: Request) -> anyhow::Result<LaunchPlan> {
    let Request {
        target,
        namespace,
        shell,
        path,
    } = request;
    // `None` for contexts the CLI signs in to by itself.
    let token = target
        .token()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("the cluster needs a sign-in in Kubyl first")?;
    let source = target.kubeconfig.clone();
    let context = target.context.clone();
    let kubyl_token = token.is_some();
    let text = tokio::task::spawn_blocking({
        let namespace = namespace.clone();
        move || kubeconfig_text(&source, &target.context, namespace.as_deref(), kubyl_token)
    })
    .await??;
    // Writes a file: not on a runtime thread.
    let token = token.map(|t| t.expose_secret().to_string());
    let plan = tokio::task::spawn_blocking(move || {
        LaunchPlan::build(&text, &context, token.as_deref(), path, shell.as_deref())
    })
    .await??;
    Ok(plan)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

// ---------------------------------------------------------------------------------------------
// The session

/// Kills the shell when the session is dropped (the tab closed), whatever way that happens.
struct KillOnDrop(Box<dyn portable_pty::ChildKiller + Send + Sync>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        self.0.kill().ok();
    }
}

/// Runs the shell until it exits or `input` closes. The same channels as
/// [`crate::exec::run`]: `output` is bounded (a flood is read only as fast as the view draws
/// it), `resize` carries `(columns, rows)`, `connected` is signaled once the shell started.
/// Returns whether the shell exited (rather than the tab closing). The plan, and with it the
/// kubeconfig, lives as long as the session.
pub async fn run(
    plan: LaunchPlan,
    size: (u16, u16),
    mut input: mpsc::UnboundedReceiver<Vec<u8>>,
    mut output: mpsc::Sender<Vec<u8>>,
    mut resize: mpsc::UnboundedReceiver<(u16, u16)>,
    connected: oneshot::Sender<Result<(), String>>,
) -> anyhow::Result<Ending> {
    let started = tokio::task::spawn_blocking({
        let (program, args, env, cwd) = (
            plan.program.clone(),
            plan.args.clone(),
            plan.env.clone(),
            plan.cwd.clone(),
        );
        move || spawn(&program, &args, env, cwd, size)
    })
    .await?;
    let process = match started {
        Ok(process) => {
            connected.send(Ok(())).ok();
            process
        }
        Err(err) => {
            let message = format!("couldn't start {}: {err:#}", plan.program);
            connected.send(Err(message.clone())).ok();
            anyhow::bail!(message);
        }
    };
    let _kill = KillOnDrop(process.child.clone_killer());

    // Blocking reads on a thread of their own; bounded, so the PTY is read as fast as it's used.
    let (read_tx, mut read_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let mut reader = process.reader;
    std::thread::Builder::new()
        .name("kubyl-local-shell-read".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if read_tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        })?;
    // Writes can block when the shell isn't reading: a thread of their own, unbounded queue.
    let (write_tx, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let mut writer = process.writer;
    std::thread::Builder::new()
        .name("kubyl-local-shell-write".into())
        .spawn(move || {
            while let Ok(bytes) = write_rx.recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })?;
    let (exit_tx, exit_rx) = oneshot::channel();
    let mut child = process.child;
    std::thread::Builder::new()
        .name("kubyl-local-shell-wait".into())
        .spawn(move || {
            let status = child.wait().ok().map(|s| s.exit_code() as i32);
            exit_tx.send(status).ok();
        })?;

    let mut closed = false;
    loop {
        tokio::select! {
            bytes = input.next() => {
                let Some(bytes) = bytes else { closed = true; break };
                if write_tx.send(bytes).is_err() {
                    break;
                }
            }
            size = resize.next() => {
                if let Some((columns, rows)) = size {
                    process.master.resize(PtySize { rows, cols: columns, pixel_width: 0, pixel_height: 0 }).ok();
                }
            }
            chunk = read_rx.recv() => {
                let Some(chunk) = chunk else { break };
                if futures::future::poll_fn(|cx| match output.poll_ready(cx) {
                    Poll::Ready(Ok(())) => Poll::Ready(true),
                    Poll::Ready(Err(_)) => Poll::Ready(false),
                    Poll::Pending => Poll::Pending,
                })
                .await
                    && output.start_send(chunk).is_ok()
                {
                    continue;
                }
                closed = true;
                break;
            }
        }
    }
    drop(write_tx);
    if closed {
        return Ok(Ending::Dropped);
    }
    // The output ended: the shell exited (or is about to).
    let code = match tokio::time::timeout(EXIT_GRACE, exit_rx).await {
        Ok(Ok(code)) => code,
        _ => None,
    };
    Ok(Ending::Exited { code })
}

/// A started shell.
struct Process {
    master: Box<dyn portable_pty::MasterPty + Send>,
    reader: Box<dyn std::io::Read + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

fn spawn(
    program: &str,
    args: &[String],
    env: Vec<(OsString, OsString)>,
    cwd: Option<PathBuf>,
    (columns, rows): (u16, u16),
) -> anyhow::Result<Process> {
    let pair = native_pty_system().openpty(PtySize {
        rows: rows.max(1),
        cols: columns.max(1),
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut command = CommandBuilder::new(program);
    command.args(args);
    // Exactly the environment of the plan.
    command.env_clear();
    for (name, value) in env {
        command.env(name, value);
    }
    if let Some(cwd) = cwd {
        command.cwd(cwd);
    }
    let child = pair.slave.spawn_command(command)?;
    // The child holds the slave end; ours would keep the PTY open after it exits.
    drop(pair.slave);
    Ok(Process {
        reader: pair.master.try_clone_reader()?,
        writer: pair.master.take_writer()?,
        master: pair.master,
        child,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
apiVersion: v1
kind: Config
current-context: kind
clusters:
- name: kind-cluster
  cluster:
    server: https://127.0.0.1:6443
    certificate-authority: certs/ca.crt
- name: other
  cluster:
    server: https://other.example.com
contexts:
- name: kind
  context: {cluster: kind-cluster, user: admin, namespace: shop}
- name: other
  context: {cluster: other, user: oidc-user}
users:
- name: admin
  user:
    client-certificate: certs/admin.crt
    client-key: certs/admin.key
- name: oidc-user
  user:
    auth-provider:
      name: oidc
      config: {id-token: ID-TOKEN-SECRET, refresh-token: REFRESH-SECRET, client-secret: CLIENT-SECRET}
- name: execer
  user:
    exec:
      apiVersion: client.authentication.k8s.io/v1
      command: ./bin/get-token
"#;

    fn write(config: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config");
        std::fs::write(&file, config).unwrap();
        (dir, file)
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn holds_only_the_context_with_absolute_paths() {
        let (dir, file) = write(CONFIG);
        let text = kubeconfig_text(&file, "kind", None, false).unwrap();
        let value = parse(&text);
        assert_eq!(value["current-context"], "kind");
        assert_eq!(value["contexts"].as_array().unwrap().len(), 1);
        assert_eq!(value["clusters"].as_array().unwrap().len(), 1);
        assert_eq!(value["contexts"][0]["context"]["namespace"], "shop");
        assert!(!text.contains("oidc-user") && !text.contains("other.example.com"));
        // Paths are relative to the original file, which isn't where the copy lives.
        let ca = value["clusters"][0]["cluster"]["certificate-authority"]
            .as_str()
            .unwrap();
        assert_eq!(Path::new(ca), dir.path().join("certs/ca.crt"));
        let user = &value["users"][0]["user"];
        assert_eq!(
            Path::new(user["client-key"].as_str().unwrap()),
            dir.path().join("certs/admin.key")
        );
    }

    #[test]
    fn the_namespace_of_the_tab_wins() {
        let (_dir, file) = write(CONFIG);
        let value = parse(&kubeconfig_text(&file, "kind", Some("payments"), false).unwrap());
        assert_eq!(value["contexts"][0]["context"]["namespace"], "payments");
    }

    #[test]
    fn relative_exec_commands_are_resolved_and_path_commands_are_not() {
        let (dir, file) = write(&CONFIG.replace("user: admin,", "user: execer,"));
        let value = parse(&kubeconfig_text(&file, "kind", None, false).unwrap());
        let command = value["users"][0]["user"]["exec"]["command"]
            .as_str()
            .unwrap();
        assert_eq!(Path::new(command), dir.path().join("./bin/get-token"));
        let (_dir, file) = write(
            &CONFIG
                .replace("user: admin,", "user: execer,")
                .replace("./bin/get-token", "aws"),
        );
        let value = parse(&kubeconfig_text(&file, "kind", None, false).unwrap());
        assert_eq!(value["users"][0]["user"]["exec"]["command"], "aws");
    }

    #[test]
    fn kubyl_contexts_get_the_env_plugin_and_none_of_their_credentials() {
        let (_dir, file) = write(CONFIG);
        let text = kubeconfig_text(&file, "other", None, true).unwrap();
        for secret in [
            "ID-TOKEN-SECRET",
            "REFRESH-SECRET",
            "CLIENT-SECRET",
            "oidc-user",
        ] {
            assert!(!text.contains(secret), "{secret} is in the kubeconfig");
        }
        let value = parse(&text);
        let exec = &value["users"][0]["user"]["exec"];
        assert_eq!(value["contexts"][0]["context"]["user"], "kubyl");
        assert_eq!(exec["interactiveMode"], "Never");
        // The plugin names the variable; it never holds a token.
        assert!(text.contains(TOKEN_ENV));
        assert!(!text.contains("sha256~") && !text.contains("eyJ"));
    }

    #[test]
    fn unknown_contexts_say_so() {
        let (_dir, file) = write(CONFIG);
        let err = kubeconfig_text(&file, "nope", None, false).unwrap_err();
        assert!(err.to_string().contains("nope isn't in"), "{err:#}");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_goes_with_the_guard() {
        use std::os::unix::fs::PermissionsExt as _;
        let kubeconfig = ShellKubeconfig::write("{}").unwrap();
        let file = kubeconfig.path().to_owned();
        let dir = kubeconfig.dir().to_owned();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
        drop(kubeconfig);
        assert!(!file.exists() && !dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn sweeping_removes_dead_processes_only() {
        let root = root();
        std::fs::create_dir_all(&root).unwrap();
        // The pid of a process that has exited.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        // Names no other run of this test shares, as test processes share the temp root.
        let unique = format!("sweep{}", std::process::id());
        let stale = root.join(format!("{dead}-{unique}"));
        std::fs::create_dir_all(&stale).unwrap();
        let ours = ShellKubeconfig::write("{}").unwrap();
        // pid 1 is alive (and isn't ours).
        let alive = root.join(format!("1-{unique}"));
        std::fs::create_dir_all(&alive).unwrap();
        sweep_stale();
        assert!(!stale.exists());
        assert!(ours.dir().exists());
        assert!(alive.exists());
        std::fs::remove_dir_all(&alive).ok();
    }

    #[test]
    fn the_shell_comes_from_the_setting_then_the_environment() {
        assert_eq!(
            pick_shell(Some("/bin/fish"), Some("/bin/zsh"), None),
            "/bin/fish"
        );
        assert_eq!(
            pick_shell(None, Some("/bin/zsh"), Some("cmd.exe")),
            "/bin/zsh"
        );
        assert_eq!(pick_shell(Some(" "), None, Some("cmd.exe")), "cmd.exe");
        assert!(!pick_shell(None, None, None).is_empty());
    }

    #[test]
    fn the_environment_points_at_the_context_and_carries_the_token_once() {
        let base = vec![
            (OsString::from("HOME"), OsString::from("/home/u")),
            (
                OsString::from("KUBECONFIG"),
                OsString::from("/home/u/.kube/config"),
            ),
            (OsString::from(TOKEN_ENV), OsString::from("stale")),
            (OsString::from("PATH"), OsString::from("/usr/bin")),
        ];
        let env = shell_env(
            base.clone(),
            Path::new("/tmp/k/config"),
            "kind",
            Some("/opt/bin:/usr/bin".into()),
            Some("sha256~secret"),
        );
        let get = |name: &str| -> Vec<&OsString> {
            env.iter()
                .filter(|(n, _)| n == name)
                .map(|(_, v)| v)
                .collect()
        };
        assert_eq!(get("KUBECONFIG"), [&OsString::from("/tmp/k/config")]);
        assert_eq!(get(TOKEN_ENV), [&OsString::from("sha256~secret")]);
        assert_eq!(get("PATH"), [&OsString::from("/opt/bin:/usr/bin")]);
        assert_eq!(get("HOME").len(), 1);
        assert_eq!(get("KUBYL_CONTEXT"), [&OsString::from("kind")]);
        // Without a Kubyl sign-in no token variable exists, and a stale one is dropped.
        let env = shell_env(base, Path::new("/tmp/k/config"), "kind", None, None);
        assert!(env.iter().all(|(n, _)| n != TOKEN_ENV));
    }

    #[test]
    fn debug_output_shows_no_values() {
        let plan = LaunchPlan {
            program: "/bin/sh".into(),
            args: vec![],
            env: shell_env(
                [],
                Path::new("/tmp/x/config"),
                "kind",
                None,
                Some("sha256~secret"),
            ),
            cwd: None,
            kubeconfig: ShellKubeconfig::write("{}").unwrap(),
        };
        let text = format!("{plan:?}");
        assert!(!text.contains("sha256~secret"), "{text}");
        assert!(text.contains(TOKEN_ENV));
    }

    #[cfg(unix)]
    mod pty {
        use super::*;

        fn plan(script: &str, extra: &[(&str, &str)]) -> LaunchPlan {
            let mut env = shell_env(
                std::env::vars_os(),
                Path::new("/tmp/x/config"),
                "kind",
                None,
                None,
            );
            env.extend(extra.iter().map(|(n, v)| ((*n).into(), (*v).into())));
            LaunchPlan {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), script.into()],
                env,
                cwd: None,
                kubeconfig: ShellKubeconfig::write("{}").unwrap(),
            }
        }

        async fn session(
            plan: LaunchPlan,
            keys: Vec<Vec<u8>>,
        ) -> (String, Ending, Result<(), String>) {
            let (input_tx, input_rx) = mpsc::unbounded();
            let (output_tx, mut output_rx) = mpsc::channel(8);
            let (_resize_tx, resize_rx) = mpsc::unbounded();
            let (connected_tx, connected_rx) = oneshot::channel();
            for key in keys {
                input_tx.unbounded_send(key).unwrap();
            }
            let run = tokio::spawn(run(
                plan,
                (80, 24),
                input_rx,
                output_tx,
                resize_rx,
                connected_tx,
            ));
            let connected = connected_rx.await.unwrap();
            let mut text = Vec::new();
            while let Some(chunk) = output_rx.next().await {
                text.extend(chunk);
            }
            let ending = run.await.unwrap().unwrap();
            drop(input_tx);
            (
                String::from_utf8_lossy(&text).into_owned(),
                ending,
                connected,
            )
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn runs_a_shell_with_the_planned_environment() {
            let plan = plan(
                "echo ctx=$KUBYL_CONTEXT cfg=$KUBECONFIG tok=${KUBYL_KUBE_TOKEN:-none}; exit 3",
                &[],
            );
            let (text, ending, connected) = session(plan, vec![]).await;
            assert_eq!(connected, Ok(()));
            assert!(
                text.contains("ctx=kind cfg=/tmp/x/config tok=none"),
                "{text}"
            );
            assert_eq!(ending, Ending::Exited { code: Some(3) });
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn input_reaches_the_shell_and_a_tty_is_attached() {
            let plan = plan("test -t 0 && echo tty=yes; read line; echo got:$line", &[]);
            let (text, ending, _) = session(plan, vec![b"hello\n".to_vec()]).await;
            assert!(text.contains("tty=yes"), "{text}");
            assert!(text.contains("got:hello"), "{text}");
            assert_eq!(ending, Ending::Exited { code: Some(0) });
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_missing_shell_is_reported() {
            let mut plan = plan("true", &[]);
            plan.program = "/nonexistent/shell".into();
            let (_input_tx, input_rx) = mpsc::unbounded();
            let (output_tx, _output_rx) = mpsc::channel(8);
            let (_resize_tx, resize_rx) = mpsc::unbounded();
            let (connected_tx, connected_rx) = oneshot::channel();
            let result = run(plan, (80, 24), input_rx, output_tx, resize_rx, connected_tx).await;
            assert!(result.is_err());
            assert!(
                connected_rx
                    .await
                    .unwrap()
                    .unwrap_err()
                    .contains("couldn't start")
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn closing_the_input_kills_the_shell_and_removes_the_kubeconfig() {
            let plan = plan("sleep 30", &[]);
            let dir = plan.kubeconfig.dir().to_owned();
            let (input_tx, input_rx) = mpsc::unbounded();
            let (output_tx, mut output_rx) = mpsc::channel(8);
            let (_resize_tx, resize_rx) = mpsc::unbounded();
            let (connected_tx, connected_rx) = oneshot::channel();
            let run = tokio::spawn(run(
                plan,
                (80, 24),
                input_rx,
                output_tx,
                resize_rx,
                connected_tx,
            ));
            connected_rx.await.unwrap().unwrap();
            assert!(dir.exists());
            drop(input_tx);
            let ending = run.await.unwrap().unwrap();
            assert_eq!(ending, Ending::Dropped);
            while output_rx.next().await.is_some() {}
            assert!(!dir.exists(), "the kubeconfig outlived the session");
        }
    }
}
