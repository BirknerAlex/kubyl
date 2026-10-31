//! Kubyl's MCP server in the app: other tools (Claude Code, Cursor, …) get the clusters Kubyl is
//! connected to as MCP tools, on `127.0.0.1` (port 7421 unless settings say otherwise).
//!
//! - [`service`]: [`McpService`], the GPUI entity around `kubyl_mcp_core::hub::McpHub`.
//! - [`context`]: the tool context of a cluster, shared with the agent panel.
//! - [`view`]: the diff of a proposed change and the dialog that asks the user.
//! - A status bar item, and the actions "MCP: Show Server", "MCP: Copy Claude Code Command",
//!   "MCP: Copy Client Config" and "MCP: Regenerate Token".
//!
//! Changes from outside clients wait for the user in a dialog; they are off unless
//! `mcp.writes` is `ask`.

pub mod context;
pub mod service;
mod status;
pub mod view;

use std::collections::HashSet;

use gpui::{App, ClipboardItem, Window, actions, div, prelude::*, px};
use gpui_component::WindowExt as _;
use kubyl_core::{ActionRegistry, ActionSpec, ChromeRegistry, Notification, NotificationCenter};
use kubyl_mcp_core::hub::ServerState;
use kubyl_mcp_core::settings::McpSettings;
use kubyl_settings::Settings;
use kubyl_ui::{ActiveColors as _, Button, h_flex, u, v_flex};

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
    ActionRegistry::register(
        cx,
        ActionSpec::new("MCP: Regenerate Token", RegenerateToken),
    );

    cx.on_action(|_: &ShowServer, cx| {
        with_window(cx, open_status);
    });
    cx.on_action(|_: &CopyClaudeCodeCommand, cx| copy(cx, McpService::claude_code_command));
    cx.on_action(|_: &CopyClientConfig, cx| copy(cx, McpService::json_config));
    cx.on_action(|_: &RegenerateToken, cx| {
        if let Some(service) = McpService::global(cx) {
            service.update(cx, |service, cx| service.regenerate_token(cx));
            NotificationCenter::push(
                cx,
                Notification::info(
                    "New MCP token made. Clients need the new one: copy the command again.",
                ),
            );
        }
    });

    // Ask about every change an outside client proposes, once.
    let mut asked = HashSet::new();
    cx.observe(&service, move |service, cx| {
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

fn copy(cx: &mut App, text: fn(&McpService) -> Option<String>) {
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
    fn claude_code_command(&self) -> Option<String> {
        self.core().claude_code_command()
    }

    fn json_config(&self) -> Option<String> {
        self.core().json_config()
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
        None => show_status(window, cx),
    }
}

fn show_status(window: &mut Window, cx: &mut App) {
    let colors = cx.colors().clone();
    let Some(service) = McpService::global(cx) else {
        return;
    };
    let (state, url, settings) = {
        let service = service.read(cx);
        (
            service.state().clone(),
            service.url(),
            service.settings().clone(),
        )
    };
    let headline = match &state {
        ServerState::Off => {
            "The MCP server is off. Set \"enabled\": true under \"mcp\" in settings.json."
                .to_string()
        }
        ServerState::Starting => "Starting…".to_string(),
        ServerState::Running { .. } => format!(
            "Running at {}. Clients on this computer can read {} through your accounts.",
            url.clone().unwrap_or_default(),
            "every cluster Kubyl is connected to"
        ),
        ServerState::Failed(message) => message.clone(),
    };
    let writes = match settings.writes {
        settings::ExternalWrites::Off => {
            "Read only. Set \"writes\": \"ask\" under \"mcp\" to let clients propose changes; you approve every one here."
        }
        settings::ExternalWrites::Ask => {
            "Clients can propose changes. Each one opens a dialog and waits for you."
        }
    };
    let running = matches!(state, ServerState::Running { .. });
    window.open_dialog(cx, move |dialog, _, _| {
        let headline = headline.clone();
        dialog
            .w(px(560.0))
            .margin_top(px(80.0))
            .bg(colors.panel)
            .close_button(true)
            .content({
                let colors = colors.clone();
                move |content, _, _| {
                    content.child(
                        v_flex()
                            .gap(u(10.0))
                            .child(div().text_size(u(14.0)).child("Kubyl as an MCP server"))
                            .child(div().text_size(u(12.5)).text_color(colors.text_muted).child(headline.clone()))
                            .child(div().text_size(u(12.0)).text_color(colors.text_dim).child(writes))
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child("Every request needs the access token, and the server only listens on this computer. The port and the switch are in settings.json under \"mcp\"."),
                            )
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap(u(6.0))
                                    .child(
                                        Button::new("mcp-copy-claude")
                                            .label("Copy Claude Code command")
                                            .primary()
                                            .disabled(!running)
                                            .on_click(|_, _, cx| copy(cx, McpService::claude_code_command)),
                                    )
                                    .child(
                                        Button::new("mcp-copy-json")
                                            .label("Copy JSON config")
                                            .disabled(!running)
                                            .on_click(|_, _, cx| copy(cx, McpService::json_config)),
                                    )
                                    .child(
                                        Button::new("mcp-regenerate")
                                            .label("New token")
                                            .ghost()
                                            .disabled(!running)
                                            .on_click(|_, window, cx| {
                                                window.close_dialog(cx);
                                                cx.dispatch_action(&RegenerateToken);
                                            }),
                                    ),
                            ),
                    )
                }
            })
    });
}
