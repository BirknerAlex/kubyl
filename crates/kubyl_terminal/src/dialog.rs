//! The "Debug Container" dialog (`kubectl debug`): image, the container whose processes and
//! files to share, and the command.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::ResourceRef;
use kubyl_ui::{ActiveColors, Button, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::exec::{DebugSpec, PodInfo};

type OnStart = Rc<dyn Fn(DebugSpec, &mut Window, &mut App)>;

/// Opens the dialog for `pod`; `on_start` runs with the chosen spec.
pub fn debug_container(
    pod: ResourceRef,
    image: String,
    info: gpui::Task<anyhow::Result<PodInfo>>,
    on_start: impl Fn(DebugSpec, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| DebugDialog::new(pod, image, Rc::new(on_start), window, cx));
    let weak = view.downgrade();
    cx.spawn(async move |cx| {
        let info = info.await;
        weak.update(cx, |this, cx| {
            match info {
                Ok(info) => {
                    this.target = info.default.clone();
                    this.containers = info
                        .containers
                        .into_iter()
                        .filter(|c| !c.ephemeral)
                        .map(|c| c.name)
                        .collect();
                }
                Err(err) => this.error = Some(format!("{err:#}")),
            }
            this.loaded = true;
            cx.notify();
        })
        .ok();
    })
    .detach();
    let colors = cx.colors().clone();
    let content = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(460.0))
            .margin_top(px(90.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .content({
                let view = content.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    let focus = view.read(cx).image.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
}

struct DebugDialog {
    pod: ResourceRef,
    image: Entity<InputState>,
    command: Entity<InputState>,
    containers: Vec<String>,
    target: Option<String>,
    loaded: bool,
    error: Option<String>,
    on_start: OnStart,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl DebugDialog {
    fn new(
        pod: ResourceRef,
        image: String,
        on_start: OnStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let image = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(image, window, cx);
            state
        });
        let command = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("sh");
            state.set_value("sh", window, cx);
            state
        });
        let mut subscriptions = Vec::new();
        for input in [&image, &command] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit(window, cx);
                    }
                },
            ));
        }
        Self {
            pod,
            image,
            command,
            containers: Vec::new(),
            target: None,
            loaded: false,
            error: None,
            on_start,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image = self.image.read(cx).value().trim().to_string();
        if image.is_empty() {
            self.error = Some("Enter an image.".into());
            cx.notify();
            return;
        }
        let command = self
            .command
            .read(cx)
            .value()
            .split_whitespace()
            .map(String::from)
            .collect();
        let spec = DebugSpec {
            image,
            target: self.target.clone(),
            command,
        };
        window.close_dialog(cx);
        (self.on_start)(spec, window, cx);
    }
}

impl Focusable for DebugDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn field(label: &'static str, child: impl IntoElement, cx: &App) -> impl IntoElement {
    let colors = cx.colors();
    v_flex()
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
                .child(child),
        )
}

impl Render for DebugDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.weak_entity();
        let containers = self.containers.clone();
        let current = self.target.clone();
        let target_label: SharedString = if !self.loaded {
            "loading…".into()
        } else {
            current.clone().unwrap_or_else(|| "none".into()).into()
        };
        let target_menu = MenuButton::new("debug-target")
            .ghost()
            .compact()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .gap(u(6.0))
                    .child(Icon::new(IconName::Box).size(12.0))
                    .child(div().flex_1().child(target_label))
                    .child(Icon::new(IconName::ChevronDown).size(11.0)),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                let none = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new("None (no shared processes)")
                        .checked(current.is_none())
                        .on_click(move |_, _, cx| {
                            none.update(cx, |this, cx| {
                                this.target = None;
                                cx.notify();
                            })
                            .ok();
                        }),
                );
                for name in &containers {
                    let weak = weak.clone();
                    let pick = name.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(current.as_deref() == Some(name))
                            .on_click(move |_, _, cx| {
                                let pick = pick.clone();
                                weak.update(cx, |this, cx| {
                                    this.target = Some(pick);
                                    cx.notify();
                                })
                                .ok();
                            }),
                    );
                }
                menu
            });
        let pod = self.pod.name.clone().unwrap_or_default();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                div()
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Debug container in {pod}")),
            )
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .text_color(colors.text_muted)
                    .child(div().text_size(u(12.0)).child(
                        "Adds an ephemeral container to the pod (it stays until the pod is \
                         deleted) and attaches to it. Sharing a container's processes makes its \
                         files reachable at /proc/1/root, also for distroless images.",
                    ))
                    .child(field(
                        "Image",
                        Input::new(&self.image).appearance(false),
                        cx,
                    ))
                    .child(field("Share processes with", target_menu, cx))
                    .child(field(
                        "Command",
                        Input::new(&self.command).appearance(false),
                        cx,
                    ))
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
                            .label("Start")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
