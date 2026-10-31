//! The `"mcp"` section of settings.json: Kubyl as an MCP server for other tools.

use kubyl_settings_core::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The port Kubyl's MCP server listens on unless settings say otherwise.
pub const DEFAULT_PORT: u16 = 7421;

/// What an outside MCP client (Claude Code, Cursor, …) may do through Kubyl.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExternalWrites {
    /// Read only: the change tools refuse every call.
    #[default]
    Off,
    /// The change tools work, and every change waits for your approval in Kubyl's window.
    Ask,
}

/// Kubyl's MCP server (on 127.0.0.1 only): gives tools such as Claude Code access to the
/// clusters Kubyl is connected to, with Kubyl's masking and your own RBAC.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct McpSettings {
    /// Run the server. On by default; it only listens on the loopback interface and every
    /// request needs the bearer token.
    pub enabled: bool,
    /// The TCP port (1024–65535). If another program holds it, Kubyl says so instead of
    /// picking another one, because clients are configured with this port.
    pub port: u16,
    /// Whether outside clients may propose changes (they are always approved in Kubyl).
    pub writes: ExternalWrites,
    /// The most a tool returns, in KiB (8–1024).
    pub max_tool_output_kib: usize,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_PORT,
            writes: ExternalWrites::Off,
            max_tool_output_kib: 64,
        }
    }
}

impl SettingsSection for McpSettings {
    const KEY: Option<&'static str> = Some("mcp");
}

impl McpSettings {
    /// The port, kept out of the privileged range.
    pub fn port(&self) -> u16 {
        self.port.max(1024)
    }

    pub fn max_tool_output(&self) -> usize {
        self.max_tool_output_kib.clamp(8, 1024) * 1024
    }

    /// `http://127.0.0.1:<port>/mcp`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_by_default_on_the_standard_port_and_read_only() {
        let settings = McpSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.port(), 7421);
        assert_eq!(settings.writes, ExternalWrites::Off);
        assert_eq!(settings.url(), "http://127.0.0.1:7421/mcp");
    }

    #[test]
    fn settings_override_and_stay_in_range() {
        let settings: McpSettings =
            serde_json::from_str(r#"{"port": 80, "writes": "ask", "enabled": false}"#).unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.port(), 1024);
        assert_eq!(settings.writes, ExternalWrites::Ask);
        assert_eq!(settings.max_tool_output(), 64 * 1024);
    }
}
