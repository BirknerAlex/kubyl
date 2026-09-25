//! The port-forward dialogs: "Port-Forward…" (pick the remote port, local port and bind
//! address; open in a browser; save) and "Saved Port-Forwards" (start, auto-start, remove).

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::ResourceRef;
use kubyl_ui::{ActiveColors, Button, Chip, Icon, IconButton, IconName, fonts, h_flex, u, v_flex};

use crate::favorites::{SavedForward, SavedForwards, short_kind};
use crate::manager::PortForwardManager;
use crate::resolve::PortChoice;

/// What the user chose in the forward dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct ForwardChoice {
    pub remote_port: Option<u16>,
    pub local_port: u16,
    pub bind_address: String,
    pub http: bool,
    pub https: bool,
    pub open_browser: bool,
    pub save: bool,
    pub auto_start: bool,
}

type OnStart = Rc<dyn Fn(ForwardChoice, &mut Window, &mut App)>;

fn open<V: Render>(view: Entity<V>, width: f32, window: &mut Window, cx: &mut App) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(width))
            .margin_top(px(90.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
}

fn header(title: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let colors = cx.colors();
    div()
        .px(u(16.0))
        .py(u(14.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}

fn field(label: &'static str, input: &Entity<InputState>, cx: &App) -> impl IntoElement {
    let colors = cx.colors();
    v_flex()
        .flex_1()
        .gap(u(4.0))
        .child(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(label),
        )
        .child(
            div()
                .h(u(28.0))
                .px(u(8.0))
                .flex()
                .items_center()
                .rounded(u(5.0))
                .bg(colors.input_background)
                .border_1()
                .border_color(colors.border)
                .font_family(fonts::MONO)
                .text_size(u(12.5))
                .child(Input::new(input).appearance(false)),
        )
}

/// Opens the forward dialog for `target`; `ports` loads its ports.
pub fn forward(
    target: ResourceRef,
    ports: gpui::Task<anyhow::Result<Vec<PortChoice>>>,
    on_start: impl Fn(ForwardChoice, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| ForwardDialog::new(target, Rc::new(on_start), window, cx));
    let weak = view.downgrade();
    cx.spawn(async move |cx| {
        let ports = ports.await;
        weak.update(cx, |this, cx| {
            match ports {
                Ok(ports) => {
                    // HTTP ports first, so "open in browser" targets are one click away.
                    this.selected = ports
                        .iter()
                        .position(|p| p.http)
                        .or((!ports.is_empty()).then_some(0));
                    this.ports = Some(ports);
                }
                Err(err) => {
                    this.error = Some(format!("{err:#}"));
                    this.ports = Some(Vec::new());
                }
            }
            this.sync_http();
            cx.notify();
        })
        .ok();
    })
    .detach();
    open(view.clone(), 500.0, window, cx);
    let focus = view.read(cx).focus.clone();
    window.focus(&focus, cx);
}

struct ForwardDialog {
    target: ResourceRef,
    ports: Option<Vec<PortChoice>>,
    /// Index into `ports`; `None` with `custom` set = the custom port.
    selected: Option<usize>,
    custom: Entity<InputState>,
    local: Entity<InputState>,
    bind: Entity<InputState>,
    open_browser: bool,
    save: bool,
    auto_start: bool,
    error: Option<String>,
    on_start: OnStart,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl ForwardDialog {
    fn new(
        target: ResourceRef,
        on_start: OnStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let custom = cx.new(|cx| InputState::new(window, cx).placeholder("port"));
        let local = cx.new(|cx| InputState::new(window, cx).placeholder("auto"));
        let bind = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value("127.0.0.1", window, cx);
            state
        });
        let mut subscriptions = Vec::new();
        for input in [&custom, &local, &bind] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    InputEvent::Change => {
                        if input == &this.custom && !input.read(cx).value().is_empty() {
                            this.selected = None;
                        }
                        this.error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ));
        }
        Self {
            target,
            ports: None,
            selected: None,
            custom,
            local,
            bind,
            open_browser: false,
            save: false,
            auto_start: false,
            error: None,
            on_start,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn selected_port(&self) -> Option<&PortChoice> {
        self.ports.as_ref()?.get(self.selected?)
    }

    /// Opening a browser is on by default for HTTP ports.
    fn sync_http(&mut self) {
        self.open_browser = self.selected_port().is_some_and(|p| p.http);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (remote_port, http, https) = match self.selected_port() {
            Some(port) => (Some(port.port), port.http, port.https),
            None => {
                let text = self.custom.read(cx).value().trim().to_string();
                if text.is_empty() {
                    // No ports declared and none typed: the first container port (resolved per
                    // connection) is all we can do.
                    if self.ports.as_ref().is_some_and(|p| p.is_empty()) {
                        self.error = Some("Enter the remote port.".into());
                        cx.notify();
                        return;
                    }
                    (None, false, false)
                } else {
                    match text.parse::<u16>() {
                        Ok(port) if port > 0 => {
                            let (http, https) = crate::resolve::http_kind(port, None, None);
                            (Some(port), http, https)
                        }
                        _ => {
                            self.error = Some("The remote port must be 1-65535.".into());
                            cx.notify();
                            return;
                        }
                    }
                }
            }
        };
        let local_text = self.local.read(cx).value().trim().to_string();
        let local_port = if local_text.is_empty() || local_text == "auto" {
            0
        } else {
            match local_text.parse::<u16>() {
                Ok(port) => port,
                Err(_) => {
                    self.error =
                        Some("The local port must be a number (or empty for auto).".into());
                    cx.notify();
                    return;
                }
            }
        };
        let bind_address = self.bind.read(cx).value().trim().to_string();
        if bind_address.parse::<std::net::IpAddr>().is_err() && bind_address != "localhost" {
            self.error = Some("The bind address must be an IP address.".into());
            cx.notify();
            return;
        }
        let choice = ForwardChoice {
            remote_port,
            local_port,
            bind_address,
            http,
            https,
            open_browser: self.open_browser && http,
            save: self.save,
            auto_start: self.save && self.auto_start,
        };
        window.close_dialog(cx);
        (self.on_start)(choice, window, cx);
    }
}

impl Focusable for ForwardDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ForwardDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let title = format!(
            "Port-forward {}/{}",
            short_kind(&self.target.gvr.resource),
            self.target.name.clone().unwrap_or_default()
        );
        let mut ports = v_flex().gap(u(2.0));
        match &self.ports {
            None => {
                ports = ports.child(
                    div()
                        .py(u(6.0))
                        .text_color(colors.text_dim)
                        .child("Loading ports…"),
                );
            }
            Some(list) if list.is_empty() => {
                ports = ports.child(
                    div()
                        .py(u(6.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child("No TCP ports declared. Enter the port the process listens on."),
                );
            }
            Some(list) => {
                for (ix, port) in list.iter().enumerate() {
                    let selected = self.selected == Some(ix);
                    let hover = colors.hover;
                    ports = ports.child(
                        h_flex()
                            .id(("forward-port", ix))
                            .gap(u(8.0))
                            .px(u(10.0))
                            .py(u(5.0))
                            .rounded(u(5.0))
                            .cursor_pointer()
                            .when(selected, |this| {
                                this.bg(colors.selection)
                                    .border_1()
                                    .border_color(colors.accent)
                            })
                            .when(!selected, |this| this.hover(move |s| s.bg(hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(ix);
                                this.sync_http();
                                cx.notify();
                            }))
                            .child(Icon::new(IconName::Link).size(12.0))
                            .child(
                                div()
                                    .flex_1()
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.5))
                                    .child(port.label()),
                            )
                            .when(port.http, |this| {
                                this.child(Chip::new(if port.https { "HTTPS" } else { "HTTP" }))
                            }),
                    );
                }
            }
        }
        let has_ports = self.ports.as_ref().is_some_and(|p| !p.is_empty());
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(title, cx))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(12.0))
                    .child(
                        v_flex()
                            .gap(u(4.0))
                            .child(div().text_size(u(12.0)).text_color(colors.text_dim).child(
                                if self.target.gvr.resource == "services" {
                                    "Service port"
                                } else {
                                    "Container port"
                                },
                            ))
                            .child(ports),
                    )
                    .child(
                        h_flex()
                            .gap(u(10.0))
                            .child(field(
                                if has_ports {
                                    "Other remote port"
                                } else {
                                    "Remote port"
                                },
                                &self.custom,
                                cx,
                            ))
                            .child(field("Local port", &self.local, cx))
                            .child(field("Bind address", &self.bind, cx)),
                    )
                    .child(
                        v_flex()
                            .gap(u(6.0))
                            .child(
                                Checkbox::new("forward-open-browser")
                                    .label("Open in browser when ready")
                                    .checked(self.open_browser)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.open_browser = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new("forward-save")
                                    .label("Save this forward")
                                    .checked(self.save)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.save = *checked;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .pl(u(22.0))
                                    .when(!self.save, |this| this.opacity(0.5))
                                    .child(
                                        Checkbox::new("forward-auto-start")
                                            .label("Start when the cluster connects")
                                            .checked(self.auto_start && self.save)
                                            .on_click(cx.listener(
                                                |this, checked: &bool, _, cx| {
                                                    if this.save {
                                                        this.auto_start = *checked;
                                                        cx.notify();
                                                    }
                                                },
                                            )),
                                    ),
                            ),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(div().text_size(u(12.0)).text_color(colors.red).child(error))
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(u(8.0))
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("start")
                            .primary()
                            .label("Start Forward")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}

// ----- Saved forwards -----

type OnStartSaved = Rc<dyn Fn(SavedForward, &mut Window, &mut App)>;

/// Opens the saved-forwards dialog; `on_start` starts one.
pub fn saved(
    on_start: impl Fn(SavedForward, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| SavedDialog::new(Rc::new(on_start), cx));
    open(view.clone(), 560.0, window, cx);
    let focus = view.read(cx).focus.clone();
    window.focus(&focus, cx);
}

struct SavedDialog {
    on_start: OnStartSaved,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SavedDialog {
    fn new(on_start: OnStartSaved, cx: &mut Context<Self>) -> Self {
        let saved = SavedForwards::global(cx);
        let manager = PortForwardManager::global(cx);
        Self {
            on_start,
            focus: cx.focus_handle(),
            _subscriptions: vec![
                cx.observe(&saved, |_, _, cx| cx.notify()),
                cx.observe(&manager, |_, _, cx| cx.notify()),
            ],
        }
    }
}

impl Focusable for SavedDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SavedDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let forwards = SavedForwards::global(cx).read(cx).state().forwards.clone();
        let manager = PortForwardManager::global(cx);
        let mut list = v_flex().p(u(8.0)).gap(u(2.0)).max_h(u(380.0));
        if forwards.is_empty() {
            list = list.child(
                div()
                    .p(u(10.0))
                    .text_color(colors.text_dim)
                    .child("No saved forwards. Save one from the Port-Forward dialog or an active forward's star."),
            );
        }
        for (ix, forward) in forwards.into_iter().enumerate() {
            let running = manager.read(cx).is_running(&forward, cx);
            let start = forward.clone();
            let toggle = forward.clone();
            let remove = forward.clone();
            let on_start = self.on_start.clone();
            list = list.child(
                h_flex()
                    .id(("saved-forward", ix))
                    .gap(u(8.0))
                    .px(u(10.0))
                    .py(u(6.0))
                    .rounded(u(5.0))
                    .child(Icon::new(IconName::Link).size(13.0).color(if running {
                        colors.green
                    } else {
                        colors.text_dim
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.5))
                                    .child(forward.label()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(format!(
                                        "{} · {}{}",
                                        forward.namespace,
                                        forward.context,
                                        if running { " · running" } else { "" }
                                    )),
                            ),
                    )
                    .child(
                        IconButton::new(("saved-auto", ix), IconName::Zap)
                            .icon_size(13.0)
                            .toggled(forward.auto_start)
                            .on_click(move |_, _, cx| {
                                let toggle = toggle.clone();
                                SavedForwards::global(cx).update(cx, |this, cx| {
                                    this.update_state(cx, |state| {
                                        let mut updated = toggle.clone();
                                        updated.auto_start = !toggle.auto_start;
                                        state.upsert(updated);
                                    })
                                });
                            }),
                    )
                    .when(!running, |this| {
                        this.child(
                            IconButton::new(("saved-start", ix), IconName::Play)
                                .icon_size(13.0)
                                .on_click(move |_, window, cx| on_start(start.clone(), window, cx)),
                        )
                    })
                    .child(
                        IconButton::new(("saved-remove", ix), IconName::Trash)
                            .icon_size(13.0)
                            .on_click(move |_, _, cx| {
                                let remove = remove.clone();
                                SavedForwards::global(cx).update(cx, |this, cx| {
                                    this.update_state(cx, |state| state.remove(&remove))
                                });
                            }),
                    ),
            );
        }
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header("Saved port-forwards", cx))
            .child(
                div()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("⚡ starts a forward when its cluster connects."),
            )
            .child(div().id("saved-forwards").overflow_y_scroll().child(list))
            .child(
                h_flex()
                    .justify_end()
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("close")
                            .label("Close")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}
