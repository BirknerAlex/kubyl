//! Agents without the UI (phase 21).
//!
//! Kubyl's MCP server and tools live in `kubyl_mcp_core`; [`mcp`] and [`tools`] re-export them.

pub use kubyl_mcp_core::{server as mcp, tools};

pub mod acp;
pub mod agents;
pub mod elicitation;
pub mod jsonrpc;
pub mod kubeconfig;

pub mod mentions;
pub mod policy;
pub mod prompts;
pub mod service;
pub mod settings;
pub mod terminal;
pub mod thread;
