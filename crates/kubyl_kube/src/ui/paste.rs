//! "Paste kubeconfig YAML": saves pasted YAML under `<config dir>/kubeconfigs/<name>.yaml`
//! (mode 0600), which is loaded as the "Pasted kubeconfigs" source. Exec plugins in it are
//! listed and have to be trusted first: once added, they run when you connect.

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use kubyl_core::{Notification, NotificationCenter};
use kubyl_ui::{ActiveColors, Button, fonts, h_flex, u, v_flex};

use crate::ConnectionManager;
use crate::kubeconfig::{ExecCommand, exec_commands};
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
    /// The exec plugins in the YAML; they need `trusted`.
    commands: Vec<ExecCommand>,
    trusted: bool,
    focus: FocusHandle,
    _subscription: gpui::Subscription,
}

impl PasteView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let yaml = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("apiVersion: v1\nkind: Config\nclusters: …")
        });
        let subscription = cx.subscribe(&yaml, |this, yaml, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let commands = exec_commands(&yaml.read(cx).value());
                if commands != this.commands {
                    this.commands = commands;
                    this.trusted = false;
                }
                cx.notify();
            }
        });
        Self {
            name: cx
                .new(|cx| InputState::new(window, cx).placeholder("kubeconfig name, e.g. team-a")),
            yaml,
            error: None,
            saving: false,
            commands: Vec::new(),
            trusted: false,
            focus: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    /// "This kubeconfig runs commands": each command line with its env and users.
    fn render_commands(
        &self,
        colors: &kubyl_ui::Colors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut list = v_flex()
            .gap(u(8.0))
            .p(u(10.0))
            .rounded(u(6.0))
            .border_1()
            .border_color(colors.yellow.opacity(0.5))
            .bg(colors.yellow.opacity(0.06))
            .child(div().text_size(u(12.0)).child(
                "This kubeconfig runs commands to get credentials. Once added, Kubyl runs them when you connect; they can do anything your user can.",
            ));
        for command in &self.commands {
            list = list.child(
                v_flex()
                    .gap(u(2.0))
                    .child(
                        div()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(command.command.clone()),
                    )
                    .when(!command.env.is_empty(), |this| {
                        this.child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(11.0))
                                .text_color(colors.text_muted)
                                .child(format!("env: {}", command.env.join(" "))),
                        )
                    })
                    .child(
                        div()
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(format!("for {}", command.users.join(", "))),
                    ),
            );
        }
        list.child(
            Checkbox::new("trust-paste-commands")
                .checked(self.trusted)
                .label("I checked these commands and trust them")
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.trusted = *checked;
                    cx.notify();
                })),
        )
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let yaml = self.yaml.read(cx).value().to_string();
        if !exec_commands(&yaml).is_empty() && !self.trusted {
            self.error = Some("Check the commands first.".into());
            cx.notify();
            return;
        }
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
                    .when(!self.commands.is_empty(), |this| {
                        this.child(self.render_commands(&colors, cx))
                    })
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
                            .disabled(self.saving || (!self.commands.is_empty() && !self.trusted))
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use kubyl_settings::Settings;

    const EXEC: &str = "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster: {server: https://127.0.0.1:6443}\ncontexts:\n- name: c\n  context: {cluster: c, user: u}\nusers:\n- name: u\n  user:\n    exec:\n      apiVersion: client.authentication.k8s.io/v1\n      command: sso-helper\n      args: [token]\n";

    #[gpui::test]
    fn pasted_exec_plugins_need_trust(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let pasted = dir.path().join("pasted");
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            gpui_component::init(cx);
            Settings::register::<crate::settings::KubeSettings>(cx);
            ConnectionManager::install(pasted.clone(), false, cx);
        });
        // Closing the dialog needs gpui-component's Root as the window's first layer.
        let slot: std::rc::Rc<std::cell::RefCell<Option<Entity<PasteView>>>> = Default::default();
        let (_root, cx) = cx.add_window_view({
            let slot = slot.clone();
            move |window, cx| {
                let view = cx.new(|cx| PasteView::new(window, cx));
                *slot.borrow_mut() = Some(view.clone());
                gpui_component::Root::new(view, window, cx)
            }
        });
        let view = slot.borrow().clone().unwrap();
        view.update_in(cx, |view, window, cx| {
            view.yaml.update(cx, |state, cx| {
                state.set_value(EXEC, window, cx);
                // Typing and pasting emit this; set_value doesn't.
                cx.emit(InputEvent::Change);
            });
        });
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            assert_eq!(view.commands.len(), 1);
            assert_eq!(view.commands[0].command, "sso-helper token");
            view.save(window, cx);
            assert!(view.error.is_some(), "refused without trust");
        });
        cx.run_until_parked();
        assert!(!pasted.exists() || std::fs::read_dir(&pasted).unwrap().next().is_none());
        view.update_in(cx, |view, window, cx| {
            view.trusted = true;
            view.save(window, cx);
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read_dir(&pasted).unwrap().count(), 1);
    }
}
