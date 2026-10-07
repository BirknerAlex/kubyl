//! Commands the agent runs through Kubyl (ACP `terminal/*`): local processes with captured
//! output, so the user sees every command and its output in the thread.
//!
//! A command runs through the platform shell (`sh -c` / `cmd /C`) with the login shell's
//! `PATH` and the thread's `KUBECONFIG`. Output is kept up to the agent's byte limit (the start
//! is dropped, as ACP asks) and scrubbed of token shapes before anyone reads it.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::{oneshot, watch};

/// Output kept when the agent sets no limit.
const DEFAULT_LIMIT: usize = 1024 * 1024;

/// How a command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<u32>,
    pub signal: Option<String>,
}

/// What to run.
#[derive(Clone, Debug)]
pub struct Launch {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub output_limit: Option<u64>,
    /// `PATH` for the process (the login shell's).
    pub path: Option<OsString>,
    pub kubeconfig: Option<PathBuf>,
}

#[derive(Default)]
struct Output {
    text: String,
    truncated: bool,
}

/// One running or finished command.
pub struct LocalTerminal {
    pub command_line: String,
    pub cwd: PathBuf,
    output: Mutex<Output>,
    limit: usize,
    exit: watch::Receiver<Option<Exit>>,
    kill: Mutex<Option<oneshot::Sender<()>>>,
}

impl LocalTerminal {
    /// The output so far (scrubbed), whether its start was dropped, and the exit once it
    /// ended.
    pub fn snapshot(&self) -> (String, bool, Option<Exit>) {
        let output = self.output.lock();
        let text = kubyl_resources_core::redact::scrub_text(&output.text).into_owned();
        (text, output.truncated, self.exit.borrow().clone())
    }

    pub fn exit(&self) -> Option<Exit> {
        self.exit.borrow().clone()
    }

    pub async fn wait(&self) -> Exit {
        let mut exit = self.exit.clone();
        loop {
            if let Some(done) = exit.borrow().clone() {
                return done;
            }
            if exit.changed().await.is_err() {
                return exit.borrow().clone().unwrap_or(Exit {
                    code: None,
                    signal: Some("lost".into()),
                });
            }
        }
    }

    pub fn kill(&self) {
        if let Some(kill) = self.kill.lock().take() {
            kill.send(()).ok();
        }
    }

    fn append(&self, chunk: &str) {
        let mut output = self.output.lock();
        output.text.push_str(chunk);
        if output.text.len() > self.limit {
            let mut cut = output.text.len() - self.limit;
            while !output.text.is_char_boundary(cut) {
                cut += 1;
            }
            output.text.drain(..cut);
            output.truncated = true;
        }
    }
}

/// The commands of one agent connection, by terminal id.
#[derive(Default)]
pub struct Terminals {
    map: Mutex<HashMap<String, Arc<LocalTerminal>>>,
    next: AtomicU64,
}

impl Terminals {
    pub fn get(&self, id: &str) -> Option<Arc<LocalTerminal>> {
        self.map.lock().get(id).cloned()
    }

    /// Kills and forgets a terminal (`terminal/release`).
    pub fn release(&self, id: &str) -> bool {
        match self.map.lock().remove(id) {
            Some(terminal) => {
                terminal.kill();
                true
            }
            None => false,
        }
    }

    pub fn kill_all(&self) {
        for (_, terminal) in self.map.lock().drain() {
            terminal.kill();
        }
    }

    /// Starts `launch`. Must run inside a Tokio runtime.
    pub fn spawn(&self, launch: Launch) -> std::io::Result<(String, Arc<LocalTerminal>)> {
        let line = crate::policy::command_line(&launch.command, &launch.args);
        let mut cmd = if cfg!(windows) {
            shell_command(&crate::policy::windows_command_line(
                &launch.command,
                &launch.args,
            ))
        } else {
            shell_command(&line)
        };
        cmd.current_dir(&launch.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = &launch.path {
            cmd.env("PATH", path);
        }
        for (name, value) in &launch.env {
            cmd.env(name, value);
        }
        // Last, so the agent's environment can't point commands at another kubeconfig.
        if let Some(kubeconfig) = &launch.kubeconfig {
            cmd.env("KUBECONFIG", kubeconfig);
        }
        let mut child = cmd.spawn()?;
        let (exit_tx, exit_rx) = watch::channel(None);
        let (kill_tx, kill_rx) = oneshot::channel();
        let terminal = Arc::new(LocalTerminal {
            command_line: line,
            cwd: launch.cwd.clone(),
            output: Mutex::default(),
            limit: launch
                .output_limit
                .map(|l| l.clamp(1024, 16 * 1024 * 1024) as usize)
                .unwrap_or(DEFAULT_LIMIT),
            exit: exit_rx,
            kill: Mutex::new(Some(kill_tx)),
        });
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let readers = [
            stdout.map(|s| tokio::spawn(pump(s, terminal.clone()))),
            stderr.map(|s| tokio::spawn(pump(s, terminal.clone()))),
        ];
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status.ok(),
                _ = kill_rx => {
                    child.kill().await.ok();
                    child.wait().await.ok()
                }
            };
            for reader in readers.into_iter().flatten() {
                reader.await.ok();
            }
            exit_tx.send(Some(exit_of(status))).ok();
        });
        let id = format!("term-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        self.map.lock().insert(id.clone(), terminal.clone());
        Ok((id, terminal))
    }
}

async fn pump(mut stream: impl AsyncRead + Unpin, terminal: Arc<LocalTerminal>) {
    let mut buf = vec![0u8; 8192];
    let mut pending = Vec::new();
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                // Keep an incomplete UTF-8 sequence for the next read.
                let valid = match std::str::from_utf8(&pending) {
                    Ok(_) => pending.len(),
                    Err(err) if err.error_len().is_none() => err.valid_up_to(),
                    Err(_) => pending.len(),
                };
                let text = String::from_utf8_lossy(&pending[..valid]).into_owned();
                pending.drain(..valid);
                terminal.append(&text);
            }
        }
    }
    if !pending.is_empty() {
        terminal.append(&String::from_utf8_lossy(&pending));
    }
}

fn exit_of(status: Option<std::process::ExitStatus>) -> Exit {
    let Some(status) = status else {
        return Exit {
            code: None,
            signal: Some("unknown".into()),
        };
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return Exit {
                code: None,
                signal: Some(format!("signal {signal}")),
            };
        }
    }
    Exit {
        code: status.code().map(|c| c as u32),
        signal: None,
    }
}

fn shell_command(line: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(line);
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(line);
        cmd
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn launch(command: &str, limit: Option<u64>) -> Launch {
        Launch {
            command: command.into(),
            args: Vec::new(),
            env: vec![("GREETING".into(), "hi".into())],
            cwd: std::env::temp_dir(),
            output_limit: limit,
            path: None,
            kubeconfig: Some(PathBuf::from("/nonexistent/kubeconfig")),
        }
    }

    #[tokio::test]
    async fn output_exit_and_environment() {
        let terminals = Terminals::default();
        let (id, terminal) = terminals
            .spawn(launch(
                "echo $GREETING $KUBECONFIG; echo 'Authorization: Bearer abcdefghijkl' >&2; exit 3",
                None,
            ))
            .unwrap();
        let exit = terminal.wait().await;
        assert_eq!(exit.code, Some(3));
        let (text, truncated, _) = terminals.get(&id).unwrap().snapshot();
        assert!(text.contains("hi /nonexistent/kubeconfig"), "{text}");
        assert!(!text.contains("abcdefghijkl"), "{text}");
        assert!(!truncated);
        assert!(terminals.release(&id));
        assert!(terminals.get(&id).is_none());
    }

    #[tokio::test]
    async fn output_keeps_the_end_and_kill_stops() {
        let terminals = Terminals::default();
        let (_, terminal) = terminals
            .spawn(launch(
                "for i in $(seq 1 2000); do echo line-$i; done",
                Some(1024),
            ))
            .unwrap();
        terminal.wait().await;
        let (text, truncated, _) = terminal.snapshot();
        assert!(truncated);
        assert!(text.len() <= 1024);
        assert!(text.ends_with("line-2000\n"));

        let (_, sleeper) = terminals.spawn(launch("sleep 30", None)).unwrap();
        sleeper.kill();
        let exit = tokio::time::timeout(std::time::Duration::from_secs(5), sleeper.wait())
            .await
            .unwrap();
        assert!(exit.code.is_none() || exit.code != Some(0));
    }
}
