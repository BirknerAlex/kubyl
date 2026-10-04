//! The login shell's `PATH` on macOS and Linux.
//!
//! Apps started from Finder or a desktop entry get a minimal `PATH` (`/usr/bin:/bin:…`), so
//! exec plugins installed by Homebrew, asdf or the cloud SDKs aren't found. We ask the user's
//! login shell once and use its `PATH` for exec plugins.

use std::ffi::OsString;
use std::sync::OnceLock;

static SHELL_PATH: OnceLock<Option<OsString>> = OnceLock::new();

#[cfg(unix)]
const MARKER: &str = "__KUBYL_PATH__";

/// Resolves the login-shell `PATH` in the background. Call once at startup.
pub fn prefetch() {
    if SHELL_PATH.get().is_some() {
        return;
    }
    std::thread::Builder::new()
        .name("kubyl-shell-env".into())
        .spawn(|| {
            path();
        })
        .ok();
}

/// The `PATH` for exec plugins: the login shell's merged with the process `PATH`.
pub fn path() -> Option<OsString> {
    SHELL_PATH
        .get_or_init(|| {
            let shell = login_shell_path();
            merge(shell, std::env::var_os("PATH"))
        })
        .clone()
}

fn merge(shell: Option<OsString>, process: Option<OsString>) -> Option<OsString> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    for value in [shell, process].into_iter().flatten() {
        for path in std::env::split_paths(&value) {
            if !path.as_os_str().is_empty() && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).ok()
}

#[cfg(unix)]
fn login_shell_path() -> Option<OsString> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    // `-l` loads the login profile; `-i` would also load rc files but can hang on prompts.
    let mut child = Command::new(&shell)
        .args([
            "-l",
            "-c",
            &format!("printf '{MARKER}%s{MARKER}' \"$PATH\""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .inspect_err(|err| tracing::warn!("couldn't start the login shell {shell}: {err}"))
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > Duration::from_secs(5) => {
                tracing::warn!("the login shell didn't print PATH within 5s");
                child.kill().ok();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    let path = output.split(MARKER).nth(1)?;
    (!path.is_empty()).then(|| OsString::from(path))
}

#[cfg(not(unix))]
fn login_shell_path() -> Option<OsString> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_order_and_drops_duplicates() {
        let shell = std::env::join_paths(["/opt/homebrew/bin", "/usr/bin"]).unwrap();
        let process = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let merged = merge(Some(shell), Some(process)).unwrap();
        let parts: Vec<_> = std::env::split_paths(&merged).collect();
        assert_eq!(
            parts,
            ["/opt/homebrew/bin", "/usr/bin", "/bin"]
                .iter()
                .map(std::path::PathBuf::from)
                .collect::<Vec<_>>()
        );
    }

    #[cfg(unix)]
    #[test]
    fn reads_the_login_shell_path() {
        assert!(login_shell_path().is_some_and(|p| !p.is_empty()));
    }
}
