//! Which agents Kubyl knows and where they're installed.
//!
//! The built-in list is data only: launch commands come from the ACP registry
//! (`cdn.agentclientprotocol.com/registry/v1/latest/registry.json`), checked 2026-10-07. Kubyl
//! never installs or downloads an agent; a missing one shows its install command.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::settings::{AgentSettings, CustomAgent};

/// How to start one agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    /// `claude`, `codex`, … or a custom id.
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// A command to copy when it isn't installed.
    pub install: Option<String>,
    /// How to sign in when the agent says it needs that.
    pub sign_in: Option<String>,
    /// From settings.
    pub custom: bool,
}

fn builtin(
    id: &str,
    name: &str,
    command: &str,
    args: &[&str],
    install: &str,
    sign_in: &str,
) -> AgentSpec {
    AgentSpec {
        id: id.into(),
        name: name.into(),
        command: command.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        env: Vec::new(),
        install: Some(install.into()),
        sign_in: Some(sign_in.into()),
        custom: false,
    }
}

/// The agents Kubyl knows without settings, in picker order.
pub fn builtins() -> Vec<AgentSpec> {
    vec![
        builtin(
            "claude",
            "Claude",
            "claude-agent-acp",
            &[],
            "npm install -g @agentclientprotocol/claude-agent-acp",
            "Run `claude` in a terminal once and sign in (Claude Code), then try again.",
        ),
        builtin(
            "codex",
            "Codex",
            "codex-acp",
            &[],
            "npm install -g @agentclientprotocol/codex-acp",
            "Run `codex login` in a terminal, then try again.",
        ),
        builtin(
            "gemini",
            "Gemini CLI",
            "gemini",
            &["--acp"],
            "npm install -g @google/gemini-cli",
            "Run `gemini` in a terminal once and sign in, then try again.",
        ),
        builtin(
            "copilot",
            "GitHub Copilot",
            "copilot",
            &["--acp"],
            "npm install -g @github/copilot",
            "Run `copilot` in a terminal and `/login`, then try again.",
        ),
        builtin(
            "goose",
            "goose",
            "goose",
            &["acp"],
            "See block.github.io/goose for install options",
            "Run `goose configure` in a terminal, then try again.",
        ),
        builtin(
            "opencode",
            "OpenCode",
            "opencode",
            &["acp"],
            "See opencode.ai for install options",
            "Run `opencode auth login` in a terminal, then try again.",
        ),
    ]
}

/// The built-in agents with settings applied: a custom entry with a built-in id replaces its
/// command; other custom entries come last.
pub fn configured(settings: &AgentSettings) -> Vec<AgentSpec> {
    let mut specs = builtins();
    for custom in &settings.custom {
        if custom.id.trim().is_empty() || custom.command.trim().is_empty() {
            continue;
        }
        let spec = from_custom(custom);
        match specs.iter_mut().find(|s| s.id == spec.id) {
            Some(existing) => {
                existing.command = spec.command;
                existing.args = spec.args;
                existing.env = spec.env;
                if !custom.name.trim().is_empty() {
                    existing.name = spec.name;
                }
                existing.custom = true;
            }
            None => specs.push(spec),
        }
    }
    specs
}

fn from_custom(custom: &CustomAgent) -> AgentSpec {
    AgentSpec {
        id: custom.id.trim().to_string(),
        name: if custom.name.trim().is_empty() {
            custom.id.trim().to_string()
        } else {
            custom.name.trim().to_string()
        },
        command: custom.command.trim().to_string(),
        args: custom.args.clone(),
        env: custom
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        install: None,
        sign_in: None,
        custom: true,
    }
}

/// Where `command` is: itself when it's a path that exists, else the first match in `path`
/// (on Windows also `.exe`, `.cmd` and `.bat`).
pub fn resolve(command: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let as_path = Path::new(command);
    if command.contains('/') || command.contains('\\') {
        let expanded = expand_home(command);
        return expanded.is_file().then_some(expanded);
    }
    let path: OsString = path
        .map(OsStr::to_os_string)
        .or_else(|| std::env::var_os("PATH"))?;
    for dir in std::env::split_paths(&path) {
        if cfg!(windows) && as_path.extension().is_none() {
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

fn expand_home(command: &str) -> PathBuf {
    match command.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(command)),
        None => PathBuf::from(command),
    }
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

/// A Tokio command for a resolved program. Windows `.cmd`/`.bat` shims (npm installs agents as
/// those) run through `cmd /C`; no console window opens.
pub fn command(program: &Path) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let shim = program
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
        let mut cmd = if shim {
            let mut cmd = tokio::process::Command::new("cmd");
            cmd.arg("/C").arg(program);
            cmd
        } else {
            tokio::process::Command::new(program)
        };
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        tokio::process::Command::new(program)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_agents_replace_or_extend_the_builtins() {
        let settings: AgentSettings = serde_json::from_str(
            r#"{"custom": [
                {"id": "claude", "command": "/opt/claude-acp", "args": ["--debug"]},
                {"id": "mine", "name": "Mine", "command": "mine-acp"},
                {"id": "", "command": "ignored"}
            ]}"#,
        )
        .unwrap();
        let specs = configured(&settings);
        let claude = specs.iter().find(|s| s.id == "claude").unwrap();
        assert_eq!(claude.command, "/opt/claude-acp");
        assert_eq!(claude.args, ["--debug"]);
        assert_eq!(claude.name, "Claude");
        assert!(claude.install.is_some());
        assert_eq!(specs.last().unwrap().id, "mine");
        assert_eq!(specs.len(), builtins().len() + 1);
    }

    #[test]
    fn programs_are_found_in_the_given_path() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "fake-agent.cmd"
        } else {
            "fake-agent"
        };
        let program = dir.path().join(name);
        std::fs::write(&program, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(resolve("fake-agent", Some(&path)), Some(program.clone()));
        assert_eq!(resolve("missing-agent", Some(&path)), None);
        assert_eq!(
            resolve(program.to_str().unwrap(), None),
            Some(program.clone())
        );
    }
}
