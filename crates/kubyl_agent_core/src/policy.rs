//! What the agent may do without asking, and what the prompts warn about.
//!
//! - Shell commands (`terminal/create`) always ask, unless the user just allowed the agent's own
//!   permission prompt for that command ([`ExecuteGrant`]) and nothing in it needs a warning.
//! - File reads inside the thread's folder are allowed; reads outside it and every write ask.
//! - [`command_warnings`]: commands that read kubeconfigs, tokens or Secrets.

use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

/// How long an allowed "execute" permission covers the `terminal/create` that follows it.
pub const GRANT_WINDOW: Duration = Duration::from_secs(30);

struct Warning {
    pattern: Regex,
    text: &'static str,
}

static WARNINGS: LazyLock<Vec<Warning>> = LazyLock::new(|| {
    let warning = |pattern: &str, text: &'static str| Warning {
        pattern: Regex::new(pattern).expect("valid warning pattern"),
        text,
    };
    vec![
        warning(
            r"(?i)\b(?:kubectl|oc|kubecolor)\b[^|;&]*\b(?:get|describe|edit|view-secret)\b[^|;&]*\b(?:secrets?|sa-token)\b",
            "Reads Secrets: their values would go to the agent's provider.",
        ),
        warning(
            r"(?i)\b(?:kubectl|oc)\b[^|;&]*\bconfig\s+view\b[^|;&]*--raw",
            "Prints kubeconfig credentials.",
        ),
        warning(
            r"(?i)(?:\.kube/|kubeconfig|\$KUBECONFIG|%KUBECONFIG%)",
            "Touches a kubeconfig, which can hold credentials.",
        ),
        warning(
            r"(?i)\b(?:kubectl|oc)\b[^|;&]*\bcreate\s+token\b",
            "Creates a service-account token.",
        ),
        warning(
            r"(?i)\b(?:oc\s+whoami\s+(?:-t|--show-token)|gcloud\s+auth\s+print-(?:access|identity)-token|aws\s+eks\s+get-token|az\s+account\s+get-access-token|kubelogin\s+get-token)\b",
            "Prints an access token.",
        ),
        warning(
            r"(?i)(?:\.aws/credentials|\.config/gcloud|\.azure/|\.docker/config\.json|\.netrc|id_rsa|id_ed25519)",
            "Reads a credentials file.",
        ),
        warning(
            r"(?i)\b(?:kubectl|oc|helm)\b[^|;&]*\b(?:apply|create|delete|patch|replace|scale|rollout\s+restart|drain|cordon|annotate|label|set|edit|uninstall|upgrade|install)\b",
            "Changes the cluster outside Kubyl's review.",
        ),
    ]
});

/// What a shell command line deserves a warning for, in the order of [`WARNINGS`].
pub fn command_warnings(command_line: &str) -> Vec<&'static str> {
    WARNINGS
        .iter()
        .filter(|w| w.pattern.is_match(command_line))
        .map(|w| w.text)
        .collect()
}

/// `command` and `args` as one line for prompts and matching (arguments with spaces quoted).
pub fn command_line(command: &str, args: &[String]) -> String {
    let mut line = command.to_string();
    for arg in args {
        line.push(' ');
        if arg.is_empty()
            || arg
                .chars()
                .any(|c| c.is_whitespace() || c == '\'' || c == '"')
        {
            line.push('\'');
            line.push_str(&arg.replace('\'', r"'\''"));
            line.push('\'');
        } else {
            line.push_str(arg);
        }
    }
    line
}

/// The user allowed the agent's own permission prompt for an "execute" tool call.
#[derive(Clone, Debug)]
pub struct ExecuteGrant {
    pub at: Instant,
    /// The command the tool call showed, when it showed one.
    pub command: Option<String>,
}

impl ExecuteGrant {
    /// Whether this grant covers running `command_line` now.
    pub fn covers(&self, command_line: &str, now: Instant) -> bool {
        if now.duration_since(self.at) > GRANT_WINDOW {
            return false;
        }
        match &self.command {
            None => true,
            Some(granted) => {
                let granted = normalize(granted);
                let wanted = normalize(command_line);
                !granted.is_empty() && (wanted.contains(&granted) || granted.contains(&wanted))
            }
        }
    }
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `path` without `.` and `..` (purely lexical; the file need not exist).
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Whether `path` (absolute) is inside `root`. Symlinks are resolved when both exist, so a
/// link inside the folder can't reach outside it.
pub fn inside(root: &Path, path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let root = root.canonicalize().unwrap_or_else(|_| clean(root));
    resolved(&clean(path)).starts_with(&root)
}

/// `path` with its nearest existing ancestor's symlinks resolved (a new file's folders may not
/// exist yet).
fn resolved(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
    let mut out = existing.canonicalize().unwrap_or(existing);
    for name in rest.into_iter().rev() {
        out.push(name);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_commands_get_warnings() {
        assert!(command_warnings("kubectl get pods -n shop").is_empty());
        assert_eq!(
            command_warnings("kubectl -n shop get secret db -o yaml"),
            ["Reads Secrets: their values would go to the agent's provider."]
        );
        assert!(!command_warnings("cat ~/.kube/config").is_empty());
        assert!(!command_warnings("kubectl config view --raw").is_empty());
        assert!(!command_warnings("oc whoami -t").is_empty());
        assert!(!command_warnings("kubectl delete pod web-0").is_empty());
        assert!(!command_warnings("helm upgrade shop ./chart").is_empty());
        assert!(command_warnings("ls -la && git status").is_empty());
    }

    #[test]
    fn grants_cover_the_same_command_shortly_after() {
        let now = Instant::now();
        let grant = ExecuteGrant {
            at: now,
            command: Some("kubectl  get pods".into()),
        };
        assert!(grant.covers("kubectl get pods", now));
        assert!(!grant.covers("rm -rf /", now));
        assert!(!grant.covers(
            "kubectl get pods",
            now + GRANT_WINDOW + Duration::from_secs(1)
        ));
        let any = ExecuteGrant {
            at: now,
            command: None,
        };
        assert!(any.covers("make test", now));
    }

    #[test]
    fn paths_stay_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.yaml"), "").unwrap();
        assert!(inside(root, &root.join("a.yaml")));
        assert!(inside(root, &root.join("new/b.yaml")));
        assert!(!inside(root, &root.join("../outside.yaml")));
        assert!(!inside(root, Path::new("relative.yaml")));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", root.join("link")).unwrap();
            assert!(!inside(root, &root.join("link/hosts")));
        }
    }

    #[test]
    fn command_lines_quote_arguments() {
        assert_eq!(
            command_line("sh", &["-c".into(), "echo 'hi' there".into()]),
            r"sh -c 'echo '\''hi'\'' there'"
        );
    }
}
