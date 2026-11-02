//! "MCP: Show Server": the server's state, its switches and port (saved to settings.json) and
//! the buttons that copy what a client needs.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla, IntoElement,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::switch::Switch;
use kubyl_core::Tone;
use kubyl_mcp_core::hub::ServerState;
use kubyl_mcp_core::settings::{ExternalWrites, McpSettings};
use kubyl_settings::Settings;
use kubyl_ui::{
    ActiveColors as _, Button, Colors, IconButton, IconName, StatusPill, fonts, h_flex, u, v_flex,
};

use crate::service::McpService;
use crate::{RegenerateToken, copy};

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    if McpService::global(cx).is_none() {
        return;
    }
    let colors = cx.colors().clone();
    let view = cx.new(|cx| ServerDialog::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(520.0))
            .margin_top(px(80.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
}

struct ServerDialog {
    port: Entity<InputState>,
    /// The port settings.json had when the field was last filled from it.
    saved_port: u16,
    error: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ServerDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved_port = Settings::get::<McpSettings>(cx).port();
        let port = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(saved_port.to_string(), window, cx);
            state
        });
        let mut subscriptions =
            vec![
                cx.subscribe_in(
                    &port,
                    window,
                    |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.save_port(window, cx),
                        InputEvent::Change => {
                            this.error = None;
                            cx.notify();
                        }
                        _ => {}
                    },
                ),
            ];
        if let Some(service) = McpService::global(cx) {
            subscriptions.push(
                cx.observe_in(&service, window, |this, service, window, cx| {
                    // settings.json edited by hand: show its port unless the user is typing one.
                    let port = service.read(cx).settings().port();
                    if port != this.saved_port {
                        let untouched = this.typed_port(cx) == this.saved_port.to_string();
                        this.saved_port = port;
                        if untouched || !this.port.focus_handle(cx).is_focused(window) {
                            this.port.update(cx, |state, cx| {
                                state.set_value(port.to_string(), window, cx)
                            });
                        }
                    }
                    cx.notify();
                }),
            );
        }
        Self {
            port,
            saved_port,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn typed_port(&self, cx: &App) -> String {
        self.port.read(cx).value().trim().to_string()
    }

    fn dirty(&self, cx: &App) -> bool {
        self.typed_port(cx) != self.saved_port.to_string()
    }

    fn save_port(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        match self.typed_port(cx).parse::<u16>() {
            Ok(port) if port >= 1024 => {
                self.error = None;
                if port != self.saved_port {
                    Settings::update::<McpSettings>(cx, |s| s.port = port);
                }
            }
            _ => self.error = Some("The port must be 1024–65535.".into()),
        }
        cx.notify();
    }
}

impl Focusable for ServerDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ServerDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let Some(service) = McpService::global(cx) else {
            return div().into_any_element();
        };
        let (state, settings) = {
            let service = service.read(cx);
            (service.state().clone(), service.settings().clone())
        };
        let running = matches!(state, ServerState::Running { .. });
        // On standby the other instance serves the same URL and token, so copying still works.
        let serving = running || matches!(state, ServerState::Standby { .. });
        let (status, tone) = match &state {
            ServerState::Off => ("Off", Tone::Muted),
            ServerState::Starting => ("Starting", Tone::Info),
            ServerState::Running { .. } => ("Running", Tone::Good),
            ServerState::Standby { .. } => ("Standby", Tone::Muted),
            ServerState::Failed(_) => ("Failed", Tone::Bad),
        };
        // (text, color, monospace)
        let address: Option<(SharedString, Hsla, bool)> = match &state {
            ServerState::Running { port } => Some((
                format!("http://127.0.0.1:{port}/mcp").into(),
                colors.text_muted,
                true,
            )),
            ServerState::Standby { port } => Some((
                format!("Another Kubyl window serves http://127.0.0.1:{port}/mcp. This one takes over when it closes.").into(),
                colors.text_muted,
                false,
            )),
            ServerState::Failed(message) => Some((message.clone().into(), colors.red, false)),
            ServerState::Off | ServerState::Starting => None,
        };
        let dirty = self.dirty(cx);

        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                h_flex()
                    .gap(u(10.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("MCP server"),
                    )
                    .child(div().text_size(u(12.0)).child(StatusPill::new(status, tone)))
                    .child(
                        IconButton::new("mcp-close", IconName::X)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
            .child(
                v_flex()
                    .px(u(16.0))
                    .pb(u(8.0))
                    .child(
                        row(
                            "Run the server",
                            "Clients on this computer reach every connected cluster through your accounts, with the access token.",
                            &colors,
                        )
                        .child(Switch::new("mcp-enabled").checked(settings.enabled).on_click(
                            |checked: &bool, _, cx| {
                                let checked = *checked;
                                Settings::update::<McpSettings>(cx, |s| s.enabled = checked);
                            },
                        )),
                    )
                    .child(
                        row(
                            "Allow changes",
                            "Clients may propose changes; each one opens a dialog here and waits for you.",
                            &colors,
                        )
                        .child(
                            Switch::new("mcp-writes")
                                .checked(settings.writes == ExternalWrites::Ask)
                                .on_click(|checked: &bool, _, cx| {
                                    let writes = if *checked {
                                        ExternalWrites::Ask
                                    } else {
                                        ExternalWrites::Off
                                    };
                                    Settings::update::<McpSettings>(cx, |s| s.writes = writes);
                                }),
                        ),
                    )
                    .child(
                        row("Port", "Listens on 127.0.0.1 only. Clients are configured with it.", &colors)
                            .child(
                                div()
                                    .w(u(72.0))
                                    .h(u(26.0))
                                    .px(u(8.0))
                                    .flex()
                                    .items_center()
                                    .rounded(u(5.0))
                                    .bg(colors.input_background)
                                    .border_1()
                                    .border_color(if self.error.is_some() {
                                        colors.red
                                    } else {
                                        colors.border
                                    })
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.5))
                                    .child(Input::new(&self.port).appearance(false)),
                            )
                            .child(
                                Button::new("mcp-port-save")
                                    .label("Save")
                                    .disabled(!dirty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.save_port(window, cx)
                                    })),
                            ),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .pt(u(6.0))
                                .text_size(u(12.0))
                                .text_color(colors.red)
                                .child(error),
                        )
                    })
                    .when_some(address, |this, (text, color, mono)| {
                        this.child(
                            div()
                                .pt(u(10.0))
                                .text_size(u(12.0))
                                .text_color(color)
                                .when(mono, |this| this.font_family(fonts::MONO))
                                .child(text),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap(u(6.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("mcp-regenerate")
                            .label("New token")
                            .ghost()
                            .disabled(!running)
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                cx.dispatch_action(&RegenerateToken);
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("mcp-copy-token")
                            .label("Copy token")
                            .disabled(!serving)
                            .on_click(|_, _, cx| copy(cx, McpService::token_text)),
                    )
                    .child(
                        Button::new("mcp-copy-json")
                            .label("Copy JSON")
                            .disabled(!serving)
                            .on_click(|_, _, cx| copy(cx, McpService::json_config)),
                    )
                    .child(
                        Button::new("mcp-copy-claude")
                            .label("Copy Claude Code command")
                            .primary()
                            .disabled(!serving)
                            .on_click(|_, _, cx| copy(cx, McpService::claude_code_command)),
                    ),
            )
            .into_any_element()
    }
}

/// A setting: title and explanation on the left, its control(s) appended on the right.
fn row(title: &'static str, sub: &'static str, colors: &Colors) -> gpui::Div {
    h_flex()
        .gap(u(10.0))
        .py(u(10.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(u(2.0))
                .child(div().text_size(u(12.5)).child(title))
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(sub),
                ),
        )
}
