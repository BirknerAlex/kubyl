//! Agents over ACP (phase 21, board 19): the user's own coding agent (Claude, Codex, Gemini
//! CLI, Copilot, …) in a right-dock panel, reading one cluster through Kubyl's read-only tools.
//!
//! - [`service`]: [`AgentService`], the app's `AgentCore` (agents, threads, decisions) with the
//!   per-cluster tool contexts kept current.
//! - [`panel`]: the Agent panel (threads, tool cards, permission prompts, composer).
//! - Actions: "View: Agent Panel" (`secondary-?`), "Agent: New Thread", "Resource: Ask Agent"
//!   (`shift-a` in lists), `kubyl::AskAgent` from other crates (log lines), "Agent: Show Output".
//! - The status bar item while an agent works or waits for the user.
//!
//! `kubyl mcp-bridge` (see [`bridge`]) is the stdio side of Kubyl's MCP server for agents that
//! can't reach it over HTTP; the binary checks for it before starting the UI.

pub mod panel;
pub mod service;
mod status;

pub use kubyl_agent_core::{acp, agents, mcp, policy, settings, thread, tools};

use gpui::{App, KeyBinding, Window, actions, div, prelude::*, px};
use gpui_component::WindowExt as _;
use kubyl_agent_core::thread::ContextChip;
use kubyl_core::actions::AskAgent;
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, ChromeRegistry, ClusterCaps, ResourceRef,
};
use kubyl_resources::ResourceSelection;
use kubyl_settings::Settings;
use kubyl_ui::{ActiveColors as _, Button, h_flex, u, v_flex};

use service::{AgentEvent, AgentService};
use settings::AgentSettings;

/// The argument that makes the Kubyl binary run [`bridge`] instead of the app.
pub const BRIDGE_ARG: &str = mcp::BRIDGE_ARG;

/// Runs `kubyl mcp-bridge` (no window); returns the exit code.
pub fn bridge() -> i32 {
    mcp::bridge()
}

actions!(
    agent,
    [
        /// Shows the Agent panel.
        TogglePanel,
        /// Starts a new agent thread about the active cluster.
        NewThread,
        /// Attaches the selected object to the agent's next prompt.
        AskAboutSelection,
        /// Shows what the current thread's agent printed (stderr).
        ShowOutput,
    ]
);

const RESOURCE_LIST: &str = "ResourceList";

/// Registers settings, the service, the panel, actions and the status bar item.
///
/// Must run after `kubyl_kube::init`, `kubyl_resources::init`, `kubyl_metrics::init` and
/// `kubyl_alerts::init`.
pub fn init(cx: &mut App) {
    init_with(true, cx);
}

/// Like [`init`]; `live` off keeps GPUI tests free of background lookups.
pub fn init_with(live: bool, cx: &mut App) {
    Settings::register::<AgentSettings>(cx);
    let service = AgentService::install(live, cx);
    cx.subscribe(&service, |_, event: &AgentEvent, cx| {
        if let AgentEvent::TurnEnded { title, .. } = event {
            service::notify_turn(title, cx);
        }
    })
    .detach();
    ChromeRegistry::add_dock_panel(cx, panel::AgentDockPanel);
    ChromeRegistry::add_status_item(cx, status::AgentStatusItem);

    ActionRegistry::register(
        cx,
        ActionSpec::new("View: Agent Panel", TogglePanel).bind("secondary-?", None),
    );
    ActionRegistry::register(cx, ActionSpec::new("Agent: New Thread", NewThread));
    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Ask Agent", AskAboutSelection)
            .hint("Ask")
            .bind("shift-a", Some(RESOURCE_LIST))
            .available_when(|target: &ResourceRef, _: &ClusterCaps| target.is_object()),
    );
    ActionRegistry::register(cx, ActionSpec::new("Agent: Show Agent Output", ShowOutput));
    cx.bind_keys([KeyBinding::new("secondary-?", TogglePanel, None)]);

    cx.on_action(|_: &TogglePanel, cx| {
        with_window(cx, |window, cx| {
            panel::show(window, cx);
        })
    });
    cx.on_action(|_: &NewThread, cx| {
        with_window(cx, |window, cx| {
            if let Some(panel) = panel::show(window, cx) {
                let cluster = ActiveContext::global(cx)
                    .cluster
                    .as_ref()
                    .map(|c| c.id.clone());
                panel.update(cx, |panel, cx| panel.new_thread(cluster, cx));
            }
        })
    });
    cx.on_action(|_: &AskAboutSelection, cx| {
        let Some((cluster, chip)) = selection_chip(cx) else {
            return;
        };
        attach(cluster, chip, cx);
    });
    cx.on_action(|ask: &AskAgent, cx| {
        let chip = ContextChip {
            label: ask.label.clone(),
            uri: ask.uri.clone(),
            text: kubyl_resources::redact::scrub_text(&ask.text).into_owned(),
        };
        attach(ask.cluster.clone(), chip, cx);
    });
    cx.on_action(|_: &ShowOutput, cx| {
        let agent = AgentService::global(cx).and_then(|service| {
            let service = service.read(cx);
            service
                .threads()
                .last()
                .map(|t| t.agent.clone())
                .or_else(|| {
                    service
                        .agents()
                        .iter()
                        .find(|a| a.running)
                        .map(|a| a.spec.id.clone())
                })
        });
        if let Some(agent) = agent {
            with_window(cx, move |window, cx| show_output(&agent, window, cx));
        }
    });
}

/// Runs `f` in the focused window, after the dispatching window's update (global action
/// handlers run inside it).
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window.update(cx, |_, window, cx| f(window, cx)).ok();
        }
    });
}

/// Opens the panel with `chip` attached for `cluster`.
fn attach(cluster: kubyl_core::ClusterId, chip: ContextChip, cx: &mut App) {
    with_window(cx, move |window, cx| {
        if let Some(panel) = panel::show(window, cx) {
            panel.update(cx, |panel, cx| panel.attach(cluster, chip, cx));
        }
    });
}

/// The selected object as an attachment: `kubectl describe`-like text without events, masked.
fn selection_chip(cx: &App) -> Option<(kubyl_core::ClusterId, ContextChip)> {
    let selected = ResourceSelection::global(cx).primary()?;
    let chip = object_chip(
        &selected.kind,
        &selected.target,
        selected.object.as_deref(),
        "selected in Kubyl",
    )?;
    Some((selected.target.cluster.clone(), chip))
}

/// An object as an attachment: `kubectl describe`-like text without events, masked. `how`
/// says how it got there (`selected in Kubyl`, `mentioned`).
pub(crate) fn object_chip(
    kind: &str,
    target: &ResourceRef,
    object: Option<&serde_json::Value>,
    how: &str,
) -> Option<ContextChip> {
    let name = target.name.clone()?;
    let path = match &target.namespace {
        Some(ns) => format!("{ns}/{name}"),
        None => name.clone(),
    };
    let label = format!("{kind} {path}");
    let uri = format!(
        "kubyl://{}/{}/{}",
        target.cluster, target.gvr.resource, path
    );
    let text = match object {
        Some(value) => {
            let mut value = value.clone();
            if value
                .get("kind")
                .is_none_or(|k| k == "PartialObjectMetadata")
            {
                value["kind"] = serde_json::Value::String(kind.to_string());
                value["apiVersion"] = serde_json::Value::String(target.gvr.api_version());
            }
            if kubyl_resources::redact::is_helm_release(&value) {
                format!("{label} holds a Helm release; Kubyl doesn't share its contents.")
            } else {
                kubyl_resources::redact::mask_object(&mut value);
                format!(
                    "{label} ({how}):\n{}",
                    kubyl_resources::describe::describe_as(
                        &target.gvr.group,
                        kind,
                        &value,
                        &[],
                        jiff::Timestamp::now()
                    )
                )
            }
        }
        None => format!("{label} ({how})."),
    };
    Some(ContextChip { label, uri, text })
}

/// Shows the agent's stderr (scrubbed) in a dialog.
pub(crate) fn show_output(agent: &str, window: &mut Window, cx: &mut App) {
    let output = AgentService::global(cx)
        .and_then(|s| s.read(cx).agent_output(agent))
        .unwrap_or_else(|| "The agent isn't running.".into());
    let output = if output.trim().is_empty() {
        "The agent printed nothing.".to_string()
    } else {
        output
    };
    let colors = cx.colors().clone();
    let title = AgentService::global(cx)
        .and_then(|s| s.read(cx).agent(agent).map(|a| a.spec.name.clone()))
        .unwrap_or_else(|| agent.to_string());
    window.open_dialog(cx, move |dialog, _, _| {
        let output = output.clone();
        let title = title.clone();
        dialog
            .w(px(640.0))
            .margin_top(px(80.0))
            .bg(colors.panel)
            .close_button(true)
            .content(move |content, _, _| {
                let copy = output.clone();
                content.child(
                    v_flex()
                        .gap(u(10.0))
                        .child(div().text_size(u(14.0)).child(format!("{title}: output")))
                        .child(
                            div()
                                .id("agent-output-text")
                                .max_h(px(420.0))
                                .overflow_y_scroll()
                                .p(u(8.0))
                                .rounded(u(4.0))
                                .bg(colors.background)
                                .font_family(kubyl_ui::fonts::MONO)
                                .text_size(u(11.5))
                                .children(
                                    output
                                        .lines()
                                        .map(|l| div().child(l.to_string()))
                                        .collect::<Vec<_>>(),
                                ),
                        )
                        .child(h_flex().gap(u(6.0)).child(
                            Button::new("agent-output-copy").label("Copy").on_click(
                                move |_, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        copy.clone(),
                                    ))
                                },
                            ),
                        )),
                )
            })
    });
}
