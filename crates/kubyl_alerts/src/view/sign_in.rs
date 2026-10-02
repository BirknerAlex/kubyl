//! The strip that signs in to an Alertmanager behind HTTP basic auth: the username and password
//! go to the keychain once it takes them, and are asked again when it stops taking them.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, IntoElement, Subscription, Window,
    div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_core::ClusterId;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};
use secrecy::SecretString;

use super::AlertsView;
use crate::service::{AlertsService, Locked};

pub(crate) struct State {
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<AlertsView>) -> Self {
        Self {
            username: cx.new(|cx| InputState::new(window, cx).placeholder("username")),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("password")
            }),
        }
    }

    pub(crate) fn subscriptions(
        &self,
        window: &mut Window,
        cx: &mut Context<AlertsView>,
    ) -> Vec<Subscription> {
        let on_enter = |this: &mut AlertsView,
                        _: &Entity<InputState>,
                        event: &InputEvent,
                        window: &mut Window,
                        cx: &mut Context<AlertsView>| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.sign_in(window, cx);
            }
        };
        vec![
            cx.subscribe_in(&self.username, window, on_enter),
            cx.subscribe_in(&self.password, window, on_enter),
        ]
    }
}

impl AlertsView {
    /// The first Alertmanager of the view's clusters that waits for a sign-in.
    fn locked(&self, cx: &gpui::App) -> Option<(ClusterId, Locked)> {
        self.clusters(cx).into_iter().find_map(|cluster| {
            let locked = self.state(&cluster, cx)?.locked.first()?.clone();
            Some((cluster, locked))
        })
    }

    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((cluster, locked)) = self.locked(cx) else {
            return;
        };
        if locked.busy {
            return;
        }
        let username = self.sign_in_form.username.read(cx).value().trim().to_string();
        let password = self.sign_in_form.password.read(cx).value().to_string();
        if username.is_empty() || password.is_empty() {
            return;
        }
        // The password isn't kept in the field.
        self.sign_in
            .password
            .update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(service) = AlertsService::global(cx) {
            let password = SecretString::from(password);
            service.update(cx, |s, cx| {
                s.sign_in(&cluster, &locked.label(), &username, &password, cx)
            });
        }
    }

    /// A strip above the tab body while an Alertmanager waits for a sign-in.
    pub(crate) fn render_sign_in(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (cluster, locked) = self.locked(cx)?;
        let colors: Colors = cx.colors().clone();
        let field = |input: &Entity<InputState>| {
            div()
                .w(u(170.0))
                .h(u(26.0))
                .px(u(8.0))
                .flex()
                .items_center()
                .rounded(u(5.0))
                .bg(colors.input_background)
                .border_1()
                .border_color(colors.border)
                .child(Input::new(input).appearance(false))
        };
        let mut title = format!("Alertmanager {} asks for a username and password", locked.label());
        if self.cluster.is_none() {
            title.push_str(&format!(" ({})", Self::cluster_name(&cluster, cx)));
        }
        let label = locked.label();
        Some(
            v_flex()
                .flex_none()
                .px(u(16.0))
                .py(u(10.0))
                .gap(u(8.0))
                .border_b_1()
                .border_color(colors.border_variant)
                .bg(colors.accent.opacity(0.06))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(Icon::new(IconName::Lock).size(13.0).color(colors.accent))
                        .child(
                            div()
                                .text_size(u(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        ),
                )
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(field(&self.sign_in_form.username))
                        .child(field(&self.sign_in_form.password))
                        .child(
                            Button::new("alerts-sign-in")
                                .primary()
                                .icon(IconName::Key)
                                .label(if locked.busy { "Signing in…" } else { "Sign in" })
                                .disabled(locked.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.sign_in(window, cx)
                                })),
                        )
                        .when(locked.saved && !locked.busy, |this| {
                            this.child(
                                Button::new("alerts-reconnect")
                                    .icon(IconName::RefreshCw)
                                    .label("Reconnect")
                                    .on_click(move |_, _, cx| {
                                        if let Some(service) = AlertsService::global(cx) {
                                            let (cluster, label) = (cluster.clone(), label.clone());
                                            service.update(cx, |s, cx| {
                                                s.reconnect(&cluster, &label, cx)
                                            });
                                        }
                                    }),
                            )
                        }),
                )
                .when_some(locked.problem.clone(), |this, problem| {
                    this.child(
                        div()
                            .text_size(u(12.0))
                            .text_color(colors.red)
                            .child(problem),
                    )
                })
                .child(
                    div()
                        .text_size(u(11.5))
                        .text_color(colors.text_dim)
                        .child(format!(
                            "Read through a temporary port-forward, since the API server's \
                             proxy can't pass credentials on. They go to the {} and only to \
                             this Alertmanager; once it rejects them they're deleted and you're \
                             asked again.",
                            kubyl_kube::auth::store::store_name()
                        )),
                )
                .font_family(fonts::UI)
                .into_any_element(),
        )
    }
}
