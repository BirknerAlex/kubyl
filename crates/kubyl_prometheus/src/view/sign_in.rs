//! The panel of a server that wants a username and password (HTTP basic auth): they go to the
//! keychain once the server takes them, and are asked again when it stops taking them.

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FontWeight, IntoElement, Subscription, Window,
    div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use kubyl_ui::{ActiveColors, Colors, Icon, IconName, fonts, h_flex, u, v_flex};
use secrecy::SecretString;

use super::{PrometheusView, widgets};
use crate::service::{Access, Instance, PrometheusService};

pub(crate) struct State {
    username: Entity<InputState>,
    password: Entity<InputState>,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<PrometheusView>) -> Self {
        Self {
            username: cx.new(|cx| InputState::new(window, cx).placeholder("username")),
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
        }
    }

    pub(crate) fn subscriptions(
        &self,
        window: &mut Window,
        cx: &mut Context<PrometheusView>,
    ) -> Vec<Subscription> {
        let on_enter = |this: &mut PrometheusView,
                        _: &Entity<InputState>,
                        event: &InputEvent,
                        window: &mut Window,
                        cx: &mut Context<PrometheusView>| {
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

impl PrometheusView {
    fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(instance) = self.current(cx) else {
            return;
        };
        if !matches!(instance.access, Access::Locked { .. }) {
            return;
        }
        let username = self
            .sign_in_tab
            .username
            .read(cx)
            .value()
            .trim()
            .to_string();
        let password = self.sign_in_tab.password.read(cx).value().to_string();
        if username.is_empty() || password.is_empty() {
            return;
        }
        // The password isn't kept in the field.
        self.sign_in_tab
            .password
            .update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(service) = PrometheusService::global(cx) {
            let cluster = self.cluster.clone();
            let password = SecretString::from(password);
            service.update(cx, |s, cx| {
                s.sign_in(&cluster, &instance.id, &username, &password, cx)
            });
        }
    }

    pub(crate) fn render_sign_in(&self, instance: &Instance, cx: &mut Context<Self>) -> AnyElement {
        let colors: Colors = cx.colors().clone();
        let busy = instance.access == Access::Unlocking;
        let (saved, problem) = match &instance.access {
            Access::Locked { saved, problem } => (*saved, problem.clone()),
            _ => (false, None),
        };
        let field = |label: &'static str, input: &Entity<InputState>| {
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
                        .child(Input::new(input).appearance(false)),
                )
        };
        let id = instance.id.clone();
        let cluster = self.cluster.clone();
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .p(u(24.0))
            .child(
                v_flex()
                    .w(u(360.0))
                    .gap(u(12.0))
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(Icon::new(IconName::Lock).size(15.0).color(colors.accent))
                            .child(
                                div()
                                    .text_size(u(15.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Sign in to Prometheus"),
                            ),
                    )
                    .child(
                        div()
                            .text_size(u(12.5))
                            .text_color(colors.text_dim)
                            .child(format!(
                                "{} asks for a username and password. Kubyl reads it through a \
                                 temporary port-forward, since the API server's proxy can't \
                                 pass them on.",
                                instance.label()
                            )),
                    )
                    .child(field("Username", &self.sign_in_tab.username))
                    .child(field("Password", &self.sign_in_tab.password))
                    .when_some(problem, |this, problem| {
                        this.child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.red)
                                .child(problem),
                        )
                    })
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(widgets::button(
                                "prometheus-sign-in",
                                Some(IconName::Key),
                                if busy { "Signing in…" } else { "Sign in" },
                                true,
                                &colors,
                                cx.listener(|this, _, window, cx| this.sign_in(window, cx)),
                            ))
                            .when(saved && !busy, |this| {
                                this.child(widgets::button(
                                    "prometheus-reconnect",
                                    Some(IconName::RefreshCw),
                                    "Reconnect",
                                    false,
                                    &colors,
                                    move |_, _, cx| {
                                        if let Some(service) = PrometheusService::global(cx) {
                                            let (cluster, id) = (cluster.clone(), id.clone());
                                            service
                                                .update(cx, |s, cx| s.reconnect(&cluster, &id, cx));
                                        }
                                    },
                                ))
                            }),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .gap(u(8.0))
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(Icon::new(IconName::Lock).size(12.0))
                            .child(div().flex_1().min_w_0().child(format!(
                                "The credentials go to the {} and only to this server, over a \
                                 loopback port. When the server stops taking them, they're \
                                 deleted and you're asked again.",
                                kubyl_kube::auth::store::store_name()
                            ))),
                    )
                    .font_family(fonts::UI),
            )
            .into_any_element()
    }
}
