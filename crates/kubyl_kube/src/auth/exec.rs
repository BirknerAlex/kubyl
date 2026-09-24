//! Exec credential plugins (`aws eks get-token`, `gke-gcloud-auth-plugin`, `kubelogin`…).
//!
//! Plugins run on the Tokio runtime with the login shell's `PATH`. Their credential is cached
//! until shortly before it expires, shared by every context with the same exec config. stderr
//! is captured and shown when a plugin fails. Plugins with `interactiveMode: Always` get a
//! prompt modal in the UI that shows their output and sends typed lines to their stdin.

use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use jiff::Timestamp;
use kube::config::{ExecAuthCluster, ExecConfig, ExecInteractiveMode};
use parking_lot::Mutex;
use secrecy::SecretString;
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::{AuthError, shell_env};

/// Refresh credentials this long before they expire.
const EXPIRY_MARGIN: Duration = Duration::from_secs(60);
/// Plugins that wait for a browser sign-in can take a while.
const TIMEOUT: Duration = Duration::from_secs(300);

/// A credential returned by a plugin.
#[derive(Clone)]
pub enum Credential {
    Token(SecretString),
    ClientCertificate {
        certificate: String,
        key: SecretString,
    },
}

#[derive(Clone)]
struct Cached {
    credential: Credential,
    /// `None`: the plugin gave no expiry; the credential is reused until the plugin is run again
    /// after an authentication failure.
    expires_at: Option<Timestamp>,
}

impl Cached {
    fn is_fresh(&self) -> bool {
        self.expires_at
            .is_none_or(|at| Timestamp::now() + EXPIRY_MARGIN < at)
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Cached>>>;

/// Cached credentials by exec config, shared across contexts.
static CACHE: LazyLock<Mutex<HashMap<u64, Slot>>> = LazyLock::new(Default::default);

/// Runs one exec plugin config and caches its credential.
pub struct ExecAuth {
    context: String,
    config: ExecConfig,
    cluster: Option<ExecAuthCluster>,
    slot: Slot,
}

impl ExecAuth {
    pub fn new(context: String, config: ExecConfig, cluster: Option<ExecAuthCluster>) -> Self {
        let key = cache_key(&config, cluster.as_ref());
        let slot = CACHE.lock().entry(key).or_default().clone();
        Self {
            context,
            config,
            cluster,
            slot,
        }
    }

    /// The cached credential, or a fresh one from the plugin.
    pub async fn credential(&self) -> Result<Credential, AuthError> {
        let mut slot = self.slot.lock().await;
        if let Some(cached) = slot.as_ref().filter(|c| c.is_fresh()) {
            return Ok(cached.credential.clone());
        }
        let cached = run(&self.context, &self.config, self.cluster.as_ref()).await?;
        let credential = cached.credential.clone();
        *slot = Some(cached);
        Ok(credential)
    }

    pub async fn expires_at(&self) -> Option<Timestamp> {
        self.slot.lock().await.as_ref()?.expires_at
    }

    /// Drops the cached credential, so the next request runs the plugin again (after a 401).
    pub async fn invalidate(&self) {
        self.slot.lock().await.take();
    }
}

fn cache_key(config: &ExecConfig, cluster: Option<&ExecAuthCluster>) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.command.hash(&mut hasher);
    config.args.hash(&mut hasher);
    config.api_version.hash(&mut hasher);
    if let Some(env) = &config.env {
        for var in env {
            let mut pairs: Vec<_> = var.iter().collect();
            pairs.sort();
            pairs.hash(&mut hasher);
        }
    }
    if config.provide_cluster_info {
        cluster.and_then(|c| c.server.clone()).hash(&mut hasher);
    }
    hasher.finish()
}

/// A running interactive plugin, shown in a prompt modal.
pub struct ExecPrompt {
    pub context: String,
    pub command: String,
    /// The plugin's stderr (and stdout of non-JSON lines), in chunks as they arrive.
    pub output: mpsc::UnboundedReceiver<String>,
    /// Lines to write to the plugin's stdin.
    pub input: mpsc::UnboundedSender<String>,
    /// Kills the plugin.
    pub cancel: oneshot::Sender<()>,
}

static PROMPTS: OnceLock<mpsc::UnboundedSender<ExecPrompt>> = OnceLock::new();

/// Receives interactive plugins that need the user. Called once by the UI; without a receiver
/// interactive plugins run like non-interactive ones.
pub(crate) fn prompt_receiver() -> Option<mpsc::UnboundedReceiver<ExecPrompt>> {
    let (tx, rx) = mpsc::unbounded();
    PROMPTS.set(tx).ok().map(|_| rx)
}

#[derive(Deserialize)]
struct ExecCredentialOutput {
    status: Option<ExecCredentialStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecCredentialStatus {
    expiration_timestamp: Option<String>,
    token: Option<String>,
    client_certificate_data: Option<String>,
    client_key_data: Option<String>,
}

/// Parses the plugin's stdout. Errors never include the output (it may hold a token).
fn parse_output(stdout: &[u8]) -> Result<Cached, String> {
    let output: ExecCredentialOutput = serde_json::from_slice(stdout).map_err(|err| {
        format!(
            "the plugin's output is not an ExecCredential (line {}, column {})",
            err.line(),
            err.column()
        )
    })?;
    let status = output
        .status
        .ok_or("the plugin's ExecCredential has no status")?;
    let expires_at = status
        .expiration_timestamp
        .map(|ts| ts.parse::<Timestamp>())
        .transpose()
        .map_err(|err| format!("invalid expirationTimestamp: {err}"))?;
    let credential = match (
        status.token,
        status.client_certificate_data,
        status.client_key_data,
    ) {
        (_, Some(certificate), Some(key)) => Credential::ClientCertificate {
            certificate,
            key: SecretString::from(key),
        },
        (Some(token), _, _) => Credential::Token(SecretString::from(token)),
        _ => return Err("the plugin returned neither a token nor a client certificate".into()),
    };
    Ok(Cached {
        credential,
        expires_at,
    })
}

async fn run(
    context: &str,
    config: &ExecConfig,
    cluster: Option<&ExecAuthCluster>,
) -> Result<Cached, AuthError> {
    let command = config.command.clone().ok_or_else(|| AuthError::Exec {
        message: "the exec config has no command".into(),
        stderr: None,
    })?;
    let path = shell_env::path();
    let interactive =
        config.interactive_mode == Some(ExecInteractiveMode::Always) && PROMPTS.get().is_some();

    let exec_info = serde_json::json!({
        "apiVersion": config.api_version.clone()
            .unwrap_or_else(|| "client.authentication.k8s.io/v1beta1".into()),
        "kind": "ExecCredential",
        "spec": {
            "interactive": interactive,
            "cluster": if config.provide_cluster_info { serde_json::to_value(cluster).ok() } else { None },
        },
    });

    let mut cmd = resolve_command(&command, path.as_ref());
    if let Some(args) = &config.args {
        cmd.args(args);
    }
    if let Some(path) = &path {
        cmd.env("PATH", path);
    }
    for var in config.env.iter().flatten() {
        if let (Some(name), Some(value)) = (var.get("name"), var.get("value")) {
            cmd.env(name, value);
        }
    }
    cmd.env("KUBERNETES_EXEC_INFO", exec_info.to_string())
        .stdin(if interactive {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    tracing::info!(context, command = %command, "running exec credential plugin");
    let mut child = cmd.spawn().map_err(|err| AuthError::Exec {
        message: match (err.kind(), &config.install_hint) {
            (std::io::ErrorKind::NotFound, Some(hint)) => {
                format!("{command} was not found. {}", hint.trim())
            }
            (std::io::ErrorKind::NotFound, None) => format!("{command} was not found in PATH"),
            _ => format!("couldn't start {command}: {err}"),
        },
        stderr: None,
    })?;

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (output_tx, output_rx) = mpsc::unbounded::<String>();
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let stdin_task = if interactive {
        let (input_tx, mut input_rx) = mpsc::unbounded::<String>();
        let mut stdin = child.stdin.take().expect("piped stdin");
        let prompt = ExecPrompt {
            context: context.to_string(),
            command: command.clone(),
            output: output_rx,
            input: input_tx,
            cancel: cancel_tx,
        };
        PROMPTS.get().map(|p| p.unbounded_send(prompt));
        Some(tokio::spawn(async move {
            while let Some(line) = input_rx.next().await {
                if stdin
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                stdin.flush().await.ok();
            }
        }))
    } else {
        drop((output_rx, cancel_tx));
        None
    };

    let read_stdout = async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    };
    let read_stderr = async move {
        let mut all = String::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&chunk[..n]).into_owned();
                    output_tx.unbounded_send(text.clone()).ok();
                    all.push_str(&text);
                }
            }
        }
        all
    };
    // A dropped sender (no prompt modal) must not count as a cancel.
    let cancelled = async move {
        if cancel_rx.await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let result = tokio::select! {
        result = async {
            let (stdout, stderr, status) = tokio::join!(read_stdout, read_stderr, child.wait());
            (stdout, stderr, status)
        } => Some(result),
        _ = tokio::time::sleep(TIMEOUT) => None,
        _ = cancelled => {
            return Err(AuthError::Exec { message: format!("{command} was cancelled"), stderr: None });
        }
    };
    if let Some(task) = stdin_task {
        task.abort();
    }
    let Some((stdout, stderr, status)) = result else {
        return Err(AuthError::Exec {
            message: format!("{command} didn't finish within {} s", TIMEOUT.as_secs()),
            stderr: None,
        });
    };
    let stderr = Some(stderr.trim().to_string()).filter(|s| !s.is_empty());
    let status = status.map_err(|err| AuthError::Exec {
        message: format!("{command} failed: {err}"),
        stderr: stderr.clone(),
    })?;
    if !status.success() {
        return Err(AuthError::Exec {
            message: format!(
                "{command} exited with {}",
                status
                    .code()
                    .map(|c| format!("code {c}"))
                    .unwrap_or_else(|| "a signal".into())
            ),
            stderr,
        });
    }
    let stdout = stdout.map_err(|err| AuthError::Exec {
        message: format!("couldn't read the output of {command}: {err}"),
        stderr: stderr.clone(),
    })?;
    parse_output(&stdout).map_err(|message| AuthError::Exec {
        message: format!("{command}: {message}"),
        stderr,
    })
}

/// The command to run. On Windows, plugins installed as `.cmd`/`.bat` shims (gcloud, az) must
/// be run through `cmd /C`, and `Command` doesn't look for them by itself.
fn resolve_command(command: &str, path: Option<&OsString>) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let has_extension = std::path::Path::new(command).extension().is_some();
        if !has_extension {
            let dirs: Vec<_> = path
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default();
            for dir in &dirs {
                if dir.join(format!("{command}.exe")).is_file() {
                    return tokio::process::Command::new(dir.join(format!("{command}.exe")));
                }
                for ext in ["cmd", "bat"] {
                    let shim = dir.join(format!("{command}.{ext}"));
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
        // `Command` looks up the program in the child's PATH only on some platforms; resolve it
        // against the login shell PATH ourselves.
        if !command.contains('/')
            && let Some(path) = path
        {
            for dir in std::env::split_paths(path) {
                let candidate = dir.join(command);
                if candidate.is_file() {
                    return tokio::process::Command::new(candidate);
                }
            }
        }
    }
    tokio::process::Command::new(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret as _;

    #[test]
    fn parses_tokens_and_certificates() {
        let token = parse_output(
            br#"{"kind":"ExecCredential","status":{"token":"t0k","expirationTimestamp":"2030-01-01T00:00:00Z"}}"#,
        )
        .ok()
        .unwrap();
        assert!(matches!(&token.credential, Credential::Token(t) if t.expose_secret() == "t0k"));
        assert!(token.is_fresh());

        let cert = parse_output(br#"{"status":{"clientCertificateData":"C","clientKeyData":"K"}}"#)
            .ok()
            .unwrap();
        assert!(matches!(
            cert.credential,
            Credential::ClientCertificate { .. }
        ));
        assert!(cert.expires_at.is_none() && cert.is_fresh());

        let expired = parse_output(
            br#"{"status":{"token":"x","expirationTimestamp":"2001-01-01T00:00:00Z"}}"#,
        )
        .ok()
        .unwrap();
        assert!(!expired.is_fresh());
    }

    #[test]
    fn parse_errors_never_contain_the_output() {
        let err = parse_output(br#"{"status": {"token": "sekrit"#)
            .err()
            .unwrap();
        assert!(!err.contains("sekrit"));
        let err = parse_output(br#"{"status":{}}"#).err().unwrap();
        assert!(err.contains("neither"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_plugins_captures_stderr_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("count");
        let script = dir.path().join("plugin.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho run >> {}\necho '{{\"status\":{{\"token\":\"abc\"}}}}'\n",
                counter.display()
            ),
        )
        .unwrap();
        let failing = dir.path().join("fail.sh");
        std::fs::write(
            &failing,
            "#!/bin/sh\necho 'profile not found' >&2\nexit 3\n",
        )
        .unwrap();
        for file in [&script, &failing] {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let config = |command: &std::path::Path| ExecConfig {
            command: Some(command.display().to_string()),
            api_version: Some("client.authentication.k8s.io/v1".into()),
            ..Default::default()
        };
        let auth = ExecAuth::new("ctx".into(), config(&script), None);
        let Credential::Token(token) = auth.credential().await.ok().unwrap() else {
            panic!("expected a token");
        };
        assert_eq!(token.expose_secret(), "abc");
        // Same config: served from the cache, even through a second ExecAuth.
        let again = ExecAuth::new("other".into(), config(&script), None);
        again.credential().await.ok().unwrap();
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            1
        );
        again.invalidate().await;
        auth.credential().await.ok().unwrap();
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );

        let failing = ExecAuth::new("ctx".into(), config(&failing), None);
        match failing.credential().await {
            Err(AuthError::Exec { message, stderr }) => {
                assert!(message.contains("code 3"), "{message}");
                assert_eq!(stderr.as_deref(), Some("profile not found"));
            }
            _ => panic!("expected an exec error"),
        }

        let missing = ExecAuth::new(
            "ctx".into(),
            ExecConfig {
                command: Some("kubyl-no-such-plugin".into()),
                install_hint: Some("Install it.".into()),
                ..Default::default()
            },
            None,
        );
        match missing.credential().await {
            Err(AuthError::Exec { message, .. }) => {
                assert!(message.contains("Install it."), "{message}")
            }
            _ => panic!("expected an exec error"),
        }
    }
}
