//! Kubyl's MCP server in the app: other tools (Claude Code, Cursor, …) get the clusters Kubyl is
//! connected to as MCP tools, on `127.0.0.1` (port 7421 unless settings say otherwise).
//!
//! - [`service`]: [`McpService`], the GPUI entity around `kubyl_mcp_core::hub::McpHub`.
//! - [`context`]: the tool context of a cluster, shared with the agent panel.
//! - [`view`]: the diff of a proposed change and the dialog that asks the user.
//! - A status bar item, and the actions "MCP: Show Server", "MCP: Copy Claude Code Command",
//!   "MCP: Copy Client Config", "MCP: Copy Token" and "MCP: Regenerate Token".
//!
//! Changes from outside clients wait for the user in a dialog; they are off unless
//! `mcp.writes` is `ask`.

pub mod context;
mod dialog;
pub mod service;
mod status;
pub mod view;

use std::collections::HashSet;

use gpui::{App, ClipboardItem, Window, actions};
use kubyl_core::{ActionRegistry, ActionSpec, ChromeRegistry, Notification, NotificationCenter};
use kubyl_mcp_core::settings::McpSettings;
use kubyl_settings::Settings;

pub use kubyl_mcp_core::{approval, hub, settings};
pub use service::McpService;

actions!(
    mcp,
    [
        /// Shows the MCP server's address and how to connect a client.
        ShowServer,
        /// Copies the `claude mcp add` command for Claude Code (it contains the access token).
        CopyClaudeCodeCommand,
        /// Copies a JSON client configuration (it contains the access token).
        CopyClientConfig,
        /// Copies the access token alone (for the Claude Desktop extension, `kubyl.mcpb`).
        CopyToken,
        /// Makes a new access token; clients must be configured again.
        RegenerateToken,
    ]
);

/// Registers settings, the service, the status bar item, actions and the approval dialog.
///
/// Must run after `kubyl_kube::init`, `kubyl_resources::init`, `kubyl_metrics::init` and
/// `kubyl_alerts::init`.
pub fn init(cx: &mut App) {
    init_with(true, cx);
}

/// Like [`init`]; `live` off keeps GPUI tests free of sockets.
pub fn init_with(live: bool, cx: &mut App) {
    Settings::register::<McpSettings>(cx);
    let service = McpService::install(live, cx);
    ChromeRegistry::add_status_item(cx, status::McpStatusItem);

    ActionRegistry::register(cx, ActionSpec::new("MCP: Show Server", ShowServer));
    ActionRegistry::register(
        cx,
        ActionSpec::new("MCP: Copy Claude Code Command", CopyClaudeCodeCommand),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("MCP: Copy Client Config (JSON)", CopyClientConfig),
    );
    ActionRegistry::register(cx, ActionSpec::new("MCP: Copy Token", CopyToken));
    ActionRegistry::register(
        cx,
        ActionSpec::new("MCP: Regenerate Token", RegenerateToken),
    );

    // The server's dialog even while a change waits (the status bar item shows that first).
    cx.on_action(|_: &ShowServer, cx| {
        with_window(cx, dialog::open);
    });
    cx.on_action(|_: &CopyClaudeCodeCommand, cx| copy(cx, McpService::claude_code_command));
    cx.on_action(|_: &CopyClientConfig, cx| copy(cx, McpService::json_config));
    cx.on_action(|_: &CopyToken, cx| copy(cx, McpService::token_text));
    cx.on_action(|_: &RegenerateToken, cx| {
        if let Some(service) = McpService::global(cx) {
            if matches!(service.read(cx).state(), hub::ServerState::Standby { .. }) {
                NotificationCenter::push(
                    cx,
                    Notification::warning(
                        "Another Kubyl window serves the MCP server. Make the new token there.",
                    ),
                );
                return;
            }
            let off = matches!(service.read(cx).state(), hub::ServerState::Off);
            // While off this only marks the token for replacement at the next start.
            service.update(cx, |service, cx| service.regenerate_token(cx));
            NotificationCenter::push(
                cx,
                Notification::info(if off {
                    "The MCP server is off, so the token stays the same for now. It is replaced when the server is turned on."
                } else {
                    "Replacing the MCP token. When it is done, copy the command again; if it fails, MCP: Show Server says why."
                }),
            );
        }
    });

    // Ask about every change an outside client proposes, once.
    let mut asked = HashSet::new();
    cx.observe(&service, move |service, cx| {
        // Without a window there is nothing to show it in; the service notifies again every few
        // seconds while a change waits, so it is asked once a window exists.
        if cx.windows().is_empty() {
            return;
        }
        let waiting: Vec<_> = service
            .read(cx)
            .pending()
            .iter()
            .filter(|p| asked.insert(p.id))
            .map(|p| (p.id, p.proposal.clone()))
            .collect();
        for (id, proposal) in waiting {
            with_window(cx, move |window, cx| ask(id, proposal, window, cx));
        }
    })
    .detach();
}

/// Runs `f` in the focused window, after the dispatching update.
pub fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

fn ask(id: u64, proposal: approval::Proposal, window: &mut Window, cx: &mut App) {
    view::ask(
        proposal,
        "An MCP client",
        move |decision, _, cx| {
            if let Some(service) = McpService::global(cx) {
                service.update(cx, |service, cx| service.answer(id, decision, cx));
            }
        },
        window,
        cx,
    );
}

pub(crate) fn copy(cx: &mut App, text: fn(&McpService) -> Option<String>) {
    let Some(service) = McpService::global(cx) else {
        return;
    };
    let Some(text) = text(service.read(cx)) else {
        NotificationCenter::push(cx, Notification::warning("The MCP server isn't running."));
        return;
    };
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    NotificationCenter::push(
        cx,
        Notification::success("Copied. It contains the MCP access token; don't share it."),
    );
}

impl McpService {
    pub(crate) fn claude_code_command(&self) -> Option<String> {
        self.core().claude_code_command()
    }

    pub(crate) fn json_config(&self) -> Option<String> {
        self.core().json_config()
    }

    pub(crate) fn token_text(&self) -> Option<String> {
        self.core().token().map(str::to_string)
    }

    fn core(&self) -> &hub::McpHub {
        self
    }
}

/// The status bar item's click: the waiting change if there is one, else the server's details.
pub(crate) fn open_status(window: &mut Window, cx: &mut App) {
    let waiting = McpService::global(cx).and_then(|service| {
        service
            .read(cx)
            .pending()
            .first()
            .map(|p| (p.id, p.proposal.clone()))
    });
    match waiting {
        Some((id, proposal)) => ask(id, proposal, window, cx),
        None => dialog::open(window, cx),
    }
}
