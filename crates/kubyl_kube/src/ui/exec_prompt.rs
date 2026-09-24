//! A prompt for exec plugins with `interactiveMode: Always`: shows their output (stderr) and
//! sends typed lines to their stdin.

use futures::StreamExt as _;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_ui::{ActiveColors, Button, Icon, IconName, fonts, h_flex, u, v_flex};

use super::with_active_window;
use crate::auth::exec::{ExecPrompt, prompt_receiver};

pub(crate) fn init(cx: &mut App) {
    let Some(mut prompts) = prompt_receiver() else {
        return;
    };
    cx.spawn(async move |cx| {
        while let Some(prompt) = prompts.next().await {
            cx.update(|cx| with_active_window(cx, |window, cx| open(prompt, window, cx)));
        }
    })
    .detach();
}

fn open(prompt: ExecPrompt, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| ExecPromptView::new(prompt, window, cx));
    let colors = cx.colors().clone();
    let dialog_view = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(560.))
            .margin_top(px(120.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .keyboard(false)
            .overlay_closable(false)
            .child(dialog_view.clone())
    });
    let focus = view.read(cx).input.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
}

struct ExecPromptView {
    context: String,
    command: String,
    output: String,
    input: Entity<InputState>,
    stdin: futures::channel::mpsc::UnboundedSender<String>,
    cancel: Option<futures::channel::oneshot::Sender<()>>,
    focus: FocusHandle,
    _reader: Task<()>,
    _subscription: gpui::Subscription,
}

impl ExecPromptView {
    fn new(prompt: ExecPrompt, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ExecPrompt {
            context,
            command,
            mut output,
            input: stdin,
            cancel,
        } = prompt;
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type a response and press enter"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let line = input.read(cx).value().to_string();
                    this.stdin.unbounded_send(line).ok();
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            },
        );
        let reader = cx.spawn_in(window, async move |this, cx| {
            while let Some(chunk) = output.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.output.push_str(&chunk);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            // The plugin exited.
            this.update_in(cx, |_, window, cx| window.close_dialog(cx))
                .ok();
        });
        Self {
            context,
            command,
            output: String::new(),
            input,
            stdin,
            cancel: Some(cancel),
            focus: cx.focus_handle(),
            _reader: reader,
            _subscription: subscription,
        }
    }
}

impl Focusable for ExecPromptView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ExecPromptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                h_flex()
                    .gap(u(10.0))
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        Icon::new(IconName::Terminal)
                            .size(16.0)
                            .color(colors.accent),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{} needs input ({})", self.command, self.context)),
                    ),
            )
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .child(
                        div()
                            .id("exec-output")
                            .h(u(200.0))
                            .overflow_y_scroll()
                            .p(u(10.0))
                            .rounded(u(6.0))
                            .bg(colors.background)
                            .border_1()
                            .border_color(colors.border_variant)
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .whitespace_normal()
                            .child(if self.output.is_empty() {
                                "Waiting for output…".to_string()
                            } else {
                                self.output.clone()
                            }),
                    )
                    .child(Input::new(&self.input).mask_toggle()),
            )
            .child(
                h_flex()
                    .justify_end()
                    .px(u(16.0))
                    .py(u(12.0))
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("cancel-exec")
                            .danger()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(cancel) = this.cancel.take() {
                                    cancel.send(()).ok();
                                }
                                window.close_dialog(cx);
                            })),
                    ),
            )
    }
}
