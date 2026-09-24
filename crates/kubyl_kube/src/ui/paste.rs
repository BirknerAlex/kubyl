//! "Paste kubeconfig YAML": saves pasted YAML under `<config dir>/kubeconfigs/<name>.yaml`
//! (mode 0600), which is loaded as the "Pasted kubeconfigs" source.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use kubyl_core::{Notification, NotificationCenter};
use kubyl_ui::{ActiveColors, Button, fonts, h_flex, u, v_flex};

use crate::ConnectionManager;
use crate::settings::display_path;

pub fn open_paste_dialog(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| PasteView::new(window, cx));
    let colors = cx.colors().clone();
    let dialog_view = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(600.))
            .margin_top(px(90.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .child(dialog_view.clone())
    });
    let focus = view.read(cx).yaml.read(cx).focus_handle(cx);
    window.focus(&focus, cx);
}

struct PasteView {
    name: Entity<InputState>,
    yaml: Entity<TextareaState>,
    error: Option<String>,
    saving: bool,
    focus: FocusHandle,
}

impl PasteView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            name: cx
                .new(|cx| InputState::new(window, cx).placeholder("kubeconfig name, e.g. team-a")),
            yaml: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("apiVersion: v1\nkind: Config\nclusters: …")
            }),
            error: None,
            saving: false,
            focus: cx.focus_handle(),
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let yaml = self.yaml.read(cx).value().to_string();
        let mut name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            // Name it after the first context.
            name = crate::kubeconfig::validate_yaml(&yaml)
                .ok()
                .and_then(|names| names.into_iter().next())
                .unwrap_or_else(|| "kubeconfig".into());
        }
        self.saving = true;
        self.error = None;
        let task =
            ConnectionManager::global(cx).update(cx, |m, cx| m.paste_kubeconfig(name, yaml, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(path) => {
                        NotificationCenter::push(
                            cx,
                            Notification::success(format!("Saved to {}", display_path(&path))),
                        );
                        window.close_dialog(cx);
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Focusable for PasteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PasteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                div()
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Paste kubeconfig YAML"),
            )
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .child(Input::new(&self.name))
                    .child(
                        div()
                            .h(u(260.0))
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(Textarea::new(&self.yaml).h_full()),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(format!(
                                "Saved to {} with permissions 0600. Tokens inside stay in that file only.",
                                display_path(ConnectionManager::global(cx).read(cx).pasted_dir())
                            )),
                    )
                    .children(self.error.clone().map(|err| {
                        div().text_size(u(12.0)).text_color(colors.red).child(err)
                    })),
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
                        Button::new("cancel-paste")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("save-paste")
                            .primary()
                            .label(if self.saving { "Saving…" } else { "Add kubeconfig" })
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}
