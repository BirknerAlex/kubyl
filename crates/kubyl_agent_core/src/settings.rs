//! The `"agent"` section of settings.json, and what state.json keeps about threads (never
//! their transcripts).

use std::collections::BTreeMap;

use kubyl_base::ClusterId;
use kubyl_settings_core::{SettingsSection, StateSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What `kubectl` in the agent's own shell reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KubectlAccess {
    /// Nothing: `KUBECONFIG` points at an empty file, so the agent uses Kubyl's tools.
    #[default]
    None,
    /// A kubeconfig with only the thread's context, when its user has no inline credentials
    /// (exec plugins such as `aws eks get-token`, `kubelogin`); otherwise like `none`.
    Context,
}

/// An ACP agent that isn't in Kubyl's built-in list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CustomAgent {
    /// Short id, e.g. `my-agent`. A built-in id replaces that agent's command.
    pub id: String,
    /// Shown in the agent picker.
    pub name: String,
    /// The program; found through your login shell's `PATH` unless it's a path.
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment for the agent process.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// Agents (phase 21): Claude, Codex, Gemini CLI, Copilot and other ACP agents in Kubyl's agent
/// panel, with Kubyl's read-only cluster tools.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AgentSettings {
    /// The agent new threads use (`claude`, `codex`, `gemini`, `copilot`, `goose`, `opencode`, or
    /// a custom id). Default: the first one installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Agents to add, or built-in agents to start differently.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<CustomAgent>,
    /// What `kubectl` in the agent's own shell reaches: `none` (default) or `context`.
    pub kubectl: KubectlAccess,
    /// The agent's working folder: empty for a scratch folder per thread (removed with the
    /// thread), or a folder such as a GitOps repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The most a Kubyl tool returns to the agent, in KiB (8–1024).
    pub max_tool_output_kib: usize,
    /// Lines a `logs` tool call returns when the agent doesn't say (10–5000).
    pub default_log_lines: usize,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            default: None,
            custom: Vec::new(),
            kubectl: KubectlAccess::None,
            cwd: None,
            max_tool_output_kib: 64,
            default_log_lines: 500,
        }
    }
}

impl SettingsSection for AgentSettings {
    const KEY: Option<&'static str> = Some("agent");
}

impl AgentSettings {
    pub fn max_tool_output(&self) -> usize {
        self.max_tool_output_kib.clamp(8, 1024) * 1024
    }

    pub fn log_lines(&self) -> usize {
        self.default_log_lines.clamp(10, 5000)
    }
}

/// A thread as state.json keeps it: enough to reopen it from the agent's own storage
/// (`session/load`), nothing of its contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedThread {
    pub agent: String,
    pub cluster: ClusterId,
    pub title: String,
    pub cwd: String,
    /// The agent's session id.
    pub session: String,
    /// Unix seconds.
    pub updated: i64,
}

/// What the agent panel remembers between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentState {
    /// The agent picked last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_agent: Option<String>,
    /// Threads of agents that can reopen sessions, newest first (at most 30).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<SavedThread>,
    /// The first-run note was dismissed.
    pub note_dismissed: bool,
}

impl StateSection for AgentState {
    const KEY: &'static str = "agent";
}

impl AgentState {
    pub const MAX_THREADS: usize = 30;

    /// Adds or refreshes `thread` (by agent and session) at the top.
    pub fn remember(&mut self, thread: SavedThread) {
        self.threads
            .retain(|t| !(t.agent == thread.agent && t.session == thread.session));
        self.threads.insert(0, thread);
        self.threads.truncate(Self::MAX_THREADS);
    }

    pub fn forget(&mut self, agent: &str, session: &str) {
        self.threads
            .retain(|t| !(t.agent == agent && t.session == session));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_limits() {
        let settings: AgentSettings =
            serde_json::from_str(r#"{"max_tool_output_kib": 1, "kubectl": "context"}"#).unwrap();
        assert_eq!(settings.kubectl, KubectlAccess::Context);
        assert_eq!(settings.max_tool_output(), 8 * 1024);
        assert_eq!(AgentSettings::default().log_lines(), 500);
    }

    #[test]
    fn remembered_threads_are_unique_and_bounded() {
        let mut state = AgentState::default();
        for i in 0..40 {
            state.remember(SavedThread {
                agent: "claude".into(),
                cluster: ClusterId::new("kind"),
                title: format!("t{i}"),
                cwd: "/tmp".into(),
                session: format!("s{}", i % 35),
                updated: i,
            });
        }
        assert_eq!(state.threads.len(), AgentState::MAX_THREADS);
        assert_eq!(state.threads[0].title, "t39");
        state.forget("claude", "s4");
        assert!(state.threads.iter().all(|t| t.session != "s4"));
    }
}
