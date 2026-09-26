//! The editor's dialogs (board 11): exec-plugin consent, fetching a CA (trust on first use),
//! the save preview, editing opt-in, "Keep mine", saving as a Kubyl copy, rename and delete,
//! merge, split, export, and copying or moving a context to another file.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FontWeight, IntoElement, PathPromptOptions, Render, SharedString, Task, WeakEntity, Window,
    div, prelude::*, px,
};
use gpui_component::WindowExt as _;
use kubyl_core::{Notification, NotificationCenter, spawn_kube};
use kubyl_explorer::dialogs::{ConfirmSpec, confirm, prompt_text};
use kubyl_kube::ConnectionManager;
use kubyl_kube::kubeconfig::{ContextInfo, SourceKind};
use kubyl_kube::settings::display_path;
use kubyl_settings::Settings;
use kubyl_ui::{ActiveColors, Button, Chip, Colors, Icon, IconName, fonts, h_flex, u, v_flex};

use crate::certs;
use crate::editor::KubeconfigEditor;
use crate::files::{self, Backups, SaveError, SaveOptions};
use crate::model::{self, Doc, EntryRef, ExecSpec, Kind, PemSource};
use crate::settings::KubeconfigSettings;
use crate::state::Kubeconfigs;
use crate::tls::{self, CaCandidate, FetchedCa, Target};
use crate::validate::Severity;
use crate::widgets;
use crate::yaml;

/// A button's action.
type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// Opens `view` as a dialog; `focus` gets focus after it opened.
pub(crate) fn open<V: Render>(
    view: Entity<V>,
    width: f32,
    focus: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) {
    let colors = cx.colors().clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(width))
            .margin_top(px(70.0))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            // Enter in an input would close the dialog first; the views submit themselves.
            .on_ok(|_, _, _| false)
            // As content, not a child, so presses on the footer reach its buttons.
            .content({
                let view = view.clone();
                move |content, _, _| content.child(view.clone())
            })
    });
    if let Some(focus) = focus {
        window.focus(&focus, cx);
    }
}

pub(crate) fn header(
    icon: IconName,
    title: impl Into<SharedString>,
    extra: Option<AnyElement>,
    colors: &Colors,
) -> impl IntoElement {
    h_flex()
        .gap(u(10.0))
        .px(u(16.0))
        .py(u(14.0))
        .border_b_1()
        .border_color(colors.border_variant)
        .child(Icon::new(icon).size(16.0).color(colors.accent))
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
        .children(extra)
}

pub(crate) fn footer(
    note: Option<AnyElement>,
    buttons: Vec<AnyElement>,
    colors: &Colors,
) -> impl IntoElement {
    h_flex()
        .gap(u(8.0))
        .px(u(16.0))
        .py(u(12.0))
        .border_t_1()
        .border_color(colors.border_variant)
        .children(note)
        .child(div().flex_1())
        .children(buttons)
}

pub(crate) fn cancel_button(id: &'static str) -> AnyElement {
    Button::new(id)
        .ghost()
        .label("Cancel")
        .on_click(|_, window, cx| window.close_dialog(cx))
        .into_any_element()
}

// ----- Rename and delete -----

pub(crate) fn rename(
    editor: WeakEntity<KubeconfigEditor>,
    entry: EntryRef,
    window: &mut Window,
    cx: &mut App,
) {
    let title = format!("Rename {} {}", entry.kind.label(), entry.name);
    let initial = entry.name.clone();
    prompt_text(
        title.into(),
        "New name",
        initial,
        move |name, window, cx| {
            let entry = entry.clone();
            editor
                .update(cx, |this, cx| this.rename(&entry, name, window, cx))
                .ok();
        },
        window,
        cx,
    );
}

pub(crate) fn delete(
    editor: WeakEntity<KubeconfigEditor>,
    entry: EntryRef,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(this) = editor.upgrade() else { return };
    let used = this.read(cx).doc.used_by(entry.kind, &entry.name);
    let mut spec = ConfirmSpec::new(
        format!("Delete {} {}?", entry.kind.label(), entry.name),
        "Delete",
    );
    spec.danger = true;
    if !used.is_empty() {
        spec.lines = used
            .iter()
            .map(|c| SharedString::from(format!("context {c}")))
            .collect();
        spec.note = Some(
            format!(
                "{} still use this {}; they'll point at a missing entry until you change them.",
                if used.len() == 1 {
                    "This context"
                } else {
                    "These contexts"
                },
                entry.kind.label()
            )
            .into(),
        );
    } else {
        spec.note = Some("Nothing is written until you save.".into());
    }
    confirm(
        spec,
        move |_, window, cx| {
            let entry = entry.clone();
            editor
                .update(cx, |this, cx| this.delete(&entry, window, cx))
                .ok();
        },
        window,
        cx,
    );
}

// ----- Exec plugin consent -----

struct ConsentView {
    asks: Vec<(Vec<String>, ExecSpec)>,
    /// Where each command resolves (looked up off the UI thread).
    resolved: Vec<Option<String>>,
    on_ok: OnClick,
    focus: FocusHandle,
    _lookup: Task<()>,
}

/// Asks before running exec plugins from unsaved edits, imports or new documents. `asks`: the
/// contexts that run each plugin. `on_ok` runs after the user agreed (consent is remembered for
/// the session, for exactly these configs).
pub(crate) fn consent(
    asks: Vec<(Vec<String>, ExecSpec)>,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| {
        let commands: Vec<String> = asks.iter().map(|(_, s)| s.command.clone()).collect();
        let lookup = cx.background_executor().spawn(async move {
            commands
                .iter()
                .map(|c| crate::validate::find_command(c, None).map(|p| p.display().to_string()))
                .collect::<Vec<_>>()
        });
        let task = cx.spawn(async move |this: WeakEntity<ConsentView>, cx| {
            let resolved = lookup.await;
            this.update(cx, |this, cx| {
                this.resolved = resolved;
                cx.notify();
            })
            .ok();
        });
        ConsentView {
            resolved: vec![None; asks.len()],
            asks,
            on_ok: Rc::new(on_ok),
            focus: cx.focus_handle(),
            _lookup: task,
        }
    });
    let focus = view.read(cx).focus.clone();
    open(view, 520.0, Some(focus), window, cx);
}

impl Focusable for ConsentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ConsentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let title = match self.asks.as_slice() {
            [(contexts, _)] if contexts.len() == 1 => {
                format!("Run an exec plugin for {}?", contexts[0])
            }
            asks => format!("Run {} exec plugins?", asks.len()),
        };
        let mut body = v_flex().gap(u(12.0)).p(u(16.0)).child(
            div().text_size(u(12.5)).child(
                "These commands come from unsaved edits, an import, a new kubeconfig or a file that isn't one of your kubeconfig sources. Kubyl runs them only after you agree; they can do anything your user can. Check the command, its arguments and environment.",
            ),
        );
        for (ix, (contexts, spec)) in self.asks.iter().enumerate() {
            let env = spec
                .env
                .iter()
                .map(|(n, v)| {
                    if model::is_secret_env(n) {
                        format!("{n}=••••")
                    } else {
                        format!("{n}={v}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            let rows = vec![
                ("command", spec.command.clone()),
                (
                    "args",
                    if spec.args.is_empty() {
                        "(none)".into()
                    } else {
                        spec.args.join(" ")
                    },
                ),
                ("env", if env.is_empty() { "(none)".into() } else { env }),
                ("apiVersion", spec.api_version.clone()),
                (
                    "interactive",
                    if spec.interactive_mode.is_empty() {
                        "(not set)".into()
                    } else {
                        spec.interactive_mode.clone()
                    },
                ),
            ];
            body = body.child(
                v_flex()
                    .gap(u(6.0))
                    .when(self.asks.len() > 1 || contexts.len() > 1, |this| {
                        this.child(
                            div()
                                .text_size(u(11.5))
                                .text_color(colors.text_dim)
                                .child(format!("for {}", contexts.join(", "))),
                        )
                    })
                    .child(
                        v_flex()
                            .gap(u(3.0))
                            .p(u(10.0))
                            .rounded(u(6.0))
                            .bg(colors.input_background)
                            .border_1()
                            .border_color(colors.border)
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .children(rows.into_iter().map(|(k, v)| {
                                h_flex()
                                    .items_start()
                                    .gap(u(10.0))
                                    .child(
                                        div()
                                            .w(u(90.0))
                                            .flex_none()
                                            .text_color(colors.text_dim)
                                            .child(k),
                                    )
                                    .child(div().flex_1().min_w_0().child(v))
                            })),
                    )
                    .child(div().text_size(u(11.5)).text_color(colors.text_dim).child(
                        match self.resolved.get(ix).cloned().flatten() {
                            Some(path) => format!("Resolved with your login shell PATH: {path}"),
                            None => format!(
                                "`{}` wasn't found in your login shell's PATH.",
                                spec.command
                            ),
                        },
                    )),
            );
        }
        let specs: Vec<ExecSpec> = self.asks.iter().map(|(_, s)| s.clone()).collect();
        let on_ok = self.on_ok.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(header(IconName::Terminal, title, None, &colors))
            .child(body)
            .child(footer(
                None,
                vec![
                    cancel_button("kc-consent-cancel"),
                    Button::new("kc-consent-ok")
                        .primary()
                        .icon(IconName::Play)
                        .label("Run and test")
                        .on_click(move |_, window, cx| {
                            Kubeconfigs::global(cx).update(cx, |g, _| {
                                for spec in &specs {
                                    g.consent(spec);
                                }
                            });
                            window.close_dialog(cx);
                            on_ok(window, cx);
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

// ----- Fetching a CA (trust on first use) -----

/// A CA fetch in progress or done.
#[derive(Clone)]
pub(crate) enum CaState {
    Fetching,
    Done(FetchedCa),
    Failed(String),
}

/// Starts fetching the CA of `server`. The result goes to `on_done` on the UI thread.
pub(crate) fn start_fetch<V: 'static>(
    server: &str,
    tls_server_name: Option<&str>,
    cx: &mut Context<V>,
    on_done: impl FnOnce(&mut V, CaState, &mut Context<V>) + 'static,
) -> Task<()> {
    let target = Target::parse(server, tls_server_name);
    let fetch = match target {
        Ok(target) => spawn_kube(cx, async move { tls::fetch_ca(&target).await }),
        Err(err) => Task::ready(Err(err)),
    };
    cx.spawn(async move |this, cx| {
        let result = fetch.await;
        this.update(cx, |this, cx| {
            let state = match result {
                Ok(fetched) => CaState::Done(fetched),
                Err(err) => CaState::Failed(err),
            };
            on_done(this, state, cx);
            cx.notify();
        })
        .ok();
    })
}

/// The fetched CA with its fingerprint and the trust-on-first-use note.
pub(crate) fn ca_result(state: &CaState, server: &str, colors: &Colors) -> AnyElement {
    match state {
        CaState::Fetching => h_flex()
            .gap(u(8.0))
            .p(u(12.0))
            .child(Icon::new(IconName::RefreshCw).size(14.0).color(colors.accent))
            .child(div().text_size(u(12.5)).child(format!("Fetching the certificate of {server}…")))
            .into_any_element(),
        CaState::Failed(err) => widgets::notice(Severity::Error, format!("Couldn't fetch the CA: {err}"), colors).into_any_element(),
        CaState::Done(fetched) => match fetched.candidates.first() {
            None => widgets::notice(
                Severity::Warning,
                format!(
                    "The server doesn't publish a CA that verifies its certificate{}. Get the CA from your admin or the cloud CLI, and paste it or pick its file.",
                    fetched
                        .cluster_info_error
                        .as_ref()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                ),
                colors,
            )
            .into_any_element(),
            Some(candidate) => candidate_card(candidate, server, colors),
        },
    }
}

fn candidate_card(candidate: &CaCandidate, server: &str, colors: &Colors) -> AnyElement {
    let cert = &candidate.cert;
    let fingerprint = cert.fingerprint();
    let (first, second) = fingerprint.split_at(fingerprint.len() / 2 + 1);
    v_flex()
        .gap(u(10.0))
        .p(u(12.0))
        .rounded(u(7.0))
        .border_1()
        .border_color(colors.border)
        .child(
            h_flex()
                .gap(u(8.0))
                .child(Icon::new(IconName::Shield).size(14.0).color(colors.yellow))
                .child(div().text_size(u(12.5)).child("Fetched from"))
                .child(div().font_family(fonts::MONO).text_size(u(12.0)).child(server.to_string()))
                .child(div().text_size(u(12.0)).text_color(colors.yellow).child("· not trusted yet")),
        )
        .child(widgets::kv(
            vec![
                ("Subject", widgets::mono(cert.subject.clone())),
                (
                    "Issuer",
                    widgets::mono(if cert.self_signed { format!("{} (self-signed)", cert.issuer) } else { cert.issuer.clone() }),
                ),
                (
                    "Valid",
                    widgets::mono(format!(
                        "{} → {}",
                        cert.not_before.strftime("%Y-%m-%d"),
                        cert.not_after.strftime("%Y-%m-%d")
                    )),
                ),
                ("Source", widgets::text(candidate.origin.label())),
                (
                    "SHA-256",
                    v_flex()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(first.trim_end_matches(':').to_string())
                        .child(second.to_string())
                        .into_any_element(),
                ),
                ("Public key", widgets::mono(cert.spki_hash())),
            ],
            colors,
        ))
        .child(widgets::hint(
            "Trust on first use: compare this fingerprint with the CA your admin gave you, or with `kubectl config view --raw` on a machine you trust (or kubeadm's public key hash). No credentials were sent to this server.",
            colors,
        ))
        .into_any_element()
}

struct FetchCaView {
    editor: WeakEntity<KubeconfigEditor>,
    cluster: String,
    server: String,
    state: CaState,
    confirmed: bool,
    focus: FocusHandle,
    _task: Task<()>,
}

/// Fetches the CA of a cluster of the editor's document; confirming the fingerprint writes it
/// as `certificate-authority-data`.
pub(crate) fn fetch_ca_for(
    editor: WeakEntity<KubeconfigEditor>,
    cluster: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(this) = editor.upgrade() else { return };
    let body = this
        .read(cx)
        .doc
        .body(Kind::Cluster, &cluster)
        .cloned()
        .unwrap_or_default();
    let server = model::get_str(&body, &["server"]);
    let tls_name = model::get_str(&body, &["tls-server-name"]);
    let view = cx.new(|cx| {
        let task = start_fetch(
            &server,
            Some(&tls_name),
            cx,
            |this: &mut FetchCaView, state, _| this.state = state,
        );
        FetchCaView {
            editor,
            cluster,
            server,
            state: CaState::Fetching,
            confirmed: false,
            focus: cx.focus_handle(),
            _task: task,
        }
    });
    let focus = view.read(cx).focus.clone();
    open(view, 600.0, Some(focus), window, cx);
}

impl Render for FetchCaView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let candidate = match &self.state {
            CaState::Done(f) => f.candidates.first().cloned(),
            _ => None,
        };
        let weak = cx.entity().downgrade();
        let confirmed = self.confirmed;
        let cluster = self.cluster.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(header(
                IconName::Download,
                format!("Fetch the CA of {}", self.cluster),
                None,
                &colors,
            ))
            .child(
                v_flex()
                    .gap(u(12.0))
                    .p(u(16.0))
                    .child(ca_result(&self.state, &self.server, &colors))
                    .when(candidate.is_some(), |this| {
                        let weak = weak.clone();
                        this.child(widgets::checkbox(
                            "kc-ca-confirm",
                            confirmed,
                            format!("I compared the fingerprint; trust this CA for {cluster}"),
                            &colors,
                            move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.confirmed = !this.confirmed;
                                    cx.notify();
                                })
                                .ok();
                            },
                        ))
                    }),
            )
            .child(footer(
                None,
                vec![
                    cancel_button("kc-ca-cancel"),
                    Button::new("kc-ca-use")
                        .primary()
                        .label("Use this CA")
                        .disabled(!confirmed || candidate.is_none())
                        .on_click(move |_, window, cx| {
                            let Some(this) = weak.upgrade() else { return };
                            let (editor, cluster) = {
                                let this = this.read(cx);
                                (this.editor.clone(), this.cluster.clone())
                            };
                            if let Some(candidate) = candidate.clone() {
                                editor
                                    .update(cx, |editor, cx| {
                                        editor.set_ca(&cluster, &candidate.cert.pem(), window, cx);
                                    })
                                    .ok();
                            }
                            window.close_dialog(cx);
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

impl KubeconfigEditor {
    /// Trusts `pem` as a cluster's CA (after the user confirmed its fingerprint). Skipping TLS
    /// verification can't be combined with a CA, so that goes.
    pub(crate) fn set_ca(
        &mut self,
        cluster: &str,
        pem: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cluster = cluster.to_string();
        let data = certs::data_from_pem(pem);
        self.edit(window, cx, |doc| {
            if let Some(body) = doc.body_mut(Kind::Cluster, &cluster) {
                PemSource::Data(data).write(body, model::CA_FILE, model::CA_DATA);
                body.remove("insecure-skip-tls-verify");
            }
        });
        self.form.modes.remove("ca");
        self.rebuild_form(window, cx);
        NotificationCenter::push(
            cx,
            Notification::success(format!(
                "CA set for {cluster}. Test the connection to check it."
            )),
        );
    }
}

// ----- Save preview -----

struct SaveView {
    editor: WeakEntity<KubeconfigEditor>,
    path: PathBuf,
    draft: bool,
    owned: bool,
    editable: bool,
    allow_opt_in: bool,
    text: String,
    written: yaml::Written,
    diff: kubyl_yaml::diff::LineDiff,
    expected: Option<String>,
    private: bool,
    comments: usize,
    /// Exec plugins that are new or changed by this save.
    new_exec: Vec<(String, ExecSpec)>,
    error: Option<String>,
    saving: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
}

pub(crate) fn save_preview(
    editor: WeakEntity<KubeconfigEditor>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(this) = editor.upgrade() else { return };
    let (path, draft, owned, editable, old, doc, base, expected) = {
        let e = this.read(cx);
        (
            e.path.clone(),
            e.is_draft(),
            e.is_owned(),
            e.is_editable(cx),
            e.snapshot.as_ref().map(|s| s.text.clone()),
            e.doc.clone(),
            e.base.clone(),
            e.snapshot.as_ref().and_then(|s| s.hash.clone()),
        )
    };
    let written = match &old {
        Some(old) => yaml::write(old, &doc.0),
        None => yaml::Written {
            text: yaml::render(&doc.0),
            in_place: false,
            lost_comments: Vec::new(),
        },
    };
    let diff = kubyl_yaml::diff::diff(old.as_deref().unwrap_or(""), &written.text, 3);
    let new_exec = changed_exec(&doc, if draft { None } else { Some(&base) });
    let comments = yaml::comments(&written.text).len();
    let allow_opt_in = Settings::get::<KubeconfigSettings>(cx).allow_external_edits;
    let view = cx.new(|cx| SaveView {
        editor,
        path,
        draft,
        owned,
        editable,
        allow_opt_in,
        text: written.text.clone(),
        written,
        diff,
        expected,
        private: draft || doc.has_inline_credentials(),
        comments,
        new_exec,
        error: None,
        saving: false,
        focus: cx.focus_handle(),
        _task: None,
    });
    let focus = view.read(cx).focus.clone();
    open(view, 780.0, Some(focus), window, cx);
}

/// Exec plugins of `doc` that `base` doesn't have (by user and config).
fn changed_exec(doc: &Doc, base: Option<&Doc>) -> Vec<(String, ExecSpec)> {
    doc.names(Kind::User)
        .into_iter()
        .filter_map(|user| {
            let spec = ExecSpec::read(doc.body(Kind::User, &user)?.get("exec")?)?;
            let old = base
                .and_then(|b| b.body(Kind::User, &user))
                .and_then(|b| b.get("exec"))
                .and_then(ExecSpec::read);
            (old.as_ref() != Some(&spec)).then_some((user, spec))
        })
        .collect()
}

impl SaveView {
    fn save(&mut self, opt_in: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        if opt_in {
            self.editable = true;
        }
        self.saving = true;
        self.error = None;
        let text = self.text.clone();
        let save = editor.update(cx, |editor, cx| editor.save_file(text, opt_in, window, cx));
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = save.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(_) => window.close_dialog(cx),
                    Err(SaveError::Changed { .. }) => {
                        this.error = Some(
                            "The file changed on disk since you opened it (another tool wrote it). Kubyl didn't overwrite it: close this and use Reload or Keep mine."
                                .into(),
                        );
                    }
                    Err(err) => this.error = Some(format!("Couldn't save: {err}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for SaveView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let changes = self.diff.change_count();
        let summary = if self.draft {
            format!("new file · {} lines", self.text.lines().count())
        } else if self.written.in_place {
            format!(
                "{} · comments and key order kept · {} comments",
                if changes == 1 {
                    "1 change".to_string()
                } else {
                    format!("{changes} changes")
                },
                self.comments
            )
        } else {
            format!(
                "{} · the file is written from scratch",
                if changes == 1 {
                    "1 change".to_string()
                } else {
                    format!("{changes} changes")
                }
            )
        };
        let chip = if self.draft {
            "new file"
        } else if self.owned {
            "Kubyl-owned"
        } else {
            "external file"
        };
        let mode = if self.private {
            "mode 0600".to_string()
        } else {
            "mode kept".to_string()
        };
        let backup_line = if self.draft {
            "none (new file)".to_string()
        } else {
            format!(
                "{}{}{}<timestamp>.yaml (0600)",
                display_path(&Kubeconfigs::dirs(cx).backups),
                std::path::MAIN_SEPARATOR,
                files::backup_prefix(&files::target(&self.path))
            )
        };
        let mut body = v_flex()
            .gap(u(12.0))
            .p(u(16.0))
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(Icon::new(IconName::CircleCheck).size(14.0).color(
                        if self.written.in_place || self.draft {
                            colors.green
                        } else {
                            colors.yellow
                        },
                    ))
                    .child(div().text_size(u(12.5)).child(summary)),
            )
            .child(
                div()
                    .id("kc-save-diff")
                    .max_h(u(320.0))
                    .overflow_y_scroll()
                    .rounded(u(6.0))
                    .border_1()
                    .border_color(colors.border)
                    .child(kubyl_yaml::diff_view(&self.diff, false, &colors)),
            );
        if !self.written.lost_comments.is_empty() {
            body = body.child(widgets::notice(
                Severity::Warning,
                format!(
                    "{} comment{} won't be in the saved file: {}",
                    self.written.lost_comments.len(),
                    if self.written.lost_comments.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    self.written
                        .lost_comments
                        .iter()
                        .map(|c| format!("\"# {c}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                &colors,
            ));
        }
        if !self.new_exec.is_empty() {
            body = body.child(widgets::notice(
                Severity::Warning,
                format!(
                    "This file will run {}: Kubyl and kubectl run it whenever you connect.",
                    self.new_exec
                        .iter()
                        .map(|(user, spec)| format!(
                            "`{} {}` (user {user})",
                            spec.command,
                            spec.args.join(" ")
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                &colors,
            ));
        }
        if !self.editable {
            body = body.child(widgets::notice(
                Severity::Info,
                if self.allow_opt_in {
                    "Kubyl doesn't edit this file unless you allow it. Allow editing (with this preview, a backup and an atomic write on every save), or save a Kubyl copy that replaces it as a source."
                } else {
                    "Editing files Kubyl doesn't own is turned off (kubeconfig_editor.allow_external_edits). Save a Kubyl copy instead."
                },
                &colors,
            ));
        }
        body = body.child(widgets::kv(
            vec![
                ("Backup", widgets::mono(backup_line)),
                (
                    "Write",
                    widgets::text(format!("atomic (temp file + rename) · {mode}")),
                ),
                (
                    "Checked",
                    widgets::text(match &self.expected {
                        Some(hash) if !self.draft => format!(
                            "unchanged on disk since you opened it (sha256 {}…)",
                            &hash[..8]
                        ),
                        _ => "the file doesn't exist yet".into(),
                    }),
                ),
            ],
            &colors,
        ));
        if let Some(error) = &self.error {
            body = body.child(widgets::notice(Severity::Error, error.clone(), &colors));
        }
        let weak = cx.entity().downgrade();
        let mut buttons = vec![cancel_button("kc-save-cancel")];
        if !self.owned && !self.draft {
            let editor = self.editor.clone();
            buttons.push(
                Button::new("kc-save-copy")
                    .label("Save as a Kubyl copy…")
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        save_copy(editor.clone(), window, cx);
                    })
                    .into_any_element(),
            );
        }
        if self.editable || self.allow_opt_in {
            let opt_in = !self.editable;
            buttons.push(
                Button::new("kc-save-ok")
                    .primary()
                    .label(if self.saving {
                        "Saving…"
                    } else if opt_in {
                        "Allow editing and save"
                    } else {
                        "Save"
                    })
                    .disabled(self.saving)
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| this.save(opt_in, window, cx))
                            .ok();
                    })
                    .into_any_element(),
            );
        }
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(header(
                IconName::Download,
                format!("Save {}", display_path(&self.path)),
                Some(Chip::new(chip).into_any_element()),
                &colors,
            ))
            .child(body)
            .child(footer(None, buttons, &colors))
    }
}

// ----- Opting in -----

pub(crate) fn opt_in(editor: WeakEntity<KubeconfigEditor>, window: &mut Window, cx: &mut App) {
    let Some(this) = editor.upgrade() else { return };
    let path = this.read(cx).path.clone();
    let mut spec = ConfirmSpec::new(format!("Edit {}?", display_path(&path)), "Edit this file");
    spec.lines = vec![
        "Every save shows the changes first.".into(),
        format!(
            "The old file is kept in {} (0600).",
            display_path(&Kubeconfigs::dirs(cx).backups)
        )
        .into(),
        "Files are replaced atomically; comments and key order stay.".into(),
        "Kubyl refuses to overwrite changes other tools made meanwhile.".into(),
    ];
    spec.note = Some("You can turn this off again from the editor's menu or in settings.json (kubeconfig_editor.editable_files).".into());
    confirm(
        spec,
        move |_, _, cx| crate::settings::set_opt_in(&path, true, cx),
        window,
        cx,
    );
}

// ----- Changed on disk -----

struct DiffView {
    title: String,
    note: String,
    diff: kubyl_yaml::diff::LineDiff,
    confirm: Option<(String, OnClick)>,
    focus: FocusHandle,
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let mut buttons = vec![cancel_button("kc-diff-cancel")];
        if let Some((label, f)) = self.confirm.clone() {
            buttons.push(
                Button::new("kc-diff-ok")
                    .primary()
                    .label(label)
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        f(window, cx);
                    })
                    .into_any_element(),
            );
        }
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(header(IconName::Diff, self.title.clone(), None, &colors))
            .child(
                v_flex()
                    .gap(u(10.0))
                    .p(u(16.0))
                    .child(div().text_size(u(12.5)).child(self.note.clone()))
                    .child(if self.diff.is_empty() {
                        widgets::hint("No differences.", &colors)
                    } else {
                        div()
                            .id("kc-disk-diff")
                            .max_h(u(380.0))
                            .overflow_y_scroll()
                            .rounded(u(6.0))
                            .border_1()
                            .border_color(colors.border)
                            .child(kubyl_yaml::diff_view(&self.diff, false, &colors))
                            .into_any_element()
                    }),
            )
            .child(footer(None, buttons, &colors))
    }
}

fn open_diff(
    title: String,
    note: String,
    diff: kubyl_yaml::diff::LineDiff,
    confirm: Option<(String, OnClick)>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| DiffView {
        title,
        note,
        diff,
        confirm,
        focus: cx.focus_handle(),
    });
    let focus = view.read(cx).focus.clone();
    open(view, 760.0, Some(focus), window, cx);
}

/// Mine vs. what's on disk now: (disk text, my text written over it).
fn mine_vs_disk(editor: &KubeconfigEditor) -> Option<(String, String)> {
    let disk = editor.disk.as_ref()?.text.clone();
    let mine = yaml::write(&disk, &editor.doc.0).text;
    Some((disk, mine))
}

pub(crate) fn disk_diff(editor: WeakEntity<KubeconfigEditor>, window: &mut Window, cx: &mut App) {
    let Some(this) = editor.upgrade() else { return };
    let loaded = this
        .read(cx)
        .snapshot
        .as_ref()
        .map(|s| s.text.clone())
        .unwrap_or_default();
    let Some(disk) = this.read(cx).disk.as_ref().map(|s| s.text.clone()) else {
        return;
    };
    open_diff(
        "Changes on disk".into(),
        "What another tool changed in the file since you opened it:".into(),
        kubyl_yaml::diff::diff(&loaded, &disk, 3),
        None,
        window,
        cx,
    );
}

pub(crate) fn keep_mine(editor: WeakEntity<KubeconfigEditor>, window: &mut Window, cx: &mut App) {
    let Some(this) = editor.upgrade() else { return };
    let Some((disk, mine)) = mine_vs_disk(this.read(cx)) else {
        return;
    };
    let weak = editor.clone();
    open_diff(
        "Keep your version?".into(),
        "When you save, your version replaces the file on disk. These are the differences to what's there now (other tools' changes that would be lost show as removed):".into(),
        kubyl_yaml::diff::diff(&disk, &mine, 3),
        Some((
            "Keep mine".into(),
            Rc::new(move |_, cx| {
                weak.update(cx, |this, cx| this.keep_mine(cx)).ok();
            }),
        )),
        window,
        cx,
    );
}

// ----- Save as a Kubyl copy -----

pub(crate) fn save_copy(editor: WeakEntity<KubeconfigEditor>, window: &mut Window, cx: &mut App) {
    let Some(this) = editor.upgrade() else { return };
    let (path, doc, old) = {
        let e = this.read(cx);
        (
            e.path.clone(),
            e.doc.clone(),
            e.snapshot.as_ref().map(|s| s.text.clone()),
        )
    };
    let kind = crate::editor::source_kind(&path, cx);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kubeconfig".into());
    let stem = if stem == "config" {
        "kube-config".to_string()
    } else {
        stem
    };
    let target = files::new_owned_path(&Kubeconfigs::dirs(cx).owned, &stem);
    let mut spec = ConfirmSpec::new("Save as a Kubyl copy?", "Save copy");
    spec.lines = vec![
        format!("Writes {} (0600).", display_path(&target)).into(),
        format!("{} stays as it is.", display_path(&path)).into(),
        match kind {
            Some(SourceKind::Default) => {
                "Kubyl stops loading ~/.kube/config (turn it back on in Clusters & kubeconfigs)."
                    .into()
            }
            Some(SourceKind::Env) => {
                "Kubyl stops loading $KUBECONFIG and adds its other files as sources.".into()
            }
            Some(SourceKind::User) => "The copy replaces the original as a source.".into(),
            _ => "The copy is loaded as a Kubyl-owned kubeconfig.".into(),
        },
    ];
    spec.note = Some("Kubyl's overrides of its contexts (names, colors, production, read-only) move to the copy.".into());
    confirm(
        spec,
        move |_, window, cx| {
            let text = match &old {
                Some(old) => yaml::write(old, &doc.0).text,
                None => yaml::render(&doc.0),
            };
            let target = target.clone();
            let path = path.clone();
            let contexts = doc.names(Kind::Context);
            let editor = editor.clone();
            let write_target = target.clone();
            let write = cx.background_executor().spawn(async move {
                files::save(
                    &write_target,
                    &text,
                    &SaveOptions {
                        expected: None,
                        backups: None,
                        private: true,
                    },
                )
                .map(|saved| (saved, text))
            });
            window
                .spawn(cx, async move |cx| {
                    let result = write.await;
                    cx.update(|window, cx| match result {
                        Ok((saved, text)) => {
                            replace_source(&path, &target, &contexts, kind, cx);
                            editor
                                .update(cx, |editor, cx| {
                                    editor.moved_to(target.clone(), cx);
                                    editor.saved(saved, text, window, cx);
                                })
                                .ok();
                        }
                        Err(err) => NotificationCenter::push(
                            cx,
                            Notification::error(format!("Couldn't save the copy: {err}")),
                        ),
                    })
                    .ok();
                })
                .detach();
        },
        window,
        cx,
    );
}

/// The copy replaces the original as a source; overrides move along.
fn replace_source(
    original: &Path,
    copy: &Path,
    contexts: &[String],
    kind: Option<SourceKind>,
    cx: &mut App,
) {
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return;
    };
    let env_files: Vec<PathBuf> = manager
        .read(cx)
        .sources()
        .iter()
        .filter(|s| s.spec.kind == SourceKind::Env)
        .flat_map(|s| s.spec.files.clone())
        .filter(|f| f != original)
        .collect();
    manager.update(cx, |m, cx| {
        for context in contexts {
            m.move_context_settings(
                &ContextInfo::make_id(context, original),
                &ContextInfo::make_id(context, copy),
                cx,
            );
        }
        match kind {
            Some(SourceKind::Default) => m.set_load_default_kubeconfig(false, cx),
            Some(SourceKind::Env) => {
                m.set_load_kubeconfig_env(false, cx);
                if !env_files.is_empty() {
                    m.add_sources(env_files, cx);
                }
            }
            Some(SourceKind::User) => m.remove_source(original, cx),
            _ => {}
        }
        m.reload(cx);
    });
}

// ----- Merge, split, export, copy/move -----

pub(crate) fn merge_into(editor: WeakEntity<KubeconfigEditor>, window: &mut Window, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some("Merge".into()),
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let read = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|p| {
                            let doc = files::read(&p)
                                .map_err(|e| e.to_string())
                                .and_then(|s| Doc::parse(&s.text));
                            (p, doc)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            cx.update(|window, cx| {
                editor
                    .update(cx, |this, cx| {
                        let mut merged = 0;
                        for (path, doc) in read {
                            match doc {
                                Ok(other) => {
                                    this.edit(window, cx, |doc| {
                                        for context in other.names(Kind::Context) {
                                            if doc.import_context(&other, &context).is_some() {
                                                merged += 1;
                                            }
                                        }
                                    });
                                }
                                Err(err) => NotificationCenter::push(
                                    cx,
                                    Notification::error(format!("{}: {err}", display_path(&path))),
                                ),
                            }
                        }
                        this.rebuild_form(window, cx);
                        NotificationCenter::push(
                            cx,
                            Notification::info(format!(
                                "Merged {merged} context{} (not saved yet). Exec plugins from them ask before they run.",
                                if merged == 1 { "" } else { "s" }
                            )),
                        );
                    })
                    .ok();
            })
            .ok();
        })
        .detach();
}

pub(crate) fn split(editor: WeakEntity<KubeconfigEditor>, window: &mut Window, cx: &mut App) {
    let Some(this) = editor.upgrade() else { return };
    let doc = this.read(cx).doc.clone();
    let stem = this
        .read(cx)
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kubeconfig".into());
    let contexts = doc.names(Kind::Context);
    if contexts.is_empty() {
        return;
    }
    let dir = Kubeconfigs::dirs(cx).owned;
    let mut spec = ConfirmSpec::new(format!("Split into {} files?", contexts.len()), "Split");
    spec.lines = contexts
        .iter()
        .map(|c| {
            SharedString::from(format!(
                "{} → {}",
                c,
                display_path(&dir.join(format!("{stem}-{c}.yaml")))
            ))
        })
        .collect();
    spec.note = Some("Each file holds one context with its cluster and user (credentials included, 0600). This file stays as it is, so its contexts show up twice until you remove one.".into());
    confirm(
        spec,
        move |_, _, cx| {
            let doc = doc.clone();
            let stem = stem.clone();
            let dir = dir.clone();
            let contexts = contexts.clone();
            let write = cx.background_executor().spawn(async move {
                let mut written = Vec::new();
                for context in &contexts {
                    let Some(part) = doc.extract(context, true) else {
                        continue;
                    };
                    let path = files::new_owned_path(&dir, &format!("{stem}-{context}"));
                    let saved = files::save(
                        &path,
                        &part.to_yaml(),
                        &SaveOptions {
                            expected: None,
                            backups: None,
                            private: true,
                        },
                    );
                    written.push((path, saved.map(|_| ()).map_err(|e| e.to_string())));
                }
                written
            });
            cx.spawn(async move |cx| {
                let written = write.await;
                cx.update(|cx| {
                    let ok = written.iter().filter(|(_, r)| r.is_ok()).count();
                    for (path, result) in &written {
                        if let Err(err) = result {
                            NotificationCenter::push(
                                cx,
                                Notification::error(format!("{}: {err}", display_path(path))),
                            );
                        }
                    }
                    NotificationCenter::push(
                        cx,
                        Notification::success(format!(
                            "Wrote {ok} kubeconfigs to {}",
                            display_path(&Kubeconfigs::dirs(cx).owned)
                        )),
                    );
                    if let Some(m) = ConnectionManager::try_global(cx) {
                        m.update(cx, |m, cx| m.reload(cx));
                    }
                });
            })
            .detach();
        },
        window,
        cx,
    );
}

struct ExportView {
    doc: Doc,
    context: String,
    credentials: bool,
    focus: FocusHandle,
}

/// Exports one context as a standalone kubeconfig: to a file (0600) or the clipboard.
pub(crate) fn export(
    editor: WeakEntity<KubeconfigEditor>,
    context: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(this) = editor.upgrade() else { return };
    let doc = this.read(cx).doc.clone();
    let view = cx.new(|cx| ExportView {
        doc,
        context,
        credentials: false,
        focus: cx.focus_handle(),
    });
    let focus = view.read(cx).focus.clone();
    open(view, 540.0, Some(focus), window, cx);
}

impl ExportView {
    fn text(&self) -> Option<String> {
        Some(self.doc.extract(&self.context, self.credentials)?.to_yaml())
    }
}

impl Render for ExportView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let credentials = self.credentials;
        let has_secrets = self
            .doc
            .extract(&self.context, true)
            .is_some_and(|d| d.has_inline_credentials());
        let text = self.text();
        let copy_text = text.clone();
        let save_text = text.clone();
        let name = self.context.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(header(IconName::Upload, format!("Export {}", self.context), None, &colors))
            .child(
                v_flex()
                    .gap(u(12.0))
                    .p(u(16.0))
                    .child(div().text_size(u(12.5)).child(
                        "A standalone kubeconfig with this context, its cluster and its user.",
                    ))
                    .child(widgets::checkbox(
                        "kc-export-creds",
                        credentials,
                        "Include credentials (tokens, keys, passwords)",
                        &colors,
                        move |_, _, cx| {
                            weak.update(cx, |this, cx| {
                                this.credentials = !this.credentials;
                                cx.notify();
                            })
                            .ok();
                        },
                    ))
                    .when(credentials && has_secrets, |this| {
                        this.child(widgets::notice(
                            Severity::Warning,
                            "The export holds secrets: anyone with the file can use this cluster as you. Files are written 0600; the clipboard isn't protected.",
                            &colors,
                        ))
                    })
                    .when(!credentials && has_secrets, |this| {
                        this.child(widgets::hint(
                            "Without credentials the user entry keeps only what isn't secret (exec plugins, certificate files).",
                            &colors,
                        ))
                    }),
            )
            .child(footer(
                None,
                vec![
                    cancel_button("kc-export-cancel"),
                    Button::new("kc-export-copy")
                        .icon(IconName::Copy)
                        .label("Copy as YAML")
                        .on_click(move |_, window, cx| {
                            if let Some(text) = copy_text.clone() {
                                cx.write_to_clipboard(ClipboardItem::new_string(text));
                                NotificationCenter::push(cx, Notification::info("Copied the kubeconfig to the clipboard."));
                            }
                            window.close_dialog(cx);
                        })
                        .into_any_element(),
                    Button::new("kc-export-save")
                        .primary()
                        .icon(IconName::Download)
                        .label("Save to file…")
                        .on_click(move |_, window, cx| {
                            let Some(text) = save_text.clone() else { return };
                            let dir = dirs::home_dir().unwrap_or_default();
                            let pick = cx.prompt_for_new_path(&dir, Some(&format!("{name}.yaml")));
                            window.close_dialog(cx);
                            cx.spawn(async move |cx| {
                                let Ok(Ok(Some(path))) = pick.await else { return };
                                let target = path.clone();
                                let result = cx
                                    .background_spawn(async move {
                                        // The OS picker already asked before replacing a file.
                                        let expected = files::read(&target).ok().and_then(|s| s.hash);
                                        files::save(&target, &text, &SaveOptions { expected, backups: None, private: true })
                                    })
                                    .await;
                                cx.update(|cx| match result {
                                    Ok(_) => NotificationCenter::push(cx, Notification::success(format!("Exported to {} (0600)", display_path(&path)))),
                                    Err(err) => NotificationCenter::push(cx, Notification::error(format!("Couldn't export: {err}"))),
                                });
                            })
                            .detach();
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}

struct CopyToView {
    editor: WeakEntity<KubeconfigEditor>,
    context: String,
    remove: bool,
    targets: Vec<(PathBuf, bool)>,
    error: Option<String>,
    focus: FocusHandle,
    _task: Option<Task<()>>,
}

/// Copies (or moves) a context with its cluster and user into another kubeconfig. The target
/// is written right away (backup, hash check); a move removes it here (save to finish).
pub(crate) fn copy_to(
    editor: WeakEntity<KubeconfigEditor>,
    context: String,
    remove: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(this) = editor.upgrade() else { return };
    let current = this.read(cx).path.clone();
    let settings = Settings::get::<KubeconfigSettings>(cx).clone();
    let owned_dir = Kubeconfigs::dirs(cx).owned;
    let mut targets: Vec<(PathBuf, bool)> = ConnectionManager::try_global(cx)
        .map(|m| {
            m.read(cx)
                .sources()
                .iter()
                .flat_map(|s| s.files.iter().map(|f| f.path.clone()))
                .filter(|p| p != &current)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let editable = files::is_owned(&p, &owned_dir) || settings.opted_in(&p);
            (p, editable)
        })
        .collect();
    targets.sort();
    let view = cx.new(|cx| CopyToView {
        editor,
        context,
        remove,
        targets,
        error: None,
        focus: cx.focus_handle(),
        _task: None,
    });
    let focus = view.read(cx).focus.clone();
    open(view, 560.0, Some(focus), window, cx);
}

impl CopyToView {
    fn run(&mut self, target: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let doc = editor.read(cx).doc.clone();
        let context = self.context.clone();
        let keep = Settings::get::<KubeconfigSettings>(cx).backups_kept;
        let new_target = target.is_none();
        let target =
            target.unwrap_or_else(|| files::new_owned_path(&Kubeconfigs::dirs(cx).owned, &context));
        let path = target.clone();
        let backup_dir = Kubeconfigs::dirs(cx).backups;
        let write = cx.background_executor().spawn(async move {
            let snapshot = files::read(&path).map_err(|e| e.to_string())?;
            let mut into = if snapshot.hash.is_some() {
                Doc::parse(&snapshot.text)?
            } else {
                Doc::empty()
            };
            let name = into
                .import_context(&doc, &context)
                .ok_or("the context is gone")?;
            let text = if snapshot.hash.is_some() {
                yaml::write(&snapshot.text, &into.0).text
            } else {
                into.to_yaml()
            };
            files::save(
                &path,
                &text,
                &SaveOptions {
                    expected: snapshot.hash,
                    backups: Some(Backups {
                        dir: backup_dir,
                        keep,
                    }),
                    private: new_target || into.has_inline_credentials(),
                },
            )
            .map_err(|e| e.to_string())?;
            Ok::<_, String>(name)
        });
        let remove = self.remove;
        let context = self.context.clone();
        let weak_editor = self.editor.clone();
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(name) => {
                    NotificationCenter::push(
                        cx,
                        Notification::success(format!(
                            "Copied {context} to {} as {name}",
                            display_path(&target)
                        )),
                    );
                    if remove {
                        weak_editor
                            .update(cx, |editor, cx| {
                                editor
                                    .edit(window, cx, |doc| doc.remove_context_with_refs(&context));
                                editor.rebuild_form(window, cx);
                            })
                            .ok();
                        NotificationCenter::push(
                            cx,
                            Notification::info("Removed it here; save to finish the move."),
                        );
                    }
                    if let Some(m) = ConnectionManager::try_global(cx) {
                        m.update(cx, |m, cx| m.reload(cx));
                    }
                    window.close_dialog(cx);
                }
                Err(err) => {
                    this.error = Some(err);
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

impl Render for CopyToView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let mut list = v_flex().gap(u(4.0));
        for (ix, (path, editable)) in self.targets.iter().enumerate() {
            let weak = weak.clone();
            let target = path.clone();
            let editable = *editable;
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("kc-copy-target-{ix}")))
                    .gap(u(8.0))
                    .px(u(10.0))
                    .py(u(6.0))
                    .rounded(u(5.0))
                    .when(editable, |this| {
                        this.cursor_pointer().hover(|s| s.bg(colors.hover))
                    })
                    .when(!editable, |this| this.opacity(0.5))
                    .on_click(move |_, window, cx| {
                        if editable {
                            let target = target.clone();
                            weak.update(cx, |this, cx| this.run(Some(target), window, cx))
                                .ok();
                        }
                    })
                    .child(Icon::new(IconName::File).size(13.0).color(colors.text_dim))
                    .child(
                        div()
                            .flex_1()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .child(display_path(path)),
                    )
                    .when(!editable, |this| {
                        this.child(
                            div()
                                .text_size(u(11.0))
                                .text_color(colors.text_dim)
                                .child("read-only"),
                        )
                    }),
            );
        }
        let new_weak = weak.clone();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(header(
                IconName::Copy,
                format!("{} {} to…", if self.remove { "Move" } else { "Copy" }, self.context),
                None,
                &colors,
            ))
            .child(
                v_flex()
                    .gap(u(10.0))
                    .p(u(16.0))
                    .child(widgets::hint(
                        "The context goes with its cluster and user; taken names get a counter. The target is saved right away (with a backup).",
                        &colors,
                    ))
                    .child(list)
                    .children(self.error.clone().map(|e| widgets::notice(Severity::Error, e, &colors))),
            )
            .child(footer(
                None,
                vec![
                    cancel_button("kc-copy-cancel"),
                    Button::new("kc-copy-new")
                        .primary()
                        .icon(IconName::FilePlus)
                        .label("New Kubyl-owned file")
                        .on_click(move |_, window, cx| {
                            new_weak.update(cx, |this, cx| this.run(None, window, cx)).ok();
                        })
                        .into_any_element(),
                ],
                &colors,
            ))
    }
}
