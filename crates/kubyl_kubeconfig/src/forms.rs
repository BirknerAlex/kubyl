//! The form on the right of the editor: one per entry kind (board 11). Text fields write into
//! the document as you type; pickers, switches and presets act on click. Secret fields are
//! masked (the eye reveals them); copying one is a separate button.

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight, IntoElement,
    SharedString, Subscription, Window, div, prelude::*,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::{Notification, NotificationCenter};
use kubyl_kube::ConnectionManager;
use kubyl_kube::settings::ColorTag;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};
use serde_json::{Map, Value, json};

use crate::certs;
use crate::editor::KubeconfigEditor;
use crate::model::{self, AuthKind, EntryRef, ExecPreset, Kind, PemSource};
use crate::state::{Kubeconfigs, TestKey};
use crate::validate::{self, Severity};
use crate::widgets;

/// Where a CA or client certificate comes from in the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PemMode {
    System,
    File,
    Pem,
}

/// The inputs of the selected entry.
#[derive(Default)]
pub struct Form {
    pub entry: Option<EntryRef>,
    pub inputs: HashMap<&'static str, Entity<InputState>>,
    pub areas: HashMap<&'static str, Entity<TextareaState>>,
    /// Exec env rows: name and value.
    pub env: Vec<(Entity<InputState>, Entity<InputState>)>,
    /// PEM sources chosen before a value exists.
    pub modes: HashMap<&'static str, PemMode>,
    _subscriptions: Vec<Subscription>,
}

/// Text fields and where they write.
fn binding(id: &str) -> Option<(Kind, &'static [&'static str])> {
    Some(match id {
        "server" => (Kind::Cluster, &["server"]),
        "tls-server-name" => (Kind::Cluster, &["tls-server-name"]),
        "proxy-url" => (Kind::Cluster, &["proxy-url"]),
        "ca-file" => (Kind::Cluster, &[model::CA_FILE]),
        "namespace" => (Kind::Context, &["namespace"]),
        "token" => (Kind::User, &["token"]),
        "token-file" => (Kind::User, &["tokenFile"]),
        "cert-file" => (Kind::User, &[model::CERT_FILE]),
        "key-file" => (Kind::User, &[model::KEY_FILE]),
        "key-data" => (Kind::User, &[model::KEY_DATA]),
        "exec-command" => (Kind::User, &["exec", "command"]),
        "exec-hint" => (Kind::User, &["exec", "installHint"]),
        "oidc-issuer" => (Kind::User, &["auth-provider", "config", "idp-issuer-url"]),
        "oidc-client-id" => (Kind::User, &["auth-provider", "config", "client-id"]),
        "oidc-client-secret" => (Kind::User, &["auth-provider", "config", "client-secret"]),
        "oidc-scopes" => (Kind::User, &["auth-provider", "config", "extra-scopes"]),
        "username" => (Kind::User, &["username"]),
        "password" => (Kind::User, &["password"]),
        _ => return None,
    })
}

/// Fields that hold secrets: masked, with an eye to reveal.
fn is_secret(id: &str) -> bool {
    matches!(id, "token" | "key-data" | "oidc-client-secret" | "password")
}

impl Form {
    /// Inputs for the editor's selection with the document's current values.
    pub fn build(
        editor: &KubeconfigEditor,
        window: &mut Window,
        cx: &mut Context<KubeconfigEditor>,
    ) -> Self {
        let mut form = Form {
            entry: editor.selection.clone(),
            ..Default::default()
        };
        let Some(entry) = editor.selection.clone() else {
            return form;
        };
        let Some(body) = editor.doc.body(entry.kind, &entry.name).cloned() else {
            return form;
        };
        let ids: &[&'static str] = match entry.kind {
            Kind::Cluster => &["server", "tls-server-name", "proxy-url", "ca-file"],
            Kind::Context => &["namespace"],
            Kind::User => &[
                "token",
                "token-file",
                "cert-file",
                "key-file",
                "key-data",
                "exec-command",
                "exec-hint",
                "oidc-issuer",
                "oidc-client-id",
                "oidc-client-secret",
                "oidc-scopes",
                "username",
                "password",
            ],
        };
        for id in ids {
            let (_, path) = binding(id).expect("bound");
            let value = model::get_str(&body, path);
            form.input(id, value, is_secret(id), placeholder(id), window, cx);
        }
        // Multi-line: PEM text of inline certificates, exec args.
        let pem = |key: &str| {
            let data = model::get_str(&body, &[key]);
            if data.is_empty() {
                String::new()
            } else {
                certs::pem_from_data(&data).unwrap_or(data)
            }
        };
        match entry.kind {
            Kind::Cluster => form.area(
                "ca-pem",
                pem(model::CA_DATA),
                "-----BEGIN CERTIFICATE-----",
                window,
                cx,
            ),
            Kind::User => {
                form.area(
                    "cert-pem",
                    pem(model::CERT_DATA),
                    "-----BEGIN CERTIFICATE-----",
                    window,
                    cx,
                );
                let args = model::get(&body, &["exec", "args"])
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|v| {
                                v.as_str()
                                    .map(String::from)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                form.area("exec-args", args, "one argument per line", window, cx);
                let env: Vec<(String, String)> = model::get(&body, &["exec", "env"])
                    .and_then(Value::as_array)
                    .map(|vars| {
                        vars.iter()
                            .map(|v| {
                                (
                                    v.get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    v.get("value")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (name, value) in env {
                    form.env_row(name, value, window, cx);
                }
            }
            Kind::Context => {}
        }
        // Kubyl's own display name for contexts.
        if entry.kind == Kind::Context
            && let Some(id) = editor.cluster_id(&entry.name, cx)
        {
            let name = ConnectionManager::global(cx)
                .read(cx)
                .context_settings(&id)
                .display_name
                .unwrap_or_default();
            form.input(
                "display-name",
                name,
                false,
                "same as the context name",
                window,
                cx,
            );
        }
        form
    }

    fn input(
        &mut self,
        id: &'static str,
        value: String,
        masked: bool,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<KubeconfigEditor>,
    ) {
        let placeholder = placeholder.to_string();
        let state = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .masked(masked)
                .placeholder(placeholder);
            state.set_value(value, window, cx);
            state
        });
        self._subscriptions.push(cx.subscribe_in(
            &state,
            window,
            move |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    this.form_changed(id, value, window, cx);
                }
            },
        ));
        self.inputs.insert(id, state);
    }

    fn area(
        &mut self,
        id: &'static str,
        value: String,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<KubeconfigEditor>,
    ) {
        let placeholder = placeholder.to_string();
        let state = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).placeholder(placeholder);
            state.set_value(value, window, cx);
            state
        });
        self._subscriptions.push(cx.subscribe_in(
            &state,
            window,
            move |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = state.read(cx).value().to_string();
                    this.form_changed(id, value, window, cx);
                }
            },
        ));
        self.areas.insert(id, state);
    }

    fn env_row(
        &mut self,
        name: String,
        value: String,
        window: &mut Window,
        cx: &mut Context<KubeconfigEditor>,
    ) {
        let secret = model::is_secret_env(&name);
        let name_state = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("NAME");
            s.set_value(name, window, cx);
            s
        });
        let value_state = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("value")
                .masked(secret);
            s.set_value(value, window, cx);
            s
        });
        for state in [&name_state, &value_state] {
            self._subscriptions.push(cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.env_changed(window, cx);
                    }
                },
            ));
        }
        self.env.push((name_state, value_state));
    }
}

fn placeholder(id: &str) -> &'static str {
    match id {
        "server" => "https://kubernetes.example.com:6443",
        "tls-server-name" => "the name in the server's certificate, if not the host",
        "proxy-url" => "http://proxy:3128 or socks5://…",
        "ca-file" => "/path/to/ca.crt",
        "namespace" => "default",
        "token" => "bearer token",
        "token-file" => "/var/run/secrets/…/token",
        "cert-file" => "/path/to/client.crt",
        "key-file" => "/path/to/client.key",
        "key-data" => "base64 of the PEM key",
        "exec-command" => "aws, gke-gcloud-auth-plugin, kubelogin…",
        "exec-hint" => "shown when the command isn't installed",
        "oidc-issuer" => "https://sso.example.com/realms/platform",
        "oidc-client-id" => "kubernetes",
        "oidc-client-secret" => "only for confidential clients",
        "oidc-scopes" => "groups,email",
        "username" => "user",
        "password" => "password",
        _ => "",
    }
}

impl KubeconfigEditor {
    /// A form field changed.
    fn form_changed(
        &mut self,
        id: &'static str,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.form.entry.clone() else {
            return;
        };
        if id == "display-name" {
            if let Some(cluster) = self.cluster_id(&entry.name, cx) {
                let value = value.trim().to_string();
                ConnectionManager::global(cx).update(cx, |m, cx| {
                    m.update_context_settings(&cluster, cx, |s| {
                        s.display_name = (!value.is_empty()).then_some(value);
                    })
                });
            }
            return;
        }
        let pem_target = match id {
            "ca-pem" => Some((model::CA_FILE, model::CA_DATA)),
            "cert-pem" => Some((model::CERT_FILE, model::CERT_DATA)),
            _ => None,
        };
        self.edit(window, cx, |doc| {
            let Some(body) = doc.body_mut(entry.kind, &entry.name) else {
                return;
            };
            if let Some((file_key, data_key)) = pem_target {
                if value.trim().is_empty() {
                    model::set(body, &[data_key], None);
                } else {
                    PemSource::Data(certs::data_from_pem(&value)).write(body, file_key, data_key);
                }
                return;
            }
            if id == "exec-args" {
                let args: Vec<Value> = value
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| Value::String(l.to_string()))
                    .collect();
                model::set(
                    body,
                    &["exec", "args"],
                    (!args.is_empty()).then_some(Value::Array(args)),
                );
                return;
            }
            if let Some((_, path)) = binding(id) {
                // A file path replaces inline data and the other way round.
                match id {
                    "ca-file" => model::set(body, &[model::CA_DATA], None),
                    "cert-file" => model::set(body, &[model::CERT_DATA], None),
                    "key-file" => model::set(body, &[model::KEY_DATA], None),
                    "key-data" => model::set(body, &[model::KEY_FILE], None),
                    _ => {}
                }
                let value = if id == "server" {
                    value.trim().to_string()
                } else {
                    value
                };
                model::set_str(body, path, &value);
            }
        });
    }

    fn env_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.form.entry.clone() else {
            return;
        };
        let vars: Vec<Value> = self
            .form
            .env
            .iter()
            .map(|(n, v)| {
                (
                    n.read(cx).value().to_string(),
                    v.read(cx).value().to_string(),
                )
            })
            .filter(|(n, _)| !n.trim().is_empty())
            .map(|(n, v)| json!({"name": n.trim(), "value": v}))
            .collect();
        self.edit(window, cx, |doc| {
            if let Some(body) = doc.body_mut(entry.kind, &entry.name) {
                model::set(
                    body,
                    &["exec", "env"],
                    (!vars.is_empty()).then_some(Value::Array(vars)),
                );
            }
        });
    }

    /// Changes the selected entry's body with `f` and rebuilds the form (for pickers and
    /// switches, whose values the inputs don't hold).
    fn change_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Map<String, Value>),
    ) {
        let Some(entry) = self.selection.clone() else {
            return;
        };
        self.edit(window, cx, |doc| {
            if let Some(body) = doc.body_mut(entry.kind, &entry.name) {
                f(body);
            }
        });
        self.rebuild_form(window, cx);
    }

    // ----- Rendering -----

    pub(crate) fn render_form(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(entry) = self.selection.clone() else {
            return widgets::hint("Select or add a context, cluster or user.", &colors);
        };
        let Some(body) = self.doc.body(entry.kind, &entry.name).cloned() else {
            return widgets::hint("This entry is gone.", &colors);
        };
        let header = self.render_entry_header(&entry, &colors, cx);
        let main = match entry.kind {
            Kind::Context => self.render_context(&entry, &body, &colors, cx),
            Kind::Cluster => self.render_cluster(&entry, &body, &colors, cx),
            Kind::User => self.render_user(&entry, &body, &colors, window, cx),
        };
        let problems = self.render_problems(&entry, &colors);
        let last_test = self.render_last_test(&entry, &colors, cx);
        v_flex()
            .gap(u(14.0))
            .child(header)
            .child(main)
            .children(problems)
            .children(last_test)
            .into_any_element()
    }

    fn render_entry_header(
        &self,
        entry: &EntryRef,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let rename = entry.clone();
        let duplicate = entry.clone();
        let delete = entry.clone();
        let production = entry.kind == Kind::Context
            && self.cluster_id(&entry.name, cx).is_some_and(|id| {
                ConnectionManager::global(cx)
                    .read(cx)
                    .context_settings(&id)
                    .production
            });
        let current =
            entry.kind == Kind::Context && self.doc.current_context() == Some(entry.name.as_str());
        h_flex()
            .gap(u(8.0))
            .child(
                div()
                    .text_size(u(12.5))
                    .text_color(colors.text_dim)
                    .child(match entry.kind {
                        Kind::Context => "Context",
                        Kind::Cluster => "Cluster",
                        Kind::User => "User",
                    }),
            )
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(14.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .min_w_0()
                    .truncate()
                    .child(entry.name.clone()),
            )
            .when(production, |this| this.child(kubyl_ui::ProdBadge))
            .when(current, |this| {
                this.child(kubyl_ui::Chip::new("current-context").text_color(colors.accent))
            })
            .child(div().flex_1())
            .child({
                let weak = weak.clone();
                div()
                    .id("kc-duplicate-tip")
                    .tooltip(|window, cx| {
                        gpui_component::tooltip::Tooltip::new("Duplicate").build(window, cx)
                    })
                    .child(
                        kubyl_ui::IconButton::new("kc-duplicate", IconName::Copy).on_click(
                            move |_, window, cx| {
                                weak.update(cx, |this, cx| this.duplicate(&duplicate, window, cx))
                                    .ok();
                            },
                        ),
                    )
            })
            .child({
                let weak = weak.clone();
                Button::new("kc-rename")
                    .ghost()
                    .label("Rename…")
                    .on_click(move |_, window, cx| {
                        crate::dialogs::rename(weak.clone(), rename.clone(), window, cx)
                    })
            })
            .child({
                let weak = weak.clone();
                Button::new("kc-delete")
                    .ghost()
                    .danger()
                    .icon(IconName::Trash)
                    .label("Delete…")
                    .on_click(move |_, window, cx| {
                        crate::dialogs::delete(weak.clone(), delete.clone(), window, cx)
                    })
            })
            .when(entry.kind == Kind::Context, |this| {
                let name = entry.name.clone();
                this.child(
                    MenuButton::new("kc-context-more")
                        .ghost()
                        .compact()
                        .child(Icon::new(IconName::Ellipsis).size(14.0))
                        .dropdown_menu(move |menu, _, _| {
                            let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
                            let (n1, n2, n3) = (name.clone(), name.clone(), name.clone());
                            menu.item(PopupMenuItem::new("Export…").on_click(
                                move |_, window, cx| {
                                    crate::dialogs::export(w1.clone(), n1.clone(), window, cx)
                                },
                            ))
                            .item(PopupMenuItem::new("Copy to another kubeconfig…").on_click(
                                move |_, window, cx| {
                                    crate::dialogs::copy_to(
                                        w2.clone(),
                                        n2.clone(),
                                        false,
                                        window,
                                        cx,
                                    )
                                },
                            ))
                            .item(
                                PopupMenuItem::new("Move to another kubeconfig…").on_click(
                                    move |_, window, cx| {
                                        crate::dialogs::copy_to(
                                            w3.clone(),
                                            n3.clone(),
                                            true,
                                            window,
                                            cx,
                                        )
                                    },
                                ),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn input_row(
        &self,
        id: &'static str,
        label: &str,
        mono: bool,
        colors: &Colors,
    ) -> Option<gpui::Div> {
        let state = self.form.inputs.get(id)?;
        Some(widgets::row(
            label.to_string(),
            widgets::text_input(state, mono, is_secret(id), colors),
            colors,
        ))
    }

    // ----- Context -----

    fn render_context(
        &self,
        entry: &EntryRef,
        body: &Map<String, Value>,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let cluster = model::get_str(body, &["cluster"]);
        let user = model::get_str(body, &["user"]);
        let picker = |id: &'static str,
                      kind: Kind,
                      value: String,
                      names: Vec<String>,
                      weak: gpui::WeakEntity<Self>| {
            let label = if value.is_empty() {
                format!("choose a {}", kind.label())
            } else {
                value.clone()
            };
            let key = match kind {
                Kind::Cluster => "cluster",
                _ => "user",
            };
            MenuButton::new(id)
                .outline()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .gap(u(6.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.5))
                        .child(div().flex_1().truncate().child(label))
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for name in &names {
                        let weak = weak.clone();
                        let name = name.clone();
                        menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                            move |_, window, cx| {
                                let name = name.clone();
                                weak.update(cx, |this, cx| {
                                    this.change_body(window, cx, |b| {
                                        model::set_str(b, &[key], &name)
                                    })
                                })
                                .ok();
                            },
                        ));
                    }
                    if kind == Kind::User {
                        let weak = weak.clone();
                        menu = menu.separator().item(
                            PopupMenuItem::new("No user (anonymous)").on_click(
                                move |_, window, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.change_body(window, cx, |b| {
                                            model::set(b, &["user"], None)
                                        })
                                    })
                                    .ok();
                                },
                            ),
                        );
                    }
                    menu
                })
        };
        let edit_link =
            |id: &'static str, kind: Kind, name: String, weak: gpui::WeakEntity<Self>| {
                widgets::link(
                    id,
                    format!("Edit {}", kind.label()),
                    colors,
                    move |_, window, cx| {
                        let name = name.clone();
                        weak.update(cx, |this, cx| {
                            this.select(Some(EntryRef::new(kind, name)), window, cx)
                        })
                        .ok();
                    },
                )
            };
        let namespaces: Vec<String> = Kubeconfigs::try_global(cx)
            .and_then(|g| {
                g.read(cx)
                    .report(&TestKey::new(self.doc_key(), entry.name.clone()))
                    .and_then(|r| r.namespaces.clone())
            })
            .unwrap_or_default();
        let ns_hint = if namespaces.is_empty() {
            "Test the connection to pick from the cluster's namespaces.".to_string()
        } else {
            format!("{} namespaces from the last test", namespaces.len())
        };
        let ns_picker = (!namespaces.is_empty()).then(|| {
            let weak = weak.clone();
            MenuButton::new("kc-ns-pick")
                .ghost()
                .compact()
                .child(Icon::new(IconName::ChevronDown).size(11.0))
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for ns in &namespaces {
                        let weak = weak.clone();
                        let ns = ns.clone();
                        menu = menu.item(PopupMenuItem::new(ns.clone()).on_click(
                            move |_, window, cx| {
                                let ns = ns.clone();
                                weak.update(cx, |this, cx| {
                                    this.change_body(window, cx, |b| {
                                        model::set_str(b, &["namespace"], &ns)
                                    })
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                })
        });
        let current = self.doc.current_context() == Some(entry.name.as_str());
        let name = entry.name.clone();
        let card = widgets::card("Context", None, colors)
            .child(widgets::row(
                "Cluster",
                h_flex()
                    .gap(u(10.0))
                    .child(div().flex_1().child(picker(
                        "kc-cluster",
                        Kind::Cluster,
                        cluster.clone(),
                        self.doc.names(Kind::Cluster),
                        weak.clone(),
                    )))
                    .when(
                        !cluster.is_empty() && self.doc.contains(Kind::Cluster, &cluster),
                        |this| {
                            this.child(edit_link(
                                "kc-edit-cluster",
                                Kind::Cluster,
                                cluster.clone(),
                                weak.clone(),
                            ))
                        },
                    ),
                colors,
            ))
            .child(widgets::row(
                "User",
                h_flex()
                    .gap(u(10.0))
                    .child(div().flex_1().child(picker(
                        "kc-user",
                        Kind::User,
                        user.clone(),
                        self.doc.names(Kind::User),
                        weak.clone(),
                    )))
                    .when(
                        !user.is_empty() && self.doc.contains(Kind::User, &user),
                        |this| {
                            this.child(edit_link(
                                "kc-edit-user",
                                Kind::User,
                                user.clone(),
                                weak.clone(),
                            ))
                        },
                    ),
                colors,
            ))
            .children(self.form.inputs.get("namespace").map(|state| {
                widgets::row_with_hint(
                    "Namespace",
                    h_flex()
                        .gap(u(6.0))
                        .child(
                            div()
                                .flex_1()
                                .child(widgets::text_input(state, true, false, colors)),
                        )
                        .children(ns_picker),
                    Some(widgets::hint(ns_hint, colors)),
                    colors,
                )
            }))
            .child(widgets::option_row(
                "kc-current",
                "Current context",
                "kubectl uses it when no --context is given",
                current,
                true,
                colors,
                {
                    let weak = weak.clone();
                    move |_, window, cx| {
                        let name = name.clone();
                        weak.update(cx, |this, cx| {
                            this.edit(window, cx, |doc| {
                                if doc.current_context() == Some(name.as_str()) {
                                    doc.set_current_context(None);
                                } else {
                                    doc.set_current_context(Some(&name));
                                }
                            })
                        })
                        .ok();
                    }
                },
            ));
        v_flex()
            .gap(u(14.0))
            .child(card)
            .child(self.render_overrides(entry, colors, cx))
            .into_any_element()
    }

    /// Kubyl's overrides for a context: settings.json, never the kubeconfig.
    fn render_overrides(
        &self,
        entry: &EntryRef,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card = widgets::card(
            "Kubyl overrides",
            Some("settings.json (not written to the kubeconfig)"),
            colors,
        );
        let Some(id) = self.cluster_id(&entry.name, cx) else {
            return card
                .child(widgets::hint(
                    "Save the file to set a display name, color, production and read-only for this context.",
                    colors,
                ))
                .into_any_element();
        };
        let manager = ConnectionManager::global(cx);
        let settings = manager.read(cx).context_settings(&id);
        let current_color = manager.read(cx).color(&id, cx);
        let swatches = h_flex()
            .gap(u(8.0))
            .children(ColorTag::ALL.iter().enumerate().map(|(ix, tag)| {
                let color = tag.color(colors);
                let selected = color == current_color;
                let id = id.clone();
                let tag = *tag;
                div()
                    .id(SharedString::from(format!("kc-color-{ix}")))
                    .size(u(14.0))
                    .rounded_full()
                    .bg(color)
                    .cursor_pointer()
                    .when(selected, |this| this.border_2().border_color(colors.text))
                    .on_click(move |_, _, cx| {
                        let id = id.clone();
                        ConnectionManager::global(cx).update(cx, |m, cx| {
                            m.update_context_settings(&id, cx, |s| s.color = Some(tag))
                        });
                    })
            }));
        let toggle_setting = |key: &'static str, on: bool, id: kubyl_core::ClusterId| {
            move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
                let id = id.clone();
                ConnectionManager::global(cx).update(cx, |m, cx| {
                    m.update_context_settings(&id, cx, |s| match key {
                        "production" => s.production = !on,
                        _ => s.read_only = !on,
                    })
                });
            }
        };
        card.children(self.form.inputs.get("display-name").map(|state| {
            widgets::row(
                "Display name",
                widgets::text_input(state, false, false, colors),
                colors,
            )
        }))
        .child(widgets::row("Color", swatches, colors))
        .child(widgets::option_row(
            "kc-prod",
            "Production cluster",
            "Red accent in title bar, typed confirmation for delete / scale to 0",
            settings.production,
            true,
            colors,
            toggle_setting("production", settings.production, id.clone()),
        ))
        .child(widgets::option_row(
            "kc-ro",
            "Read-only mode",
            "Block all mutating requests from this app",
            settings.read_only,
            true,
            colors,
            toggle_setting("read_only", settings.read_only, id.clone()),
        ))
        .into_any_element()
    }

    // ----- Cluster -----

    fn render_cluster(
        &self,
        entry: &EntryRef,
        body: &Map<String, Value>,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let ca = PemSource::read(body, model::CA_FILE, model::CA_DATA);
        let mode = self.form.modes.get("ca").copied().unwrap_or(match ca {
            PemSource::None => PemMode::System,
            PemSource::File(_) => PemMode::File,
            PemSource::Data(_) => PemMode::Pem,
        });
        let insecure = model::get_bool(body, &["insecure-skip-tls-verify"]);
        let no_compression = model::get_bool(body, &["disable-compression"]);
        let options = vec![
            (
                PemMode::System,
                SharedString::from("System trust store"),
                Some(IconName::Shield),
            ),
            (PemMode::File, "File".into(), Some(IconName::File)),
            (PemMode::Pem, "Paste PEM".into(), Some(IconName::Code)),
        ];
        let seg = {
            let weak = weak.clone();
            widgets::segmented(
                "kc-ca-mode",
                options,
                mode,
                colors,
                move |mode, window, cx| {
                    let mode = *mode;
                    weak.update(cx, |this, cx| {
                        this.form.modes.insert("ca", mode);
                        if mode == PemMode::System {
                            this.change_body(window, cx, |b| {
                                PemSource::None.write(b, model::CA_FILE, model::CA_DATA)
                            });
                            this.form.modes.insert("ca", mode);
                        }
                        cx.notify();
                    })
                    .ok();
                },
            )
        };
        let server = model::get_str(body, &["server"]);
        let fetch = {
            let weak = weak.clone();
            let name = entry.name.clone();
            Button::new("kc-fetch-ca")
                .icon(IconName::Download)
                .label("Fetch from server…")
                .disabled(server.is_empty())
                .on_click(move |_, window, cx| {
                    crate::dialogs::fetch_ca_for(weak.clone(), name.clone(), window, cx)
                })
        };
        let mut ca_block = v_flex().gap(u(8.0)).child(
            h_flex()
                .gap(u(8.0))
                .child(div().flex_1().child(seg))
                .child(fetch),
        );
        match mode {
            PemMode::System => {
                ca_block = ca_block.child(widgets::hint(
                    "The server's certificate must be signed by a CA your system trusts.",
                    colors,
                ))
            }
            PemMode::File => {
                if let Some(state) = self.form.inputs.get("ca-file") {
                    ca_block = ca_block.child(widgets::text_input(state, true, false, colors));
                }
            }
            PemMode::Pem => {
                if let Some(state) = self.form.areas.get("ca-pem") {
                    ca_block = ca_block.child(pem_area(state, colors));
                }
            }
        }
        let dir = self.path.parent();
        if let Ok(Some(pem)) = validate::load_pem(&ca, dir)
            && let Ok(found) = certs::certificates(&pem)
        {
            for cert in found.iter().take(3) {
                ca_block = ca_block.child(cert_card(cert, colors));
            }
        }
        let card = widgets::card("Cluster", None, colors)
            .children(self.input_row("server", "Server URL", true, colors))
            .child(widgets::row_with_hint("Certificate authority", ca_block, None, colors))
            .children(self.input_row("tls-server-name", "TLS server name", true, colors))
            .children(self.input_row("proxy-url", "Proxy URL", true, colors))
            .child(widgets::option_row(
                "kc-insecure",
                "Skip TLS verification",
                "Insecure: anyone on the network path can read your credentials",
                insecure,
                true,
                colors,
                {
                    let weak = weak.clone();
                    move |_, window, cx| {
                        weak.update(cx, |this, cx| {
                            this.change_body(window, cx, |b| model::set_bool(b, &["insecure-skip-tls-verify"], !insecure))
                        })
                        .ok();
                    }
                },
            ))
            .when(insecure, |this| {
                this.child(widgets::notice(
                    Severity::Error,
                    "TLS isn't verified for this cluster. Fetch its CA instead: Kubyl shows the fingerprint for you to confirm.",
                    colors,
                ))
            })
            .child(widgets::option_row(
                "kc-compression",
                "Disable compression",
                "Don't ask the API server for compressed responses",
                no_compression,
                true,
                colors,
                move |_, window, cx| {
                    weak.update(cx, |this, cx| {
                        this.change_body(window, cx, |b| model::set_bool(b, &["disable-compression"], !no_compression))
                    })
                    .ok();
                },
            ));
        let used = self.doc.used_by(Kind::Cluster, &entry.name);
        v_flex()
            .gap(u(14.0))
            .child(card)
            .when(!used.is_empty(), |this| {
                this.child(widgets::hint(
                    format!("Used by {}", used.join(", ")),
                    colors,
                ))
            })
            .into_any_element()
    }

    // ----- User -----

    fn render_user(
        &self,
        entry: &EntryRef,
        body: &Map<String, Value>,
        colors: &Colors,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let kind = AuthKind::detect(body);
        let kind_picker = {
            let weak = weak.clone();
            MenuButton::new("kc-auth-kind")
                .outline()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(12.5))
                        .child(kind.label())
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for choice in AuthKind::CHOICES {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(choice.label()).on_click(
                            move |_, window, cx| {
                                weak.update(cx, |this, cx| {
                                    this.change_body(window, cx, |b| {
                                        model::set_auth_kind(b, choice)
                                    })
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                })
        };
        let mut card = widgets::card("User", None, colors).child(widgets::row(
            "Authentication",
            kind_picker,
            colors,
        ));
        match kind {
            AuthKind::Token => {
                let token = model::get_str(body, &["token"]);
                let expiry = kubyl_kube::auth::jwt_expiry(&token).map(|at| {
                    let expired = at < jiff::Timestamp::now();
                    (expired, at.strftime("%Y-%m-%d %H:%M UTC").to_string())
                });
                card = card
                    .children(self.input_row("token", "Token", true, colors))
                    .child(
                        h_flex()
                            .gap(u(10.0))
                            .pl(u(162.0))
                            .children(expiry.map(|(expired, at)| {
                                widgets::hint(
                                    if expired {
                                        format!("expired {at}")
                                    } else {
                                        format!("expires {at}")
                                    },
                                    colors,
                                )
                            }))
                            .child(copy_secret_button("kc-copy-token", token, colors)),
                    );
            }
            AuthKind::TokenFile => {
                card = card.children(self.input_row("token-file", "Token file", true, colors))
            }
            AuthKind::ClientCertificate => {
                card = card.child(self.render_client_cert(body, colors, cx));
            }
            AuthKind::Exec => card = card.child(self.render_exec(entry, body, colors, cx)),
            AuthKind::OidcProvider => {
                card = card
                    .children(self.input_row("oidc-issuer", "Issuer URL", true, colors))
                    .children(self.input_row("oidc-client-id", "Client ID", true, colors))
                    .children(self.input_row("oidc-client-secret", "Client secret", true, colors))
                    .children(self.input_row("oidc-scopes", "Extra scopes", true, colors));
                let extra: Vec<String> = model::get(body, &["auth-provider", "config"])
                    .and_then(Value::as_object)
                    .map(|c| {
                        c.keys()
                            .filter(|k| {
                                !matches!(
                                    k.as_str(),
                                    "idp-issuer-url"
                                        | "client-id"
                                        | "client-secret"
                                        | "extra-scopes"
                                )
                            })
                            .map(|k| {
                                if model::SECRET_PROVIDER_KEYS.contains(&k.as_str()) {
                                    format!("{k}: ••••")
                                } else {
                                    format!("{k}: {}", model::get_str(c, &[k]))
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !extra.is_empty() {
                    card = card.child(widgets::row_with_hint(
                        "Extra parameters",
                        v_flex()
                            .gap(u(2.0))
                            .children(extra.into_iter().map(widgets::mono)),
                        Some(widgets::hint("Edit them in the YAML tab.", colors)),
                        colors,
                    ));
                }
                card = card.child(widgets::hint(
                    "Kubyl signs in with this provider in your browser (PKCE); tokens go to the OS keychain.",
                    colors,
                ));
            }
            AuthKind::Basic => {
                card = card
                    .child(widgets::notice(
                        Severity::Warning,
                        "Basic auth was removed in Kubernetes 1.19.",
                        colors,
                    ))
                    .children(self.input_row("username", "Username", true, colors))
                    .children(self.input_row("password", "Password", true, colors));
            }
            AuthKind::OtherProvider => {
                card = card.child(widgets::notice(
                    Severity::Warning,
                    "This auth provider was removed from kubectl 1.26. Switch to its exec plugin (gke-gcloud-auth-plugin, kubelogin).",
                    colors,
                ));
            }
            AuthKind::None => card = card.child(widgets::hint("Requests are anonymous.", colors)),
        }
        let used = self.doc.used_by(Kind::User, &entry.name);
        v_flex()
            .gap(u(14.0))
            .child(card)
            .when(!used.is_empty(), |this| {
                this.child(widgets::hint(
                    format!("Used by {}", used.join(", ")),
                    colors,
                ))
            })
            .into_any_element()
    }

    fn render_client_cert(
        &self,
        body: &Map<String, Value>,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let cert = PemSource::read(body, model::CERT_FILE, model::CERT_DATA);
        let key = PemSource::read(body, model::KEY_FILE, model::KEY_DATA);
        let cert_mode = self.form.modes.get("cert").copied().unwrap_or(match cert {
            PemSource::Data(_) => PemMode::Pem,
            _ => PemMode::File,
        });
        let key_mode = self.form.modes.get("key").copied().unwrap_or(match key {
            PemSource::Data(_) => PemMode::Pem,
            _ => PemMode::File,
        });
        let seg =
            |id: &'static str, which: &'static str, mode: PemMode, weak: gpui::WeakEntity<Self>| {
                widgets::segmented(
                    id,
                    vec![
                        (PemMode::File, "File".into(), Some(IconName::File)),
                        (PemMode::Pem, "Inline".into(), Some(IconName::Code)),
                    ],
                    mode,
                    colors,
                    move |mode, _, cx| {
                        let mode = *mode;
                        weak.update(cx, |this, cx| {
                            this.form.modes.insert(which, mode);
                            cx.notify();
                        })
                        .ok();
                    },
                )
            };
        let mut cert_block =
            v_flex()
                .gap(u(6.0))
                .child(seg("kc-cert-mode", "cert", cert_mode, weak.clone()));
        cert_block = match cert_mode {
            PemMode::Pem => {
                cert_block.children(self.form.areas.get("cert-pem").map(|s| pem_area(s, colors)))
            }
            _ => cert_block.children(
                self.form
                    .inputs
                    .get("cert-file")
                    .map(|s| widgets::text_input(s, true, false, colors)),
            ),
        };
        if let Ok(Some(pem)) = validate::load_pem(&cert, self.path.parent())
            && let Ok(found) = certs::certificates(&pem)
            && let Some(first) = found.first()
        {
            cert_block = cert_block.child(cert_card(first, colors));
        }
        let mut key_block = v_flex()
            .gap(u(6.0))
            .child(seg("kc-key-mode", "key", key_mode, weak));
        key_block = match key_mode {
            PemMode::Pem => key_block
                .children(
                    self.form
                        .inputs
                        .get("key-data")
                        .map(|s| widgets::text_input(s, true, true, colors)),
                )
                .child(widgets::hint(
                    "The PEM key, base64-encoded (client-key-data). Masked; the eye reveals it.",
                    colors,
                )),
            _ => key_block.children(
                self.form
                    .inputs
                    .get("key-file")
                    .map(|s| widgets::text_input(s, true, false, colors)),
            ),
        };
        v_flex()
            .gap(u(10.0))
            .child(widgets::row_with_hint(
                "Client certificate",
                cert_block,
                None,
                colors,
            ))
            .child(widgets::row_with_hint(
                "Client key",
                key_block,
                None,
                colors,
            ))
            .into_any_element()
    }

    fn render_exec(
        &self,
        entry: &EntryRef,
        body: &Map<String, Value>,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = cx.entity().downgrade();
        let presets = {
            let weak = weak.clone();
            MenuButton::new("kc-exec-preset")
                .ghost()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(12.5))
                        .child("Presets")
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for preset in ExecPreset::ALL {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(preset.label()).on_click(
                            move |_, window, cx| {
                                weak.update(cx, |this, cx| {
                                    this.change_body(window, cx, |b| {
                                        b.insert("exec".into(), preset.exec());
                                    })
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                })
        };
        let api = model::get_str(body, &["exec", "apiVersion"]);
        let interactive = model::get_str(body, &["exec", "interactiveMode"]);
        let cluster_info = model::get_bool(body, &["exec", "provideClusterInfo"]);
        let choice = |id: &'static str,
                      value: String,
                      choices: &'static [&'static str],
                      path: &'static [&'static str],
                      weak: gpui::WeakEntity<Self>| {
            MenuButton::new(id)
                .outline()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(if value.is_empty() {
                            "(not set)".to_string()
                        } else {
                            value.clone()
                        })
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for c in choices {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(if c.is_empty() { "(not set)" } else { c })
                                .on_click(move |_, window, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.change_body(window, cx, |b| model::set_str(b, path, c))
                                    })
                                    .ok();
                                }),
                        );
                    }
                    menu
                })
        };
        let env_rows = v_flex()
            .gap(u(6.0))
            .children(self.form.env.iter().enumerate().map(|(ix, (name, value))| {
                let weak = weak.clone();
                h_flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .w(u(170.0))
                            .child(widgets::text_input(name, true, false, colors)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(widgets::text_input(value, true, true, colors)),
                    )
                    .child(
                        kubyl_ui::IconButton::new(
                            SharedString::from(format!("kc-env-remove-{ix}")),
                            IconName::Minus,
                        )
                        .on_click(move |_, window, cx| {
                            weak.update(cx, |this, cx| {
                                if ix < this.form.env.len() {
                                    this.form.env.remove(ix);
                                }
                                this.env_changed(window, cx);
                            })
                            .ok();
                        }),
                    )
            }))
            .child({
                let weak = weak.clone();
                Button::new("kc-env-add")
                    .ghost()
                    .icon(IconName::Plus)
                    .label("Add variable")
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| {
                            this.form.env_row(String::new(), String::new(), window, cx);
                            cx.notify();
                        })
                        .ok();
                    })
            });
        let name = entry.name.clone();
        let _ = name;
        v_flex()
            .gap(u(10.0))
            .child(widgets::row("Preset", presets, colors))
            .children(self.input_row("exec-command", "Command", true, colors))
            .child(widgets::row_with_hint(
                "Arguments",
                self.form.areas.get("exec-args").map(|s| {
                    div()
                        .h(u(96.0))
                        .rounded(u(5.0))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.input_background)
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .child(Textarea::new(s).h_full().appearance(false))
                        .into_any_element()
                }).unwrap_or_else(|| div().into_any_element()),
                Some(widgets::hint("One per line.", colors)),
                colors,
            ))
            .child(widgets::row_with_hint("Environment", env_rows, None, colors))
            .child(widgets::row(
                "API version",
                choice(
                    "kc-exec-api",
                    api,
                    &["client.authentication.k8s.io/v1", "client.authentication.k8s.io/v1beta1"],
                    &["exec", "apiVersion"],
                    weak.clone(),
                ),
                colors,
            ))
            .child(widgets::row(
                "Interactive mode",
                choice("kc-exec-interactive", interactive, &["", "Never", "IfAvailable", "Always"], &["exec", "interactiveMode"], weak.clone()),
                colors,
            ))
            .child(widgets::option_row(
                "kc-exec-cluster-info",
                "Provide cluster info",
                "Pass the server and CA to the plugin (KUBERNETES_EXEC_INFO)",
                cluster_info,
                true,
                colors,
                move |_, window, cx| {
                    weak.update(cx, |this, cx| {
                        this.change_body(window, cx, |b| model::set_bool(b, &["exec", "provideClusterInfo"], !cluster_info))
                    })
                    .ok();
                },
            ))
            .children(self.input_row("exec-hint", "Install hint", false, colors))
            .child(widgets::hint(
                "Kubyl runs a plugin from unsaved edits only after you saw its command, arguments and environment and agreed.",
                colors,
            ))
            .into_any_element()
    }

    /// The entry's problems; for a context also its cluster's and user's.
    fn render_problems(&self, entry: &EntryRef, colors: &Colors) -> Option<AnyElement> {
        let mut found: Vec<(Option<String>, &validate::Problem)> =
            validate::of_entry(&self.problems, entry)
                .into_iter()
                .map(|p| (None, p))
                .collect();
        if entry.kind == Kind::Context {
            let (cluster, user) = self.doc.context_refs(&entry.name);
            for (kind, name) in [(Kind::Cluster, cluster), (Kind::User, user)] {
                let Some(name) = name else { continue };
                for p in validate::of_entry(&self.problems, &EntryRef::new(kind, name.clone())) {
                    found.push((Some(format!("{} {name}", title_case(kind.label()))), p));
                }
            }
        }
        if found.is_empty() {
            return None;
        }
        found.sort_by_key(|(_, p)| p.severity);
        let count = found
            .iter()
            .filter(|(_, p)| p.severity != Severity::Info)
            .count();
        // Only notes (certificate expiry and the like): no "Problems · 0".
        let title = if count == 0 {
            "Notes".to_string()
        } else {
            format!("Problems · {count}")
        };
        let mut card = widgets::card(title, None, colors);
        for (owner, problem) in found {
            let (color, icon) = widgets::severity_style(problem.severity, colors);
            card = card.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .child(
                        div()
                            .pt(u(1.0))
                            .child(Icon::new(icon).size(13.0).color(color)),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .flex_wrap()
                            .gap(u(4.0))
                            .text_size(u(12.5))
                            .children(owner.map(|o| {
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{o}:"))
                            }))
                            .child(problem.message.clone()),
                    ),
            );
        }
        Some(card.into_any_element())
    }

    /// "Last test": the context's last result, if it was tested.
    fn render_last_test(
        &self,
        entry: &EntryRef,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if entry.kind != Kind::Context {
            return None;
        }
        let report = Kubeconfigs::try_global(cx)?
            .read(cx)
            .report(&TestKey::new(self.doc_key(), entry.name.clone()))?
            .clone();
        let ago = (jiff::Timestamp::now().as_second() - report.started.as_second()).max(0);
        let when = if ago < 60 {
            "just now".to_string()
        } else {
            format!("{} min ago", ago / 60)
        };
        let weak = cx.entity().downgrade();
        Some(
            h_flex()
                .gap(u(10.0))
                .px(u(14.0))
                .py(u(10.0))
                .rounded(u(8.0))
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .child(
                    div()
                        .text_size(u(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_dim)
                        .child(format!("LAST TEST · {}", when.to_uppercase())),
                )
                .child(crate::panel::badge(&report, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(u(12.5))
                        .child(report.summary()),
                )
                .child(widgets::link(
                    "kc-last-test",
                    "Details",
                    colors,
                    move |_, _, cx| {
                        weak.update(cx, |this, cx| {
                            this.show_test = true;
                            cx.notify();
                        })
                        .ok();
                    },
                ))
                .into_any_element(),
        )
    }
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn pem_area(state: &Entity<TextareaState>, colors: &Colors) -> AnyElement {
    div()
        .h(u(110.0))
        .rounded(u(5.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.input_background)
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(Textarea::new(state).h_full().appearance(false))
        .into_any_element()
}

/// Subject, issuer, validity and fingerprint of a certificate.
pub fn cert_card(cert: &certs::CertInfo, colors: &Colors) -> AnyElement {
    let (validity_color, validity) = if cert.expired() {
        (colors.red, format!("expired {}", cert.not_after_date()))
    } else if cert.expires_soon() {
        (
            colors.yellow,
            format!(
                "until {} ({} days)",
                cert.not_after_date(),
                cert.days_left()
            ),
        )
    } else {
        (colors.text, format!("until {}", cert.not_after_date()))
    };
    div()
        .p(u(10.0))
        .rounded(u(6.0))
        .border_1()
        .border_color(colors.border_variant)
        .child(widgets::kv(
            vec![
                ("Subject", widgets::mono(cert.subject.clone())),
                (
                    "Issuer",
                    widgets::mono(if cert.self_signed {
                        format!("{} (self-signed)", cert.issuer)
                    } else {
                        cert.issuer.clone()
                    }),
                ),
                (
                    "Valid",
                    div()
                        .text_color(validity_color)
                        .child(validity)
                        .into_any_element(),
                ),
                ("SHA-256", widgets::mono(cert.fingerprint())),
            ],
            colors,
        ))
        .into_any_element()
}

/// Copies a secret to the clipboard: always an explicit click, with a toast.
pub fn copy_secret_button(id: &'static str, secret: String, _colors: &Colors) -> AnyElement {
    Button::new(id)
        .ghost()
        .icon(IconName::Copy)
        .label("Copy")
        .disabled(secret.is_empty())
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(secret.clone()));
            NotificationCenter::push(cx, Notification::info("Copied to the clipboard."));
        })
        .into_any_element()
}
