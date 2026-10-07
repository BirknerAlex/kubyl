//! The repositories editor (board 20): Helm's own repository list (`helm repo list`), update one
//! or all, remove, add an HTTP repository (basic auth, CA, client certificate) and log in to an
//! OCI registry. Kubyl keeps no repository credentials: Helm stores them in its
//! `repositories.yaml` (plain text, the dialog says so), registry logins go to Docker's
//! credential store. Passwords reach `helm` on stdin.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    Subscription, Task, Window, div, prelude::*,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::{Notification, NotificationCenter, spawn_kube};
use kubyl_helm_core::cli::{self, HelmInfo};
use kubyl_helm_core::repo::{self, NewRepository, Repository};
use kubyl_ui::{ActiveColors, Button, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{checkbox, error_line, footer, form_row, header, input_box, open};
use crate::cli::HelmCli;
use crate::widgets;

/// Opens the repositories editor.
pub fn open_repositories(window: &mut Window, cx: &mut App) {
    let Some(helm) = HelmCli::info(cx) else {
        super::open_missing(window, cx);
        return;
    };
    let view = cx.new(|cx| RepositoriesDialog::new(helm, window, cx));
    let focus = view.read(cx).focus.clone();
    open(view, 960.0, Some(focus), window, cx);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Http,
    Oci,
}

enum List {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Vec<Repository>),
    Failed(String),
}

pub struct RepositoriesDialog {
    helm: HelmInfo,
    list: List,
    form: Form,
    name: Entity<InputState>,
    url: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    ca_file: Entity<InputState>,
    cert_file: Entity<InputState>,
    key_file: Entity<InputState>,
    registry: Entity<InputState>,
    insecure: bool,
    /// What runs (`update`, `add`, `remove <name>`, `login`), one at a time.
    busy: Option<(String, Task<()>)>,
    /// A destructive step that waits for a second click (`replace <name>`, `remove <name>`).
    confirm: Option<String>,
    error: Option<String>,
    pub(crate) focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl RepositoriesDialog {
    fn new(helm: HelmInfo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let make =
            |placeholder: &str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
                let placeholder = placeholder.to_string();
                cx.new(|cx| {
                    let state = InputState::new(window, cx).placeholder(placeholder);
                    if masked { state.masked(true) } else { state }
                })
            };
        let name = make("private", false, window, cx);
        let url = make("https://charts.example.com/stable", false, window, cx);
        let username = make("optional", false, window, cx);
        let password = make("optional", true, window, cx);
        let ca_file = make("optional: CA bundle (PEM file)", false, window, cx);
        let cert_file = make("optional: client certificate file", false, window, cx);
        let key_file = make("optional: client key file", false, window, cx);
        let registry = make("ghcr.io", false, window, cx);
        let subscriptions = [&name, &url, &username, &password, &registry]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Self, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.error = None;
                        this.confirm = None;
                        cx.notify();
                    }
                })
            })
            .collect();
        let mut this = Self {
            helm,
            list: List::Ready(Vec::new()),
            form: Form::Http,
            name,
            url,
            username,
            password,
            ca_file,
            cert_file,
            key_file,
            registry,
            insecure: false,
            busy: None,
            confirm: None,
            error: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let helm = self.helm.clone();
        let work = spawn_kube(cx, async move {
            match cli::run(&helm, None, repo::list_repositories(), None, None).await {
                Ok(output) => repo::parse_repositories(&output.stdout),
                Err(err) if repo::is_no_repositories(&err.message) => Ok(Vec::new()),
                Err(err) => Err(err.message),
            }
        });
        self.list = List::Loading(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.list = match result {
                    Ok(repos) => List::Ready(repos),
                    Err(err) => List::Failed(err),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Runs a repository command, then reloads the list and tells the Charts tab.
    fn run(
        &mut self,
        what: String,
        invocation: cli::Invocation,
        done: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        let helm = self.helm.clone();
        let work = spawn_kube(cx, async move {
            cli::run(&helm, None, invocation, None, None).await
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                // The password went to `helm` either way: the field doesn't keep it.
                this.clear_secrets(window, cx);
                match result {
                    Ok(_) => {
                        NotificationCenter::push(cx, Notification::info(done));
                        crate::charts::repositories_changed(cx);
                    }
                    Err(err) => this.error = Some(err.message),
                }
                this.reload(cx);
            })
            .ok();
        });
        self.busy = Some((what, task));
        self.error = None;
        self.confirm = None;
        cx.notify();
    }

    /// Whether `step` was asked for twice; the first time explains what it does.
    fn confirmed(&mut self, step: String, explain: String, cx: &mut Context<Self>) -> bool {
        if self.confirm.as_ref() == Some(&step) {
            return true;
        }
        self.confirm = Some(step);
        self.error = Some(explain);
        cx.notify();
        false
    }

    fn remove(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirmed(
            format!("remove {name}"),
            format!("Remove {name} from Helm's repositories? Click its remove button again."),
            cx,
        ) {
            return;
        }
        self.run(
            format!("remove {name}"),
            repo::remove_repository(&name),
            format!("Removed the Helm repository {name}."),
            window,
            cx,
        )
    }

    /// The password fields don't keep a value once it went to `helm`.
    fn clear_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.password
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    fn value(input: &Entity<InputState>, cx: &App) -> Option<String> {
        let value = input.read(cx).value().trim().to_string();
        (!value.is_empty()).then_some(value)
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        let url = self.url.read(cx).value().trim().to_string();
        if let Err(err) =
            repo::validate_repository_name(&name).and_then(|_| repo::validate_repository_url(&url))
        {
            self.error = Some(err);
            cx.notify();
            return;
        }
        let exists =
            matches!(&self.list, List::Ready(repos) if repos.iter().any(|r| r.name == name));
        if exists
            && !self.confirmed(
                format!("replace {name}"),
                format!(
                    "A repository named {name} exists: Add again to replace its URL and credentials."
                ),
                cx,
            )
        {
            return;
        }
        let username = Self::value(&self.username, cx);
        let password = Self::value(&self.password, cx).map(Into::into);
        if password.is_some() && username.is_none() {
            self.error = Some("A password needs a user name.".into());
            cx.notify();
            return;
        }
        let new = NewRepository {
            name: name.clone(),
            url,
            username,
            password,
            ca_file: Self::value(&self.ca_file, cx).map(Into::into),
            cert_file: Self::value(&self.cert_file, cx).map(Into::into),
            key_file: Self::value(&self.key_file, cx).map(Into::into),
            insecure_skip_tls_verify: self.insecure,
        };
        self.run(
            "add".into(),
            repo::add_repository(&new),
            format!("Added the Helm repository {name}."),
            window,
            cx,
        );
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(host), Some(username), Some(password)) = (
            Self::value(&self.registry, cx),
            Self::value(&self.username, cx),
            Self::value(&self.password, cx),
        ) else {
            self.error = Some("Fill in the registry, user name and password.".into());
            cx.notify();
            return;
        };
        if let Err(err) = repo::validate_registry_host(&host) {
            self.error = Some(err);
            cx.notify();
            return;
        }
        self.run(
            "login".into(),
            repo::registry_login(
                &host,
                &username,
                password.into(),
                self.insecure,
                self.helm.version,
            ),
            format!("Logged in to {host}."),
            window,
            cx,
        );
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let busy = self.busy.as_ref().map(|(w, _)| w.clone());
        let rows: AnyElement = match &self.list {
            List::Loading(_) => widgets::muted("Reading helm repo list…", &colors),
            List::Failed(err) => {
                widgets::note(IconName::TriangleAlert, colors.yellow, err.clone(), &colors)
            }
            List::Ready(repos) if repos.is_empty() => widgets::muted(
                "No repositories yet. Add one, or type oci:// references into the Charts search.",
                &colors,
            ),
            List::Ready(repos) => v_flex()
                .children(
                    repos
                        .iter()
                        .enumerate()
                        .map(|(i, repository)| {
                            let update_name = repository.name.clone();
                            let remove_name = repository.name.clone();
                            let removing =
                                busy.as_deref() == Some(&format!("remove {}", repository.name));
                            h_flex()
                                .gap(u(8.0))
                                .py(u(5.0))
                                .px(u(4.0))
                                .border_b_1()
                                .border_color(colors.border_variant)
                                .text_size(u(12.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .w(u(160.0))
                                        .truncate()
                                        .font_family(fonts::MONO)
                                        .child(repository.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(fonts::MONO)
                                        .text_size(u(11.5))
                                        .text_color(colors.text_muted)
                                        .child(kubyl_helm_core::cli::scrub(&repository.url)),
                                )
                                .child(
                                    kubyl_ui::IconButton::new(
                                        ("repo-update", i),
                                        IconName::RefreshCw,
                                    )
                                    .icon_size(12.0)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.run(
                                                format!("update {update_name}"),
                                                repo::update_repositories(std::slice::from_ref(
                                                    &update_name,
                                                )),
                                                format!("Updated {update_name}."),
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                                .child(
                                    kubyl_ui::IconButton::new(("repo-remove", i), IconName::Trash)
                                        .icon_size(12.0)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.remove(remove_name.clone(), window, cx)
                                        })),
                                )
                                .when(removing, |this| this.opacity(0.5))
                        })
                        .collect::<Vec<_>>(),
                )
                .into_any_element(),
        };
        let count = match &self.list {
            List::Ready(repos) => repos.len(),
            _ => 0,
        };
        let config = self
            .helm
            .env
            .repository_config
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Helm's repositories.yaml".into());
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .p(u(16.0))
            .gap(u(6.0))
            .border_r_1()
            .border_color(colors.border_variant)
            .child(
                h_flex()
                    .child(div().flex_1().child(widgets::dialog_title(
                        format!("Repositories · {count}"),
                        &colors,
                    )))
                    .child(
                        Button::new("repos-update-all")
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label(if busy.as_deref() == Some("update") {
                                "Updating…"
                            } else {
                                "Update all"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run(
                                    "update".into(),
                                    repo::update_repositories(&[]),
                                    "Updated the Helm repositories.".into(),
                                    window,
                                    cx,
                                )
                            })),
                    ),
            )
            .child(
                div()
                    .id("repos-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(rows),
            )
            .child(widgets::muted(
                format!("The same list as helm repo list: {config}"),
                &colors,
            ))
            .into_any_element()
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let config = self
            .helm
            .env
            .repository_config
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "repositories.yaml".into());
        let tabs = h_flex()
            .gap(u(6.0))
            .child(widgets::toggle_chip(
                "repos-form-http",
                "HTTP repository",
                self.form == Form::Http,
                cx.listener(|this, _, _, cx| {
                    this.form = Form::Http;
                    cx.notify();
                }),
            ))
            .child(widgets::toggle_chip(
                "repos-form-oci",
                "OCI registry login",
                self.form == Form::Oci,
                cx.listener(|this, _, _, cx| {
                    this.form = Form::Oci;
                    cx.notify();
                }),
            ));
        let insecure = checkbox(
            "repos-insecure",
            self.insecure,
            "Skip TLS verification (insecure)",
            &colors,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.insecure = !this.insecure;
            cx.notify();
        }));
        let busy = self.busy.is_some();
        let form = match self.form {
            Form::Http => v_flex()
                .gap(u(8.0))
                .child(form_row("Name", input_box(&self.name, true, &colors), &colors))
                .child(form_row("URL", input_box(&self.url, true, &colors), &colors))
                .child(form_row("User name", input_box(&self.username, false, &colors), &colors))
                .child(form_row("Password", input_box(&self.password, false, &colors), &colors))
                .child(form_row("CA file", input_box(&self.ca_file, true, &colors), &colors))
                .child(form_row("Client cert", input_box(&self.cert_file, true, &colors), &colors))
                .child(form_row("Client key", input_box(&self.key_file, true, &colors), &colors))
                .child(insecure)
                .child(
                    h_flex()
                        .items_start()
                        .gap(u(8.0))
                        .p(u(8.0))
                        .rounded(u(6.0))
                        .bg(colors.yellow.opacity(0.08))
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(Icon::new(IconName::TriangleAlert).size(13.0).color(colors.yellow))
                        .child(div().flex_1().min_w_0().child(format!(
                            "Helm stores a repository's user name and password in plain text in {config}. Kubyl keeps no copy. Prefer credentials that can only read charts."
                        ))),
                )
                .child(
                    h_flex().justify_end().child(
                        Button::new("repos-add")
                            .primary()
                            .icon(IconName::Plus)
                            .label(if busy { "Working…" } else { "Add repository" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
                ),
            Form::Oci => v_flex()
                .gap(u(8.0))
                .child(form_row("Registry", input_box(&self.registry, true, &colors), &colors))
                .child(form_row("User name", input_box(&self.username, false, &colors), &colors))
                .child(form_row("Password", input_box(&self.password, false, &colors), &colors))
                .child(insecure)
                .child(widgets::muted(
                    "helm registry login: the login goes to Docker's credential store (or Helm's registry config). Type oci:// references into the Charts search.",
                    &colors,
                ))
                .child(
                    h_flex().justify_end().child(
                        Button::new("repos-login")
                            .icon(IconName::Key)
                            .label(if busy { "Working…" } else { "Log in" })
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.login(window, cx))),
                    ),
                ),
        };
        v_flex()
            .id("repos-form")
            .flex_none()
            .w(u(420.0))
            .h_full()
            .p(u(16.0))
            .gap(u(10.0))
            .overflow_y_scroll()
            .child(tabs)
            .child(form)
            .into_any_element()
    }
}

impl Focusable for RepositoriesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RepositoriesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let list = self.render_list(cx);
        let form = self.render_form(cx);
        v_flex()
            .track_focus(&self.focus)
            .key_context("HelmDialog")
            .text_color(colors.text)
            .text_size(u(13.0))
            .child(header(
                Icon::new(IconName::Settings)
                    .size(16.0)
                    .color(colors.accent)
                    .into_any_element(),
                "Helm repositories",
                Vec::new(),
                &colors,
            ))
            .child(h_flex().h(u(470.0)).items_start().child(list).child(form))
            .child(footer(
                error_line(&self.error, &colors),
                vec![super::close_button("Close")],
                &colors,
            ))
    }
}
