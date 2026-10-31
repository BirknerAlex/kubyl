//! Kubyl's MCP server without the UI.
//!
//! - [`server`]: the loopback HTTP server (MCP's streamable HTTP, JSON answers), its bearer
//!   token and the `kubyl mcp-bridge` stdio relay.
//! - [`tools`]: the read tools. [`changes`]: the change tools, which only propose; the user
//!   approves through an [`approval::Approver`].
//! - [`handler`]: [`handler::ToolsHandler`] puts the tools behind a server, for one cluster (an
//!   agent thread) or for all of them (outside clients).
//! - [`hub`]: [`hub::McpHub`], the server outside clients such as Claude Code connect to
//!   (on by default, port 7421), as a service on `kubyl_base::Host`.
//! - [`settings`]: the `"mcp"` section of settings.json.
//!
//! Nothing here depends on GPUI; `kubyl_mcp` wires it into the app.

pub mod approval;
pub mod changes;
pub mod handler;
pub mod hub;
pub mod rpc;
pub mod server;
pub mod settings;
pub mod tools;
