//! The OpenShift sign-in modal, like `oc login`: through the browser (`--web`), with a username
//! and password (`-u`), or with a token from the "Copy login command" page (`--token`).

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, IntoElement, Render,
    SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::spinner::Spinner;
use kubyl_core::{ClusterId, Notification, NotificationCenter, spawn_kube};
use kubyl_ui::{ActiveColors, Button, Icon, IconButton, IconName, fonts, h_flex, u, v_flex};
use secrecy::{ExposeSecret as _, SecretString};

use super::sign_in::status_box;
use crate::ConnectionManager;
use crate::auth::openshift::{Metadata, username_hint};
use crate::auth::{AuthError, OpenShiftAuth, SignInEvent, store};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Browser,
    Password,
    Token,
}

impl Method {
    const ALL: [Method; 3] = [Method::Browser, Method::Password, Method::Token];

    fn label(self) -> &'static str {
        match self {
            Method::Browser => "Browser",
            Method::Password => "Username & password",
            Method::Token => "Token",
        }
    }
}

enum Phase {
    Idle,
    Working,
    Browser { url: String, redirect_uri: String },
    Failed(String),
}

pub struct OpenShiftSignIn {
    cluster: ClusterId,
    name: SharedString,
    auth: Arc<OpenShiftAuth>,
    method: Method,
    phase: Phase,
    /// The OAuth server's endpoints (for the token page) and whether it supports the browser
    /// sign-in, or why they couldn't be read.
    oauth: Option<Result<(Metadata, bool), String>>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    token: Entity<InputState>,
    focus: FocusHandle,
    pending_focus: bool,
    /// The running sign-in. Dropping it cancels it and closes the loopback port.
    flow: Option<Task<()>>,
    events: Option<Task<()>>,
    _metadata: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// Opens the sign-in modal for an OpenShift context in `window`.
pub fn open(cluster: ClusterId, auth: Arc<OpenShiftAuth>, window: &mut Window, cx: &mut App) {
    let name = ConnectionManager::global(cx)
        .read(cx)
        .display_name(&cluster);
    let view = cx.new(|cx| OpenShiftSignIn::new(cluster, name, auth, window, cx));
    let colors = cx.colors().clone();
    let dialog_view = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(520.))
            .margin_top(px(130.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter would close the dialog and abort the sign-in; the inputs submit instead.
            .on_ok(|_, _, _| false)
            .child(dialog_view.clone())
    });
}

impl OpenShiftSignIn {
    fn new(
        cluster: ClusterId,
        name: SharedString,
        auth: Arc<OpenShiftAuth>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hint = username_hint(&auth.params.user);
        let username = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("username");
            state.set_value(hint.clone(), window, cx);
            state
        });
        // Masked inputs mask their placeholder too, so these have none.
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let token = cx.new(|cx| InputState::new(window, cx).masked(true));
        let on_enter = |this: &mut Self,
                        _: &Entity<InputState>,
                        event: &InputEvent,
                        window: &mut Window,
                        cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&username, window, on_enter),
            cx.subscribe_in(&password, window, on_enter),
            cx.subscribe_in(&token, window, on_enter),
        ];
        let fetch = {
            let auth = auth.clone();
            spawn_kube(cx, async move {
                let metadata = auth.metadata().await?;
                // Unreachable: the browser flow reports why.
                let browser = auth.browser_supported(&metadata).await.unwrap_or(true);
                Ok::<_, AuthError>((metadata, browser))
            })
        };
        let metadata = cx.spawn_in(window, async move |this, cx| {
            let result = fetch.await.map_err(|err| err.to_string());
            this.update_in(cx, |this, window, cx| this.on_metadata(result, window, cx))
                .ok();
        });
        let method = if hint == "kubeadmin" {
            // The cluster admin has a password.
            Method::Password
        } else {
            // Starts once the OAuth server is known to support it.
            Method::Browser
        };
        Self {
            cluster,
            name,
            auth,
            method,
            phase: if method == Method::Browser {
                Phase::Working
            } else {
                Phase::Idle
            },
            oauth: None,
            username,
            password,
            token,
            focus: cx.focus_handle(),
            pending_focus: true,
            flow: None,
            events: None,
            _metadata: metadata,
            _subscriptions: subscriptions,
        }
    }

    /// The OAuth server answered: start the browser sign-in, or switch to a token when the
    /// cluster can't do it.
    fn on_metadata(
        &mut self,
        result: Result<(Metadata, bool), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let waiting = self.method == Method::Browser && self.flow.is_none();
        let supported = match &result {
            Ok((_, supported)) => Some(*supported),
            Err(err) => {
                if waiting {
                    self.phase = Phase::Failed(err.clone());
                }
                None
            }
        };
        self.oauth = Some(result);
        match supported {
            Some(true) if waiting => self.start_browser(window, cx),
            Some(false) if self.method == Method::Browser => {
                self.method = Method::Token;
                self.phase = Phase::Idle;
                self.focus_input(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn browser_unsupported(&self) -> bool {
        matches!(self.oauth, Some(Ok((_, false))))
    }

    fn select(&mut self, method: Method, window: &mut Window, cx: &mut Context<Self>) {
        // Switching away cancels a running browser sign-in.
        self.flow = None;
        self.events = None;
        self.method = method;
        self.phase = Phase::Idle;
        if method == Method::Browser {
            match &self.oauth {
                Some(Ok(_)) => self.start_browser(window, cx),
                // `on_metadata` starts it.
                None => self.phase = Phase::Working,
                Some(Err(err)) => self.phase = Phase::Failed(err.clone()),
            }
        }
        self.focus_input(cx);
        cx.notify();
    }

    /// Focuses the method's first empty input when it renders next. Earlier, the input isn't
    /// part of the dialog yet and the dialog's focus trap takes focus back.
    fn focus_input(&mut self, cx: &mut Context<Self>) {
        self.pending_focus = true;
        cx.notify();
    }

    fn apply_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.pending_focus) {
            return;
        }
        let input = match self.method {
            Method::Browser => return,
            Method::Password if self.username.read(cx).value().trim().is_empty() => &self.username,
            Method::Password => &self.password,
            Method::Token => &self.token,
        };
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn start_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded::<SignInEvent>();
        self.events = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let SignInEvent::WaitingForBrowser { url, redirect_uri } = event else {
                    continue;
                };
                let updated = this.update(cx, |this, cx| {
                    this.phase = Phase::Browser { url, redirect_uri };
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
        let auth = self.auth.clone();
        let flow = spawn_kube(cx, async move { auth.sign_in_browser(tx).await });
        self.run(flow, window, cx);
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Working | Phase::Browser { .. }) {
            return;
        }
        let auth = self.auth.clone();
        match self.method {
            Method::Browser => self.start_browser(window, cx),
            Method::Password => {
                let username = self.username.read(cx).value().trim().to_string();
                let password = SecretString::from(self.password.read(cx).value().to_string());
                if username.is_empty() || password.expose_secret().is_empty() {
                    self.phase = Phase::Failed("Enter a username and a password.".into());
                    cx.notify();
                    return;
                }
                let flow = spawn_kube(cx, async move {
                    auth.sign_in_password(&username, &password).await
                });
                self.run(flow, window, cx);
            }
            Method::Token => {
                let pasted = SecretString::from(self.token.read(cx).value().to_string());
                let flow = spawn_kube(
                    cx,
                    async move { auth.use_token(pasted.expose_secret()).await },
                );
                self.run(flow, window, cx);
            }
        }
    }

    fn run(
        &mut self,
        flow: Task<Result<Option<String>, AuthError>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.phase = Phase::Working;
        self.flow = Some(cx.spawn_in(window, async move |this, cx| {
            let result = flow.await;
            this.update_in(cx, |this, window, cx| this.finish(result, window, cx))
                .ok();
        }));
        cx.notify();
    }

    fn finish(
        &mut self,
        result: Result<Option<String>, AuthError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.events = None;
        match result {
            Ok(user) => {
                let message = match user {
                    Some(user) => format!("Signed in to {} as {user}", self.name),
                    None => format!("Signed in to {}", self.name),
                };
                NotificationCenter::push(cx, Notification::success(message));
                let cluster = self.cluster.clone();
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
        self.flow = None;
        self.events = None;
        window.close_dialog(cx);
        cx.notify();
    }

    fn open_token_page(&self) {
        if let Some(Ok((metadata, _))) = &self.oauth {
            open::that_detached(metadata.token_request_url()).ok();
        }
    }
}

impl Focusable for OpenShiftSignIn {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for OpenShiftSignIn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_focus(window, cx);
        let colors = cx.colors().clone();
        let kv = |label: &'static str, value: String, dim: bool| {
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
                        .when(dim, |this| this.text_color(colors.text_dim))
                        .child(value),
                )
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
                        .font_family(fonts::MONO)
                        .text_size(u(12.5))
                        .child(Input::new(input).appearance(false)),
                )
        };
        let spinner = || Spinner::new().color(colors.accent).into_any_element();
        let (oauth, oauth_dim) = match &self.oauth {
            None => ("…".to_string(), true),
            Some(Ok((metadata, _))) => (metadata.issuer.clone(), false),
            Some(Err(err)) => (err.clone(), true),
        };
        let busy = matches!(self.phase, Phase::Working | Phase::Browser { .. });
        let browser_unsupported = self.browser_unsupported();

        let status = match &self.phase {
            Phase::Idle => None,
            Phase::Working => Some(status_box(
                &colors,
                spinner(),
                match self.method {
                    Method::Browser => "Contacting the OAuth server…".into(),
                    Method::Password => "Signing in…".into(),
                    Method::Token => "Checking the token…".into(),
                },
                self.auth.params.server.clone(),
                None,
            )),
            Phase::Browser { url, redirect_uri } => {
                let url = url.clone();
                Some(status_box(
                    &colors,
                    spinner(),
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
                ))
            }
            Phase::Failed(message) => Some(status_box(
                &colors,
                Icon::new(IconName::CircleX)
                    .size(18.0)
                    .color(colors.red)
                    .into_any_element(),
                "Sign-in failed".into(),
                message.clone(),
                None,
            )),
        };

        let tabs = h_flex()
            .border_1()
            .border_color(colors.border)
            .rounded(u(5.0))
            .overflow_hidden()
            .text_size(u(12.0))
            .children(Method::ALL.into_iter().map(|method| {
                let selected = method == self.method;
                let unavailable = method == Method::Browser && browser_unsupported;
                h_flex()
                    .id(SharedString::from(format!("openshift-{method:?}")))
                    .flex_1()
                    .justify_center()
                    .py(u(4.0))
                    .map(|this| {
                        if selected {
                            this.bg(colors.chip_selected_background)
                                .text_color(colors.chip_selected_text)
                        } else if unavailable {
                            this.text_color(colors.text_faint)
                        } else {
                            this.text_color(colors.text_dim)
                                .hover(|this| this.bg(colors.hover))
                                .cursor_pointer()
                        }
                    })
                    .when(!selected && !unavailable, |this| {
                        this.on_click(
                            cx.listener(move |this, _, window, cx| this.select(method, window, cx)),
                        )
                    })
                    .child(method.label())
            }));

        let note = |text: String| {
            div()
                .text_size(u(12.0))
                .text_color(colors.text_muted)
                .child(text)
        };
        let body = match self.method {
            Method::Browser => v_flex().gap(u(10.0)).children(status).child(note(
                "Like oc login --web: any identity provider of the cluster. If the browser shows \
                 an error instead of a login page, the cluster doesn't allow it (older OpenShift): \
                 use a token."
                    .into(),
            )),
            Method::Password => v_flex()
                .gap(u(10.0))
                .child(field("Username", &self.username))
                .child(field("Password", &self.password))
                .children(status)
                .child(note(
                    "Like oc login -u: for identity providers with passwords (kubeadmin, \
                     htpasswd, LDAP)."
                        .into(),
                )),
            Method::Token => v_flex()
                .gap(u(10.0))
                .child(
                    h_flex()
                        .gap(u(10.0))
                        .child(div().flex_1().min_w_0().child(note(
                            "Sign in on the token page, choose Display Token and paste the token \
                             or the whole oc login command."
                                .into(),
                        )))
                        .child(
                            Button::new("open-token-page")
                                .label("Open token page")
                                .disabled(!matches!(self.oauth, Some(Ok(_))))
                                .on_click(cx.listener(|this, _, _, _| this.open_token_page())),
                        ),
                )
                .child(field("Token", &self.token))
                .children(status)
                .when(browser_unsupported, |this| {
                    this.child(note(
                        "This cluster can't sign in through the browser directly: it has no \
                         openshift-cli-client OAuth client (older OpenShift)."
                            .into(),
                    ))
                }),
        };

        let primary = match (self.method, &self.phase) {
            (Method::Browser, Phase::Failed(_)) => Some(("Try again", false)),
            (Method::Browser, _) => None,
            (Method::Password, _) => Some(("Sign in", busy)),
            (Method::Token, _) => Some(("Use token", busy)),
        };
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
            .children(primary.map(|(label, disabled)| {
                Button::new("submit-sign-in")
                    .primary()
                    .label(label)
                    .disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
            }));

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
                            .child(kv("Server", self.auth.params.server.clone(), false))
                            .child(kv("OAuth server", oauth, oauth_dim))
                            .child(kv("User", self.auth.params.user.clone(), false)),
                    )
                    .child(tabs)
                    .child(body)
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
