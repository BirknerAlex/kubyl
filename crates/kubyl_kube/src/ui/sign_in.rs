//! The OIDC sign-in modal (mockup board 5): issuer, client id and scopes, a "waiting for your
//! browser" state with "Reopen browser", a device code with a copy button, and cancel.

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::spinner::Spinner;
use kubyl_core::{ClusterId, Notification, NotificationCenter, spawn_kube};
use kubyl_ui::{ActiveColors, Button, Icon, IconButton, IconName, fonts, h_flex, u, v_flex};

use crate::ConnectionManager;
use crate::auth::oidc::SignInMethod;
use crate::auth::{AuthError, AuthMethod, OidcParams, SignInEvent, store};

enum Phase {
    Starting,
    Browser {
        url: String,
        redirect_uri: String,
    },
    DeviceCode {
        user_code: String,
        verification_uri: String,
        verification_uri_complete: Option<String>,
    },
    Failed(String),
}

pub struct SignInView {
    cluster: ClusterId,
    name: SharedString,
    params: OidcParams,
    method: SignInMethod,
    phase: Phase,
    focus: FocusHandle,
    /// The running flow. Dropping it cancels the sign-in and closes the loopback port.
    flow: Option<Task<()>>,
    events: Option<Task<()>>,
}

/// Opens the sign-in modal for an OIDC or OpenShift context in `window`.
pub fn open_sign_in(cluster: ClusterId, window: &mut Window, cx: &mut App) {
    let manager = ConnectionManager::global(cx);
    let openshift = manager
        .read(cx)
        .context(&cluster)
        .is_some_and(|c| c.auth == AuthMethod::OpenShift);
    if openshift {
        match manager.read(cx).openshift_auth(&cluster) {
            Some(auth) => super::openshift_sign_in::open(cluster, auth, window, cx),
            // Not tried yet: connect; the modal opens if the token is rejected.
            None => manager.update(cx, |m, cx| m.connect_interactive(&cluster, cx)),
        }
        return;
    }
    let Some(auth) = manager.read(cx).oidc_auth(&cluster) else {
        NotificationCenter::push(cx, Notification::error("This context doesn't use OIDC."));
        return;
    };
    let name = manager.read(cx).display_name(&cluster);
    let params = auth.params.clone();
    let view = cx.new(|cx| {
        let mut view = SignInView {
            cluster,
            name,
            method: if params.prefer_device_code {
                SignInMethod::DeviceCode
            } else {
                SignInMethod::Browser
            },
            params,
            phase: Phase::Starting,
            focus: cx.focus_handle(),
            flow: None,
            events: None,
        };
        view.start(view.method, window, cx);
        view
    });
    show(view, window, cx);
}

fn show(view: Entity<SignInView>, window: &mut Window, cx: &mut App) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(500.))
            .margin_top(px(150.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter would close the dialog and abort the running sign-in.
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
}

impl SignInView {
    fn start(&mut self, method: SignInMethod, window: &mut Window, cx: &mut Context<Self>) {
        let Some(auth) = ConnectionManager::global(cx)
            .read(cx)
            .oidc_auth(&self.cluster)
        else {
            self.phase = Phase::Failed("The context no longer exists.".into());
            return;
        };
        self.method = method;
        self.phase = Phase::Starting;
        let (tx, mut rx) = mpsc::unbounded::<SignInEvent>();
        // Replacing the old tasks cancels a running flow (e.g. browser → device code).
        self.events = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update(cx, |this, cx| this.on_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        let flow = spawn_kube(cx, async move { auth.sign_in(method, tx).await });
        self.flow = Some(cx.spawn_in(window, async move |this, cx| {
            let result = flow.await;
            this.update_in(cx, |this, window, cx| this.finish(result, window, cx))
                .ok();
        }));
        cx.notify();
    }

    fn on_event(&mut self, event: SignInEvent, cx: &mut Context<Self>) {
        self.phase = match event {
            SignInEvent::WaitingForBrowser { url, redirect_uri } => {
                Phase::Browser { url, redirect_uri }
            }
            SignInEvent::DeviceCode {
                user_code,
                verification_uri,
                verification_uri_complete,
            } => {
                if let Some(url) = verification_uri_complete.as_ref() {
                    open::that_detached(url).ok();
                }
                Phase::DeviceCode {
                    user_code,
                    verification_uri,
                    verification_uri_complete,
                }
            }
        };
        cx.notify();
    }

    fn finish(
        &mut self,
        result: Result<(), AuthError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => {
                let cluster = self.cluster.clone();
                NotificationCenter::push(
                    cx,
                    Notification::success(format!("Signed in to {}", self.name)),
                );
                ConnectionManager::global(cx).update(cx, |m, cx| m.connect(&cluster, cx));
                window.close_dialog(cx);
            }
            Err(err) => {
                self.phase = Phase::Failed(err.to_string());
                cx.notify();
            }
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.events.take();
        window.close_dialog(cx);
        // The flow task is dropped with the view, which aborts the sign-in.
        cx.notify();
    }
}

impl Focusable for SignInView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SignInView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let kv = |label: &'static str, value: String| {
            h_flex()
                .gap(u(8.0))
                .child(
                    div()
                        .w(u(90.0))
                        .flex_none()
                        .text_color(colors.text_dim)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(value),
                )
        };
        let scopes = self.params.scopes();

        let status = match &self.phase {
            Phase::Starting => status_box(
                &colors,
                Spinner::new().color(colors.accent).into_any_element(),
                "Contacting the identity provider…".into(),
                self.params.issuer.clone(),
                None,
            ),
            Phase::Browser { url, redirect_uri } => {
                let url = url.clone();
                status_box(
                    &colors,
                    Spinner::new().color(colors.accent).into_any_element(),
                    "Waiting for your browser…".into(),
                    format!("callback on {redirect_uri} · PKCE"),
                    Some(
                        Button::new("reopen-browser")
                            .label("Reopen browser")
                            .on_click(move |_, _, _| {
                                open::that_detached(&url).ok();
                            })
                            .into_any_element(),
                    ),
                )
            }
            Phase::DeviceCode {
                verification_uri, ..
            } => status_box(
                &colors,
                Spinner::new().color(colors.accent).into_any_element(),
                "Waiting for the device code…".into(),
                format!("enter the code at {verification_uri}"),
                None,
            ),
            Phase::Failed(message) => status_box(
                &colors,
                Icon::new(IconName::CircleX)
                    .size(18.0)
                    .color(colors.red)
                    .into_any_element(),
                "Sign-in failed".into(),
                message.clone(),
                None,
            ),
        };

        let device = match &self.phase {
            Phase::DeviceCode {
                user_code,
                verification_uri,
                verification_uri_complete,
            } => {
                let code = user_code.clone();
                let link = verification_uri_complete
                    .clone()
                    .unwrap_or_else(|| verification_uri.clone());
                let host = url::Url::parse(verification_uri)
                    .map(|u| format!("{}{}", u.host_str().unwrap_or_default(), u.path()))
                    .unwrap_or_else(|_| verification_uri.clone());
                Some(
                    h_flex()
                        .gap(u(10.0))
                        .child(
                            div()
                                .font_family(fonts::MONO)
                                .text_size(u(18.0))
                                .px(u(12.0))
                                .py(u(6.0))
                                .rounded(u(6.0))
                                .bg(colors.input_background)
                                .border_1()
                                .border_color(colors.border)
                                .child(user_code.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(u(4.0))
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child("at")
                                .child(
                                    div()
                                        .id("device-link")
                                        .text_color(colors.accent)
                                        .cursor_pointer()
                                        .on_click(move |_, _, _| {
                                            open::that_detached(&link).ok();
                                        })
                                        .child(host),
                                ),
                        )
                        .child(
                            IconButton::new("copy-code", IconName::Copy)
                                .icon_size(13.0)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                                }),
                        ),
                )
            }
            _ => None,
        };

        let method = self.method;
        let footer = h_flex()
            .justify_end()
            .gap(u(8.0))
            .px(u(16.0))
            .py(u(12.0))
            .border_t_1()
            .border_color(colors.border_variant)
            .child(
                Button::new("cancel-sign-in")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
            )
            .child(match (method, &self.phase) {
                (_, Phase::Failed(_)) => Button::new("retry-sign-in")
                    .primary()
                    .label("Try again")
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.start(method, window, cx)),
                    ),
                (SignInMethod::Browser, _) => Button::new("use-device-code")
                    .primary()
                    .label("Use device code")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start(SignInMethod::DeviceCode, window, cx)
                    })),
                (SignInMethod::DeviceCode, _) => Button::new("use-browser")
                    .primary()
                    .label("Use browser")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start(SignInMethod::Browser, window, cx)
                    })),
            });

        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(
                h_flex()
                    .gap(u(10.0))
                    .px(u(16.0))
                    .py(u(14.0))
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(Icon::new(IconName::Key).size(16.0).color(colors.yellow))
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Sign in to {}", self.name)),
                    )
                    .child(
                        IconButton::new("close-sign-in", IconName::X)
                            .icon_size(13.0)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                    ),
            )
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(14.0))
                    .child(
                        v_flex()
                            .gap(u(5.0))
                            .text_size(u(12.0))
                            .child(kv("Issuer", self.params.issuer.clone()))
                            .child(kv("Client ID", self.params.client_id.clone()))
                            .child(kv("Scopes", scopes.join(" "))),
                    )
                    .child(status)
                    .when(self.method == SignInMethod::Browser, |this| {
                        this.child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_muted)
                                .child("No browser on this machine? Use a device code instead."),
                        )
                    })
                    .children(device)
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child(Icon::new(IconName::Lock).size(12.0))
                            .child(format!(
                                "Tokens are stored in the {} · never in kubeconfig or settings",
                                store::store_name()
                            )),
                    ),
            )
            .child(footer)
    }
}

pub(super) fn status_box(
    colors: &kubyl_ui::Colors,
    icon: gpui::AnyElement,
    title: String,
    subtitle: String,
    action: Option<gpui::AnyElement>,
) -> gpui::AnyElement {
    h_flex()
        .gap(u(12.0))
        .p(u(12.0))
        .rounded(u(7.0))
        .bg(colors.subheader_background)
        .border_1()
        .border_color(colors.border_variant)
        .child(div().size(u(18.0)).flex_none().child(icon))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_size(u(12.5)).child(title))
                .child(
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.0))
                        .text_color(colors.text_dim)
                        .child(subtitle),
                ),
        )
        .children(action)
        .into_any_element()
}
