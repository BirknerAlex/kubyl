//! New kubeconfigs from a service account of a connected cluster, or from a cloud CLI. Both
//! open the result as an unsaved document in the editor, where it can be tested and saved.
//!
//! Service accounts get a TokenRequest with an expiry by default; a long-lived token Secret is
//! possible but discouraged in the UI. Cloud CLIs write the same entries their
//! `get-credentials` commands write: Kubyl runs exactly those commands (after showing them)
//! into a private temp file and reads it back.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Task, Window, div, prelude::*,
};
use gpui_component::button::Button as MenuButton;
use gpui_component::input::InputState;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use k8s_openapi::api::authentication::v1::{TokenRequest, TokenRequestSpec};
use k8s_openapi::api::core::v1::{Secret, ServiceAccount};
use k8s_openapi::api::rbac::v1::{ClusterRoleBinding, RoleBinding, RoleRef, Subject};
use kube::api::{ObjectMeta, PostParams};
use kube::{Api, Client};
use kubyl_core::{ClusterId, spawn_kube};
use kubyl_kube::ConnectionManager;
use kubyl_ui::{ActiveColors, Button, Icon, IconName, fonts, h_flex, u, v_flex};
use serde_json::{Map, json};

use crate::dialogs;
use crate::files;
use crate::model::{Doc, Kind};
use crate::state::Draft;
use crate::validate::Severity;
use crate::widgets;

// ----- Service accounts: data -----

/// How the service account's token is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// A TokenRequest that expires after this many seconds (recommended).
    Request(i64),
    /// A long-lived `kubernetes.io/service-account-token` Secret.
    Secret,
}

/// Permissions to grant the service account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grant {
    None,
    /// A RoleBinding in the namespace to a built-in ClusterRole (`view`, `edit`, `admin`).
    Namespace(&'static str),
    /// A ClusterRoleBinding to `cluster-admin`.
    ClusterAdmin,
}

impl Grant {
    pub const ALL: [Grant; 5] = [
        Grant::None,
        Grant::Namespace("view"),
        Grant::Namespace("edit"),
        Grant::Namespace("admin"),
        Grant::ClusterAdmin,
    ];

    pub fn label(self) -> String {
        match self {
            Grant::None => "No binding (only what it has)".into(),
            Grant::Namespace(role) => format!("{role} in the namespace (RoleBinding)"),
            Grant::ClusterAdmin => "cluster-admin on the whole cluster (ClusterRoleBinding)".into(),
        }
    }
}

/// What a service-account request produced.
pub struct SaToken {
    pub token: String,
    pub expires: Option<jiff::Timestamp>,
    pub created: Vec<String>,
}

/// Creates (if asked) the service account, grants it, and gets a token. Runs on Tokio.
pub async fn service_account_token(
    client: Client,
    namespace: &str,
    name: &str,
    create: bool,
    kind: TokenKind,
    grant: Grant,
) -> Result<SaToken, String> {
    let describe = |err: kube::Error| match &err {
        kube::Error::Api(status) if status.code == 403 => {
            format!("not allowed: {}", status.message)
        }
        kube::Error::Api(status) => status.message.clone(),
        other => kubyl_kube::client::error_chain(other),
    };
    let accounts: Api<ServiceAccount> = Api::namespaced(client.clone(), namespace);
    let mut created = Vec::new();
    if accounts.get_opt(name).await.map_err(describe)?.is_none() {
        if !create {
            return Err(format!(
                "the service account {namespace}/{name} doesn't exist"
            ));
        }
        let sa = ServiceAccount {
            metadata: ObjectMeta {
                name: Some(name.into()),
                labels: Some(
                    [(
                        "app.kubernetes.io/managed-by".to_string(),
                        "kubyl".to_string(),
                    )]
                    .into(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        accounts
            .create(&PostParams::default(), &sa)
            .await
            .map_err(describe)?;
        created.push(format!("ServiceAccount {namespace}/{name}"));
    }
    let subject = Subject {
        kind: "ServiceAccount".into(),
        name: name.into(),
        namespace: Some(namespace.into()),
        ..Default::default()
    };
    match grant {
        Grant::None => {}
        Grant::Namespace(role) => {
            let binding = RoleBinding {
                metadata: ObjectMeta {
                    name: Some(format!("{name}-{role}")),
                    ..Default::default()
                },
                role_ref: RoleRef {
                    api_group: Some("rbac.authorization.k8s.io".into()),
                    kind: "ClusterRole".into(),
                    name: role.into(),
                },
                subjects: Some(vec![subject]),
            };
            let api: Api<RoleBinding> = Api::namespaced(client.clone(), namespace);
            if api
                .get_opt(&format!("{name}-{role}"))
                .await
                .map_err(describe)?
                .is_none()
            {
                api.create(&PostParams::default(), &binding)
                    .await
                    .map_err(describe)?;
                created.push(format!(
                    "RoleBinding {namespace}/{name}-{role} → ClusterRole {role}"
                ));
            }
        }
        Grant::ClusterAdmin => {
            let binding_name = format!("{namespace}-{name}-cluster-admin");
            let binding = ClusterRoleBinding {
                metadata: ObjectMeta {
                    name: Some(binding_name.clone()),
                    ..Default::default()
                },
                role_ref: RoleRef {
                    api_group: Some("rbac.authorization.k8s.io".into()),
                    kind: "ClusterRole".into(),
                    name: "cluster-admin".into(),
                },
                subjects: Some(vec![subject]),
            };
            let api: Api<ClusterRoleBinding> = Api::all(client.clone());
            if api
                .get_opt(&binding_name)
                .await
                .map_err(describe)?
                .is_none()
            {
                api.create(&PostParams::default(), &binding)
                    .await
                    .map_err(describe)?;
                created.push(format!("ClusterRoleBinding {binding_name} → cluster-admin"));
            }
        }
    }
    match kind {
        TokenKind::Request(seconds) => {
            let request = TokenRequest {
                spec: Some(TokenRequestSpec {
                    expiration_seconds: Some(seconds),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let response = accounts
                .create_token_request(name, &PostParams::default(), &request)
                .await
                .map_err(describe)?;
            let status = response.status.ok_or("the API server returned no token")?;
            Ok(SaToken {
                expires: status.expiration_timestamp.map(|t| t.0),
                token: status.token.ok_or("the API server returned no token")?,
                created,
            })
        }
        TokenKind::Secret => {
            let secrets: Api<Secret> = Api::namespaced(client, namespace);
            let secret_name = format!("{name}-token");
            if secrets
                .get_opt(&secret_name)
                .await
                .map_err(describe)?
                .is_none()
            {
                let secret = Secret {
                    metadata: ObjectMeta {
                        name: Some(secret_name.clone()),
                        annotations: Some(
                            [(
                                "kubernetes.io/service-account.name".to_string(),
                                name.to_string(),
                            )]
                            .into(),
                        ),
                        ..Default::default()
                    },
                    type_: Some("kubernetes.io/service-account-token".into()),
                    ..Default::default()
                };
                secrets
                    .create(&PostParams::default(), &secret)
                    .await
                    .map_err(describe)?;
                created.push(format!(
                    "Secret {namespace}/{secret_name} (long-lived token)"
                ));
            }
            // The token controller fills the Secret in.
            for _ in 0..20 {
                let secret = secrets.get(&secret_name).await.map_err(describe)?;
                if let Some(token) = secret.data.as_ref().and_then(|d| d.get("token")) {
                    let token =
                        String::from_utf8(token.0.clone()).map_err(|_| "the token isn't text")?;
                    return Ok(SaToken {
                        token,
                        expires: None,
                        created,
                    });
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err("the token Secret wasn't filled in within 10 s".into())
        }
    }
}

/// A kubeconfig for a service account: the source context's cluster (no credentials of the
/// source user), a user with the token, a context in the namespace. `file` is where `source`
/// was read from: relative paths (a CA file) are made absolute, since the new file lives
/// elsewhere.
pub fn service_account_doc(
    source: &Doc,
    file: &Path,
    source_context: &str,
    namespace: &str,
    name: &str,
    token: &str,
) -> Option<Doc> {
    let source = source.with_absolute_paths(file);
    let (cluster, _) = source.context_refs(source_context);
    let cluster_name = cluster?;
    let cluster_body = source.body(Kind::Cluster, &cluster_name)?.clone();
    let user = format!("{name}@{cluster_name}");
    let context = format!("{name}@{source_context}");
    let mut doc = Doc::empty();
    doc.add(Kind::Cluster, &cluster_name, cluster_body).ok()?;
    let mut user_body = Map::new();
    user_body.insert("token".into(), json!(token));
    doc.add(Kind::User, &user, user_body).ok()?;
    let mut context_body = Map::new();
    context_body.insert("cluster".into(), json!(cluster_name));
    context_body.insert("user".into(), json!(user));
    context_body.insert("namespace".into(), json!(namespace));
    doc.add(Kind::Context, &context, context_body).ok()?;
    doc.set_current_context(Some(&context));
    Some(doc)
}

// ----- Service accounts: dialog -----

struct SaView {
    cluster: Option<ClusterId>,
    namespace: Entity<InputState>,
    name: Entity<InputState>,
    create: bool,
    kind: TokenKind,
    grant: Grant,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
    _task: Option<Task<()>>,
}

const DURATIONS: [(i64, &str); 6] = [
    (3600, "1 hour"),
    (8 * 3600, "8 hours"),
    (24 * 3600, "1 day"),
    (7 * 24 * 3600, "7 days"),
    (30 * 24 * 3600, "30 days"),
    (365 * 24 * 3600, "1 year"),
];

/// Opens the "New from a service account" dialog for the active (or first connected) cluster.
pub fn service_account(window: &mut Window, cx: &mut App) {
    let manager = ConnectionManager::try_global(cx);
    let active = manager.as_ref().and_then(|m| {
        let m = m.read(cx);
        m.active().cloned().or_else(|| {
            m.contexts()
                .find(|c| m.state(&c.id).is_connected())
                .map(|c| c.id.clone())
        })
    });
    let namespace = active
        .as_ref()
        .and_then(|id| manager.as_ref()?.read(cx).context(id)?.namespace.clone())
        .unwrap_or_else(|| "default".into());
    let view = cx.new(|cx| SaView {
        cluster: active,
        namespace: cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("default");
            s.set_value(namespace, window, cx);
            s
        }),
        name: cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("ci-deployer");
            s.set_value("kubyl-access", window, cx);
            s
        }),
        create: true,
        kind: TokenKind::Request(8 * 3600),
        grant: Grant::Namespace("view"),
        busy: false,
        error: None,
        focus: cx.focus_handle(),
        _task: None,
    });
    let focus = view.read(cx).name.read(cx).focus_handle(cx);
    dialogs::open(view, 600.0, None, window, cx);
    window.focus(&focus, cx);
}

impl Focusable for SaView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SaView {
    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cluster) = self.cluster.clone() else {
            self.error = Some("Pick a cluster.".into());
            cx.notify();
            return;
        };
        let manager = ConnectionManager::global(cx);
        let Some(client) = manager.read(cx).client(&cluster) else {
            manager.update(cx, |m, cx| m.ensure_connected(&cluster, cx));
            self.error = Some("Connecting to the cluster… try again in a moment.".into());
            cx.notify();
            return;
        };
        if manager.read(cx).caps(&cluster).read_only {
            self.error = Some("This cluster is read-only in Kubyl.".into());
            cx.notify();
            return;
        }
        let Some(info) = manager.read(cx).context(&cluster).cloned() else {
            return;
        };
        let namespace = self.namespace.read(cx).value().trim().to_string();
        let name = self.name.read(cx).value().trim().to_string();
        if namespace.is_empty() || name.is_empty() {
            self.error = Some("Enter a namespace and a service account name.".into());
            cx.notify();
            return;
        }
        let (create, kind, grant) = (self.create, self.kind, self.grant);
        self.busy = true;
        self.error = None;
        let token_ns = namespace.clone();
        let token_name = name.clone();
        let token = spawn_kube(cx, async move {
            service_account_token(client, &token_ns, &token_name, create, kind, grant).await
        });
        let file = info.file.clone();
        let read = cx.background_executor().spawn(async move {
            files::read(&file)
                .map_err(|e| e.to_string())
                .and_then(|s| Doc::parse(&s.text))
        });
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = token.await;
            let source = read.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let doc = match (result, source) {
                    (Ok(sa), Ok(source)) => service_account_doc(
                        &source,
                        &info.file,
                        &info.context,
                        &namespace,
                        &name,
                        &sa.token,
                    )
                    .map(|doc| (doc, sa))
                    .ok_or_else(|| "the context's cluster isn't in its kubeconfig".to_string()),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                };
                match doc {
                    Ok((doc, sa)) => {
                        let mut note = format!("Kubeconfig for {namespace}/{name}");
                        if let Some(at) = sa.expires {
                            note.push_str(&format!(
                                ", token valid until {}",
                                at.strftime("%Y-%m-%d %H:%M UTC")
                            ));
                        }
                        if !sa.created.is_empty() {
                            note.push_str(&format!(". Created: {}", sa.created.join(", ")));
                        }
                        kubyl_core::NotificationCenter::push(
                            cx,
                            kubyl_core::Notification::success(note),
                        );
                        crate::actions::open_draft(
                            Draft {
                                path: files::new_owned_path(
                                    &crate::state::Kubeconfigs::dirs(cx).owned,
                                    &format!("{name}-{}", info.context),
                                ),
                                title: format!("{name}@{}", info.context),
                                doc,
                            },
                            cx,
                        );
                        use gpui_component::WindowExt as _;
                        window.close_dialog(cx);
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for SaView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let manager = ConnectionManager::global(cx);
        let contexts: Vec<(ClusterId, SharedString)> = manager
            .read(cx)
            .contexts()
            .map(|c| (c.id.clone(), manager.read(cx).display_name(&c.id)))
            .collect();
        let current = self
            .cluster
            .as_ref()
            .map(|id| manager.read(cx).display_name(id))
            .unwrap_or_else(|| "choose a cluster".into());
        let cluster_picker = {
            let weak = weak.clone();
            MenuButton::new("sa-cluster")
                .outline()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.5))
                        .child(current)
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (id, name) in &contexts {
                        let weak = weak.clone();
                        let id = id.clone();
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                            move |_, _, cx| {
                                let id = id.clone();
                                ConnectionManager::global(cx)
                                    .update(cx, |m, cx| m.ensure_connected(&id, cx));
                                weak.update(cx, |this, cx| {
                                    this.cluster = Some(id);
                                    cx.notify();
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                })
        };
        let kind = self.kind;
        let token_seg = {
            let weak = weak.clone();
            widgets::segmented(
                "sa-kind",
                vec![
                    (
                        true,
                        SharedString::from("TokenRequest (expires)"),
                        Some(IconName::Clock),
                    ),
                    (false, "Long-lived token Secret".into(), Some(IconName::Key)),
                ],
                matches!(kind, TokenKind::Request(_)),
                &colors,
                move |request, _, cx| {
                    let request = *request;
                    weak.update(cx, |this, cx| {
                        this.kind = if request {
                            TokenKind::Request(8 * 3600)
                        } else {
                            TokenKind::Secret
                        };
                        cx.notify();
                    })
                    .ok();
                },
            )
        };
        let duration = match kind {
            TokenKind::Request(seconds) => {
                let weak = weak.clone();
                let label = DURATIONS
                    .iter()
                    .find(|(s, _)| *s == seconds)
                    .map(|(_, l)| *l)
                    .unwrap_or("custom");
                Some(
                    MenuButton::new("sa-duration")
                        .outline()
                        .child(
                            h_flex()
                                .gap(u(6.0))
                                .text_size(u(12.5))
                                .child(format!("valid for {label}"))
                                .child(Icon::new(IconName::ChevronDown).size(11.0)),
                        )
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            for (seconds, label) in DURATIONS {
                                let weak = weak.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(
                                    move |_, _, cx| {
                                        weak.update(cx, |this, cx| {
                                            this.kind = TokenKind::Request(seconds);
                                            cx.notify();
                                        })
                                        .ok();
                                    },
                                ));
                            }
                            menu
                        }),
                )
            }
            TokenKind::Secret => None,
        };
        let grant = self.grant;
        let grant_picker = {
            let weak = weak.clone();
            MenuButton::new("sa-grant")
                .outline()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(12.5))
                        .child(grant.label())
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for g in Grant::ALL {
                        let weak = weak.clone();
                        menu =
                            menu.item(PopupMenuItem::new(g.label()).on_click(move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.grant = g;
                                    cx.notify();
                                })
                                .ok();
                            }));
                    }
                    menu
                })
        };
        let create = self.create;
        let run_weak = weak.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(dialogs::header(IconName::User, "New kubeconfig from a service account", None, &colors))
            .child(
                v_flex()
                    .gap(u(12.0))
                    .p(u(16.0))
                    .child(widgets::row("Cluster", cluster_picker, &colors))
                    .child(widgets::row("Namespace", widgets::text_input(&self.namespace, true, false, &colors), &colors))
                    .child(widgets::row("Service account", widgets::text_input(&self.name, true, false, &colors), &colors))
                    .child(div().pl(u(162.0)).child(widgets::checkbox("sa-create", create, "Create it if it doesn't exist", &colors, {
                        let weak = weak.clone();
                        move |_, _, cx| {
                            weak.update(cx, |this, cx| {
                                this.create = !this.create;
                                cx.notify();
                            })
                            .ok();
                        }
                    })))
                    .child(widgets::row("Token", v_flex().gap(u(6.0)).child(token_seg).children(duration), &colors))
                    .child(match kind {
                        TokenKind::Request(_) => widgets::hint(
                            "Recommended: the token expires, and deleting the service account revokes it. Create a new kubeconfig when it runs out.",
                            &colors,
                        ),
                        TokenKind::Secret => widgets::notice(
                            Severity::Warning,
                            "A token Secret never expires and anyone who can read Secrets in this namespace can copy it. Prefer a TokenRequest with an expiry.",
                            &colors,
                        )
                        .into_any_element(),
                    })
                    .child(widgets::row("Permissions", grant_picker, &colors))
                    .when(grant == Grant::ClusterAdmin, |this| {
                        this.child(widgets::notice(Severity::Warning, "cluster-admin can do anything on the whole cluster.", &colors))
                    })
                    .children(self.error.clone().map(|e| widgets::notice(Severity::Error, e, &colors))),
            )
            .child(dialogs::footer(
                Some(widgets::hint("Opens the kubeconfig in the editor: test it, then save.", &colors)),
                vec![
                    dialogs::cancel_button("sa-cancel"),
                    Button::new("sa-run")
                        .primary()
                        .label(if self.busy { "Creating…" } else { "Create kubeconfig" })
                        .disabled(self.busy)
                        .on_click(move |_, window, cx| {
                            run_weak.update(cx, |this, cx| this.run(window, cx)).ok();
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

// ----- Cloud CLIs -----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cloud {
    Eks,
    Gke,
    Aks,
}

impl Cloud {
    pub const ALL: [Cloud; 3] = [Cloud::Eks, Cloud::Gke, Cloud::Aks];

    pub fn label(self) -> &'static str {
        match self {
            Cloud::Eks => "AWS EKS",
            Cloud::Gke => "Google GKE",
            Cloud::Aks => "Azure AKS",
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            Cloud::Eks => "aws",
            Cloud::Gke => "gcloud",
            Cloud::Aks => "az",
        }
    }

    /// The fields: (id, label, placeholder, required).
    fn fields(self) -> &'static [(&'static str, &'static str, &'static str, bool)] {
        match self {
            Cloud::Eks => &[
                ("name", "Cluster name", "prod-eu", true),
                ("region", "Region", "eu-west-1", true),
                ("profile", "Profile", "default (optional)", false),
            ],
            Cloud::Gke => &[
                ("name", "Cluster name", "analytics", true),
                ("location", "Location", "europe-west4", true),
                ("project", "Project", "the gcloud default (optional)", false),
            ],
            Cloud::Aks => &[
                ("name", "Cluster name", "aks-prod", true),
                ("group", "Resource group", "rg-prod", true),
                (
                    "subscription",
                    "Subscription",
                    "the az default (optional)",
                    false,
                ),
            ],
        }
    }
}

/// The command that writes the cluster's entries into `file`, like the CLI's own
/// get-credentials. Returns (program, args, env).
pub fn cloud_command(
    cloud: Cloud,
    values: &std::collections::HashMap<&str, String>,
    file: &Path,
) -> (String, Vec<String>, Vec<(String, String)>) {
    let get = |k: &str| {
        values
            .get(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let file = file.display().to_string();
    match cloud {
        Cloud::Eks => {
            let mut args = vec![
                "eks".into(),
                "update-kubeconfig".into(),
                "--name".into(),
                get("name").unwrap_or_default(),
                "--region".into(),
                get("region").unwrap_or_default(),
                "--kubeconfig".into(),
                file,
            ];
            if let Some(profile) = get("profile") {
                args.push("--profile".into());
                args.push(profile);
            }
            ("aws".into(), args, Vec::new())
        }
        Cloud::Gke => {
            let mut args = vec![
                "container".into(),
                "clusters".into(),
                "get-credentials".into(),
                get("name").unwrap_or_default(),
                "--location".into(),
                get("location").unwrap_or_default(),
            ];
            if let Some(project) = get("project") {
                args.push("--project".into());
                args.push(project);
            }
            ("gcloud".into(), args, vec![("KUBECONFIG".into(), file)])
        }
        Cloud::Aks => {
            let mut args = vec![
                "aks".into(),
                "get-credentials".into(),
                "--name".into(),
                get("name").unwrap_or_default(),
                "--resource-group".into(),
                get("group").unwrap_or_default(),
                "--file".into(),
                file,
            ];
            if let Some(subscription) = get("subscription") {
                args.push("--subscription".into());
                args.push(subscription);
            }
            ("az".into(), args, Vec::new())
        }
    }
}

/// Runs a cloud CLI into a private temp folder and reads the kubeconfig it wrote.
async fn run_cloud(
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    dir: PathBuf,
) -> Result<Doc, String> {
    let path = kubyl_kube::auth::shell_env::path();
    let resolved = crate::validate::find_command(&program, None).ok_or_else(|| {
        format!("`{program}` isn't installed (or not in your login shell's PATH)")
    })?;
    let mut cmd = tokio::process::Command::new(resolved);
    cmd.args(&args)
        .envs(env)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::time::timeout(Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| format!("`{program}` didn't finish within 3 minutes"))?
        .map_err(|e| format!("couldn't run `{program}`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`{program}` failed: {}", stderr.trim()));
    }
    let file = dir.join("config");
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("`{program}` wrote no kubeconfig: {e}"))?;
    Doc::parse(&text)
}

struct CloudView {
    cloud: Cloud,
    inputs: std::collections::HashMap<&'static str, Entity<InputState>>,
    installed: std::collections::HashMap<&'static str, bool>,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
}

/// Opens the "Import from a cloud CLI" dialog.
pub fn cloud(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        let mut inputs = std::collections::HashMap::new();
        for cloud in Cloud::ALL {
            for (id, _, placeholder, _) in cloud.fields() {
                let key: &'static str = match (cloud, *id) {
                    (Cloud::Eks, "name") => "eks-name",
                    (Cloud::Gke, "name") => "gke-name",
                    (Cloud::Aks, "name") => "aks-name",
                    (_, other) => other,
                };
                let placeholder = placeholder.to_string();
                let state = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
                inputs.insert(key, state);
            }
        }
        let lookup = cx.background_executor().spawn(async {
            Cloud::ALL
                .iter()
                .map(|c| {
                    (
                        c.program(),
                        crate::validate::find_command(c.program(), None).is_some(),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>()
        });
        let task = cx.spawn(async move |this: gpui::WeakEntity<CloudView>, cx| {
            let installed = lookup.await;
            this.update(cx, |this, cx| {
                this.installed = installed;
                cx.notify();
            })
            .ok();
        });
        CloudView {
            cloud: Cloud::Eks,
            inputs,
            installed: std::collections::HashMap::new(),
            busy: false,
            error: None,
            focus: cx.focus_handle(),
            _tasks: vec![task],
        }
    });
    let focus = view.read(cx).focus.clone();
    dialogs::open(view, 600.0, Some(focus), window, cx);
}

impl Focusable for CloudView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CloudView {
    fn key(&self, id: &'static str) -> &'static str {
        match (self.cloud, id) {
            (Cloud::Eks, "name") => "eks-name",
            (Cloud::Gke, "name") => "gke-name",
            (Cloud::Aks, "name") => "aks-name",
            (_, other) => other,
        }
    }

    fn values(&self, cx: &App) -> std::collections::HashMap<&'static str, String> {
        self.cloud
            .fields()
            .iter()
            .map(|(id, ..)| (*id, self.inputs[self.key(id)].read(cx).value().to_string()))
            .collect()
    }

    fn preview(&self, cx: &App) -> String {
        let (program, args, env) =
            cloud_command(self.cloud, &self.values(cx), Path::new("<temp>/config"));
        let env: String = env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
        format!("{env}{program} {}", args.join(" "))
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let values = self.values(cx);
        if self
            .cloud
            .fields()
            .iter()
            .any(|(id, _, _, required)| *required && values[id].trim().is_empty())
        {
            self.error = Some("Fill in the required fields.".into());
            cx.notify();
            return;
        }
        let dir = match tempfile::Builder::new().prefix("kubyl-cloud-").tempdir() {
            Ok(dir) => dir,
            Err(err) => {
                self.error = Some(format!("Couldn't create a temp folder: {err}"));
                cx.notify();
                return;
            }
        };
        let (program, args, env) = cloud_command(self.cloud, &values, &dir.path().join("config"));
        self.busy = true;
        self.error = None;
        let dir_path = dir.path().to_path_buf();
        let run = spawn_kube(cx, async move {
            let result = run_cloud(program, args, env, dir_path).await;
            // The temp folder (and any credential the CLI wrote there) goes away here.
            drop(dir);
            result
        });
        let name = values.get("name").cloned().unwrap_or_default();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = run.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(doc) => {
                        crate::actions::open_draft(
                            Draft {
                                path: files::new_owned_path(
                                    &crate::state::Kubeconfigs::dirs(cx).owned,
                                    &name,
                                ),
                                title: name.clone(),
                                doc,
                            },
                            cx,
                        );
                        use gpui_component::WindowExt as _;
                        window.close_dialog(cx);
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for CloudView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let cloud = self.cloud;
        let seg = {
            let weak = weak.clone();
            widgets::segmented(
                "cloud-kind",
                Cloud::ALL
                    .iter()
                    .map(|c| (*c, SharedString::from(c.label()), Some(IconName::Cloud)))
                    .collect(),
                cloud,
                &colors,
                move |c, _, cx| {
                    let c = *c;
                    weak.update(cx, |this, cx| {
                        this.cloud = c;
                        this.error = None;
                        cx.notify();
                    })
                    .ok();
                },
            )
        };
        let installed = self.installed.get(cloud.program()).copied();
        let mut body = v_flex().gap(u(12.0)).p(u(16.0)).child(seg);
        for (id, label, _, required) in cloud.fields() {
            let state = &self.inputs[self.key(id)];
            body = body.child(widgets::row(
                if *required {
                    label.to_string()
                } else {
                    format!("{label} (optional)")
                },
                widgets::text_input(state, true, false, &colors),
                &colors,
            ));
        }
        body = body
            .child(
                v_flex()
                    .gap(u(4.0))
                    .child(div().text_size(u(11.5)).text_color(colors.text_dim).child("Kubyl runs, with your login shell's PATH:"))
                    .child(
                        div()
                            .p(u(8.0))
                            .rounded(u(5.0))
                            .bg(colors.input_background)
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(self.preview(cx)),
                    ),
            )
            .child(widgets::hint(
                "The result is exactly what the CLI's get-credentials writes (its exec plugin included), opened as a new kubeconfig you can test and save.",
                &colors,
            ))
            .when(installed == Some(false), |this| {
                this.child(widgets::notice(Severity::Warning, format!("`{}` isn't installed (or not in your login shell's PATH).", cloud.program()), &colors))
            })
            .children(self.error.clone().map(|e| widgets::notice(Severity::Error, e, &colors)));
        let run_weak = weak.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(dialogs::header(
                IconName::Cloud,
                "Import from a cloud CLI",
                None,
                &colors,
            ))
            .child(body)
            .child(dialogs::footer(
                None,
                vec![
                    dialogs::cancel_button("cloud-cancel"),
                    Button::new("cloud-run")
                        .primary()
                        .label(if self.busy {
                            "Running…"
                        } else {
                            "Get credentials"
                        })
                        .disabled(self.busy || installed == Some(false))
                        .on_click(move |_, window, cx| {
                            run_weak.update(cx, |this, cx| this.run(window, cx)).ok();
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_account_kubeconfigs_make_relative_paths_absolute() {
        let source = Doc::parse(
            "clusters:\n- name: kind\n  cluster: {server: \"https://127.0.0.1:6443\", certificate-authority: certs/ca.pem}\ncontexts:\n- name: kind-dev\n  context: {cluster: kind, user: admin}\nusers:\n- name: admin\n  user: {token: x}\n",
        )
        .unwrap();
        let file = Path::new("/home/me/.kube/config");
        let doc = service_account_doc(&source, file, "kind-dev", "ci", "deployer", "t0k").unwrap();
        assert_eq!(
            crate::model::get_str(
                doc.body(Kind::Cluster, "kind").unwrap(),
                &["certificate-authority"]
            ),
            Path::new("/home/me/.kube")
                .join("certs/ca.pem")
                .to_string_lossy()
        );
    }

    #[test]
    fn service_account_kubeconfigs_copy_the_cluster_only() {
        let source = Doc::parse(
            "clusters:\n- name: kind\n  cluster: {server: \"https://127.0.0.1:6443\", certificate-authority-data: QUJD}\ncontexts:\n- name: kind-dev\n  context: {cluster: kind, user: admin}\nusers:\n- name: admin\n  user: {client-key-data: S0VZ, client-certificate-data: Q0VSVA==}\n",
        )
        .unwrap();
        let doc = service_account_doc(
            &source,
            Path::new("/home/me/.kube/config"),
            "kind-dev",
            "ci",
            "deployer",
            "t0k",
        )
        .unwrap();
        assert_eq!(doc.names(Kind::User), ["deployer@kind"]);
        assert_eq!(
            doc.body(Kind::User, "deployer@kind").unwrap()["token"],
            "t0k"
        );
        assert_eq!(
            doc.body(Kind::Cluster, "kind").unwrap()["certificate-authority-data"],
            "QUJD"
        );
        assert_eq!(doc.current_context(), Some("deployer@kind-dev"));
        assert_eq!(
            crate::model::get_str(
                doc.body(Kind::Context, "deployer@kind-dev").unwrap(),
                &["namespace"]
            ),
            "ci"
        );
        // Nothing of the admin user.
        assert!(!doc.to_yaml().contains("S0VZ"));
    }

    #[test]
    fn cloud_commands_match_get_credentials() {
        let values: std::collections::HashMap<&str, String> = [
            ("name", "prod".to_string()),
            ("region", "eu-west-1".into()),
            ("profile", "".into()),
        ]
        .into();
        let (program, args, env) = cloud_command(Cloud::Eks, &values, Path::new("/t/config"));
        assert_eq!(program, "aws");
        assert_eq!(
            args.join(" "),
            "eks update-kubeconfig --name prod --region eu-west-1 --kubeconfig /t/config"
        );
        assert!(env.is_empty());
        let values: std::collections::HashMap<&str, String> = [
            ("name", "a".to_string()),
            ("location", "europe-west4".into()),
            ("project", "p".into()),
        ]
        .into();
        let (_, args, env) = cloud_command(Cloud::Gke, &values, Path::new("/t/config"));
        assert_eq!(
            args.join(" "),
            "container clusters get-credentials a --location europe-west4 --project p"
        );
        assert_eq!(env, [("KUBECONFIG".to_string(), "/t/config".to_string())]);
        let values: std::collections::HashMap<&str, String> = [
            ("name", "a".to_string()),
            ("group", "rg".into()),
            ("subscription", "".into()),
        ]
        .into();
        let (_, args, _) = cloud_command(Cloud::Aks, &values, Path::new("/t/config"));
        assert_eq!(
            args.join(" "),
            "aks get-credentials --name a --resource-group rg --file /t/config"
        );
    }
}
