//! The "New kubeconfig" wizard (board 11): name, cluster (server and CA: system trust, file,
//! pasted PEM or fetched from the server after confirming its fingerprint), credentials,
//! context, test, save (Kubyl-owned by default, or a path the user picks).
//!
//! The test runs in [`Kubeconfigs`], so closing the wizard doesn't cancel it.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    IntoElement, Render, SharedString, Subscription, Task, Window, div, prelude::*,
};
use gpui_component::WindowExt as _;
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::{Notification, NotificationCenter};
use kubyl_kube::ConnectionManager;
use kubyl_kube::kubeconfig::ContextInfo;
use kubyl_kube::settings::display_path;
use kubyl_ui::{ActiveColors, Button, Colors, Icon, IconName, fonts, h_flex, u, v_flex};
use serde_json::{Map, Value, json};

use crate::certs;
use crate::conntest::Input;
use crate::dialogs::{self, CaState};
use crate::files::{self, SaveOptions};
use crate::model::{self, Doc, ExecPreset, Kind};
use crate::panel;
use crate::state::{Kubeconfigs, TestKey};
use crate::validate::Severity;
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Name,
    Cluster,
    Credentials,
    Context,
    Test,
    Save,
}

impl Step {
    const ALL: [Step; 6] = [
        Step::Name,
        Step::Cluster,
        Step::Credentials,
        Step::Context,
        Step::Test,
        Step::Save,
    ];

    fn title(self) -> &'static str {
        match self {
            Step::Name => "Name",
            Step::Cluster => "Cluster",
            Step::Credentials => "Credentials",
            Step::Context => "Context",
            Step::Test => "Test",
            Step::Save => "Save",
        }
    }

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaMode {
    System,
    File,
    Paste,
    Fetch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Auth {
    ClientCertificate,
    Token,
    TokenFile,
    Exec,
    Oidc,
    None,
}

impl Auth {
    const ALL: [Auth; 6] = [
        Auth::ClientCertificate,
        Auth::Token,
        Auth::TokenFile,
        Auth::Exec,
        Auth::Oidc,
        Auth::None,
    ];

    fn label(self) -> &'static str {
        match self {
            Auth::ClientCertificate => "Client certificate",
            Auth::Token => "Token",
            Auth::TokenFile => "Token file",
            Auth::Exec => "Exec plugin",
            Auth::Oidc => "OIDC provider",
            Auth::None => "None",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PemInput {
    File,
    Paste,
}

pub struct Wizard {
    id: u64,
    step: Step,
    inputs: std::collections::HashMap<&'static str, Entity<InputState>>,
    areas: std::collections::HashMap<&'static str, Entity<TextareaState>>,
    pub(crate) ca_mode: CaMode,
    pub(crate) ca: CaState,
    pub(crate) ca_confirmed: bool,
    /// The server the fetched CA belongs to (a changed server needs a new fetch).
    ca_server: String,
    advanced: bool,
    insecure: bool,
    pub(crate) auth: Auth,
    pub(crate) cert_input: PemInput,
    pub(crate) key_input: PemInput,
    exec: Option<Value>,
    /// Where it's saved (`None`: a Kubyl-owned path).
    target: Option<PathBuf>,
    pub(crate) connect: bool,
    open_editor: bool,
    error: Option<String>,
    saving: bool,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Opens the wizard.
pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Wizard::new(window, cx));
    let focus = view.read(cx).inputs["name"].read(cx).focus_handle(cx);
    dialogs::open(view, 660.0, None, window, cx);
    // After opening: the dialog takes focus when it opens.
    window.focus(&focus, cx);
}

const FIELDS: &[(&str, &str, bool)] = &[
    ("name", "kind-dev", false),
    ("cluster", "kind-dev", false),
    ("server", "https://127.0.0.1:6443", false),
    ("ca-file", "/path/to/ca.crt", false),
    ("tls-name", "the name in the server's certificate", false),
    ("proxy", "http://proxy:3128", false),
    ("user", "kind-admin", false),
    ("cert-file", "/path/to/client.crt", false),
    ("key-file", "/path/to/client.key", false),
    ("key-pem", "paste the PEM key", true),
    ("token", "bearer token", true),
    ("token-file", "/path/to/token", false),
    (
        "exec-command",
        "aws, gke-gcloud-auth-plugin, kubelogin…",
        false,
    ),
    (
        "oidc-issuer",
        "https://sso.example.com/realms/platform",
        false,
    ),
    ("oidc-client-id", "kubernetes", false),
    ("oidc-client-secret", "only for confidential clients", true),
    ("context", "kind-dev", false),
    ("namespace", "default", false),
];

impl Wizard {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let id = Kubeconfigs::try_global(cx)
            .map(|g| g.update(cx, |g, _| g.next_id()))
            .unwrap_or_default();
        let mut inputs = std::collections::HashMap::new();
        let mut subscriptions = Vec::new();
        for (key, placeholder, masked) in FIELDS {
            let placeholder = placeholder.to_string();
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(*masked)
            });
            let key = *key;
            subscriptions.push(cx.subscribe_in(
                &state,
                window,
                move |this, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.input_changed(key, state.read(cx).value().to_string(), window, cx);
                    }
                },
            ));
            inputs.insert(key, state);
        }
        let mut areas = std::collections::HashMap::new();
        for (key, placeholder) in [
            ("ca-pem", "-----BEGIN CERTIFICATE-----"),
            ("cert-pem", "-----BEGIN CERTIFICATE-----"),
            ("exec-args", "one argument per line"),
        ] {
            let state = cx.new(|cx| TextareaState::new(window, cx).placeholder(placeholder));
            subscriptions.push(cx.subscribe_in(
                &state,
                window,
                |_, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                },
            ));
            areas.insert(key, state);
        }
        Self {
            id,
            step: Step::Name,
            inputs,
            areas,
            ca_mode: CaMode::Fetch,
            ca: CaState::Failed(String::new()),
            ca_confirmed: false,
            ca_server: String::new(),
            advanced: false,
            insecure: false,
            auth: Auth::ClientCertificate,
            cert_input: PemInput::File,
            key_input: PemInput::File,
            exec: None,
            target: None,
            connect: true,
            open_editor: false,
            error: None,
            saving: false,
            focus: cx.focus_handle(),
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    fn value(&self, key: &str, cx: &App) -> String {
        self.inputs
            .get(key)
            .map(|s| s.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    fn area(&self, key: &str, cx: &App) -> String {
        self.areas
            .get(key)
            .map(|s| s.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub(crate) fn set(&self, key: &str, value: &str, window: &mut Window, cx: &mut App) {
        if let Some(state) = self.inputs.get(key) {
            let value = value.to_string();
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
    }

    /// The name fills in the cluster, user and context names until those are edited.
    pub(crate) fn input_changed(
        &mut self,
        key: &'static str,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if key == "name" {
            for (field, suffix) in [("cluster", ""), ("context", ""), ("user", "-admin")] {
                let current = self.value(field, cx);
                let previous_name = self.inputs["name"].read(cx).value().to_string();
                let _ = previous_name;
                if current.is_empty() || self.follows_name(field, cx) {
                    self.set(field, &format!("{value}{suffix}"), window, cx);
                }
            }
        }
        if key == "server" && self.ca_server != value.trim() {
            // A fetched CA is for the old server.
            if self.ca_mode == CaMode::Fetch {
                self.ca = CaState::Failed(String::new());
                self.ca_confirmed = false;
            }
        }
        self.error = None;
        cx.notify();
    }

    /// Whether `field` still holds what the name put there.
    fn follows_name(&self, field: &str, cx: &App) -> bool {
        let name = self.value("name", cx);
        let current = self.value(field, cx);
        let suffix = if field == "user" { "-admin" } else { "" };
        // While typing, the field holds the name minus the last character.
        current
            .strip_suffix(suffix)
            .is_some_and(|base| name.starts_with(base) && name.len() <= base.len() + 1)
    }

    pub(crate) fn fetch_ca(&mut self, cx: &mut Context<Self>) {
        let server = self.value("server", cx);
        let tls = self.value("tls-name", cx);
        self.ca = CaState::Fetching;
        self.ca_confirmed = false;
        self.ca_server = server.clone();
        let task = dialogs::start_fetch(&server, Some(&tls), cx, |this: &mut Wizard, state, _| {
            this.ca = state
        });
        self._tasks.push(task);
        cx.notify();
    }

    fn fetched_pem(&self) -> Option<String> {
        match &self.ca {
            CaState::Done(f) => f.candidates.first().map(|c| c.cert.pem()),
            _ => None,
        }
    }

    /// Why the current step can't be left yet.
    pub(crate) fn blocker(&self, cx: &App) -> Option<String> {
        match self.step {
            Step::Name => self
                .value("name", cx)
                .is_empty()
                .then(|| "Give the kubeconfig a name.".into()),
            Step::Cluster => {
                if self.value("cluster", cx).is_empty() {
                    return Some("Name the cluster.".into());
                }
                if let Err(err) = crate::tls::Target::parse(&self.value("server", cx), None) {
                    return Some(err);
                }
                match self.ca_mode {
                    CaMode::System => None,
                    CaMode::File => self
                        .value("ca-file", cx)
                        .is_empty()
                        .then(|| "Pick the CA file.".into()),
                    CaMode::Paste => certs::certificates(&self.area("ca-pem", cx)).err(),
                    CaMode::Fetch if self.fetched_pem().is_none() => {
                        Some("Fetch the CA from the server.".into())
                    }
                    CaMode::Fetch if !self.ca_confirmed => {
                        Some("Confirm the fingerprint first.".into())
                    }
                    CaMode::Fetch => None,
                }
            }
            Step::Credentials => {
                if self.value("user", cx).is_empty() {
                    return Some("Name the user.".into());
                }
                match self.auth {
                    Auth::ClientCertificate => {
                        let cert = match self.cert_input {
                            PemInput::File => (!self.value("cert-file", cx).is_empty())
                                .then_some(())
                                .ok_or("Pick the client certificate file."),
                            PemInput::Paste => certs::certificates(&self.area("cert-pem", cx))
                                .map(|_| ())
                                .map_err(|_| "Paste the client certificate (PEM)."),
                        };
                        let key = match self.key_input {
                            PemInput::File => (!self.value("key-file", cx).is_empty())
                                .then_some(())
                                .ok_or("Pick the key file."),
                            PemInput::Paste => certs::check_private_key(&certs::normalize_pem(
                                &self.value("key-pem", cx),
                            ))
                            .map_err(|_| "Paste the private key (PEM)."),
                        };
                        cert.and(key).err().map(String::from)
                    }
                    Auth::Token => self
                        .value("token", cx)
                        .is_empty()
                        .then(|| "Paste the token.".into()),
                    Auth::TokenFile => self
                        .value("token-file", cx)
                        .is_empty()
                        .then(|| "Pick the token file.".into()),
                    Auth::Exec => self
                        .value("exec-command", cx)
                        .is_empty()
                        .then(|| "Enter the plugin's command.".into()),
                    Auth::Oidc => (self.value("oidc-issuer", cx).is_empty()
                        || self.value("oidc-client-id", cx).is_empty())
                    .then(|| "Enter the issuer URL and client ID.".into()),
                    Auth::None => None,
                }
            }
            Step::Context => self
                .value("context", cx)
                .is_empty()
                .then(|| "Name the context.".into()),
            Step::Test | Step::Save => None,
        }
    }

    /// The kubeconfig the wizard describes.
    fn doc(&self, cx: &App) -> Doc {
        let cluster_name = self.value("cluster", cx);
        let user_name = self.value("user", cx);
        let context_name = self.value("context", cx);
        let mut cluster = Map::new();
        cluster.insert("server".into(), json!(self.value("server", cx)));
        match self.ca_mode {
            CaMode::System => {}
            CaMode::File => {
                cluster.insert(model::CA_FILE.into(), json!(self.value("ca-file", cx)));
            }
            CaMode::Paste => {
                cluster.insert(
                    model::CA_DATA.into(),
                    json!(certs::data_from_pem(&self.area("ca-pem", cx))),
                );
            }
            CaMode::Fetch => {
                if let Some(pem) = self.fetched_pem().filter(|_| self.ca_confirmed) {
                    cluster.insert(model::CA_DATA.into(), json!(certs::data_from_pem(&pem)));
                }
            }
        }
        for (key, input) in [("tls-server-name", "tls-name"), ("proxy-url", "proxy")] {
            let value = self.value(input, cx);
            if !value.is_empty() {
                cluster.insert(key.into(), json!(value));
            }
        }
        if self.insecure
            && !cluster.contains_key(model::CA_DATA)
            && !cluster.contains_key(model::CA_FILE)
        {
            cluster.insert("insecure-skip-tls-verify".into(), json!(true));
        }
        let mut user = Map::new();
        match self.auth {
            Auth::ClientCertificate => {
                match self.cert_input {
                    PemInput::File => {
                        user.insert(model::CERT_FILE.into(), json!(self.value("cert-file", cx)))
                    }
                    PemInput::Paste => user.insert(
                        model::CERT_DATA.into(),
                        json!(certs::data_from_pem(&self.area("cert-pem", cx))),
                    ),
                };
                match self.key_input {
                    PemInput::File => {
                        user.insert(model::KEY_FILE.into(), json!(self.value("key-file", cx)))
                    }
                    PemInput::Paste => user.insert(
                        model::KEY_DATA.into(),
                        json!(certs::data_from_pem(&certs::normalize_pem(
                            &self.value("key-pem", cx)
                        ))),
                    ),
                };
            }
            Auth::Token => {
                user.insert("token".into(), json!(self.value("token", cx)));
            }
            Auth::TokenFile => {
                user.insert("tokenFile".into(), json!(self.value("token-file", cx)));
            }
            Auth::Exec => {
                let mut exec = self
                    .exec
                    .clone()
                    .unwrap_or_else(|| json!({"apiVersion": model::EXEC_API_VERSION}));
                exec["command"] = json!(self.value("exec-command", cx));
                let args: Vec<Value> = self
                    .area("exec-args", cx)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(|l| json!(l))
                    .collect();
                match exec.as_object_mut() {
                    Some(obj) if args.is_empty() => {
                        obj.remove("args");
                    }
                    Some(obj) => {
                        obj.insert("args".into(), Value::Array(args));
                    }
                    None => {}
                }
                user.insert("exec".into(), exec);
            }
            Auth::Oidc => {
                let mut config = json!({
                    "idp-issuer-url": self.value("oidc-issuer", cx),
                    "client-id": self.value("oidc-client-id", cx),
                });
                let secret = self.value("oidc-client-secret", cx);
                if !secret.is_empty() {
                    config["client-secret"] = json!(secret);
                }
                user.insert(
                    "auth-provider".into(),
                    json!({"name": "oidc", "config": config}),
                );
            }
            Auth::None => {}
        }
        let mut context = Map::new();
        context.insert("cluster".into(), json!(cluster_name));
        if self.auth != Auth::None {
            context.insert("user".into(), json!(user_name));
        }
        let namespace = self.value("namespace", cx);
        if !namespace.is_empty() {
            context.insert("namespace".into(), json!(namespace));
        }
        let mut doc = Doc::empty();
        doc.add(Kind::Cluster, &cluster_name, cluster).ok();
        if self.auth != Auth::None {
            doc.add(Kind::User, &user_name, user).ok();
        }
        doc.add(Kind::Context, &context_name, context).ok();
        doc.set_current_context(Some(&context_name));
        doc
    }

    fn target_path(&self, cx: &App) -> PathBuf {
        self.target.clone().unwrap_or_else(|| {
            files::new_owned_path(&Kubeconfigs::dirs(cx).owned, &self.value("name", cx))
        })
    }

    pub(crate) fn test_key(&self, cx: &App) -> TestKey {
        TestKey::new(format!("wizard:{}", self.id), self.value("context", cx))
    }

    fn run_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc(cx);
        let context = self.value("context", cx);
        let key = self.test_key(cx);
        let input = Input {
            doc: doc.clone(),
            context: context.clone(),
            file: self.target_path(cx),
            allow_exec: true,
        };
        let global = Kubeconfigs::global(cx);
        match global.read(cx).needs_consent(&doc, None, &context) {
            None => global.update(cx, |g, cx| g.test(key, input, cx)),
            Some(spec) => dialogs::consent(
                vec![(vec![context], spec)],
                move |_, cx| global.update(cx, |g, cx| g.test(key.clone(), input.clone(), cx)),
                window,
                cx,
            ),
        }
        cx.notify();
    }

    pub(crate) fn go(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.step = step;
        self.error = None;
        let first = match step {
            Step::Name => Some("name"),
            Step::Cluster => Some("server"),
            Step::Context => Some("namespace"),
            Step::Credentials | Step::Test | Step::Save => None,
        };
        if let Some(input) = first.and_then(|key| self.inputs.get(key)) {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
        if step == Step::Test {
            let global = Kubeconfigs::global(cx);
            if global.read(cx).report(&self.test_key(cx)).is_none() {
                self.run_test(window, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc(cx);
        let path = self.target_path(cx);
        let external =
            self.target.is_some() && !files::is_owned(&path, &Kubeconfigs::dirs(cx).owned);
        let text = doc.to_yaml();
        let context = self.value("context", cx);
        let connect = self.connect;
        let open_editor = self.open_editor;
        self.saving = true;
        let save_path = path.clone();
        let backup_dir = Kubeconfigs::dirs(cx).backups;
        let keep =
            kubyl_settings::Settings::get::<crate::settings::KubeconfigSettings>(cx).backups_kept;
        let write = cx.background_executor().spawn(async move {
            // The OS picker already asked before replacing a file.
            let expected = files::read(&save_path).ok().and_then(|s| s.hash);
            let backups = expected.is_some().then_some(files::Backups {
                dir: backup_dir,
                keep,
            });
            files::save(
                &save_path,
                &text,
                &SaveOptions {
                    expected,
                    backups,
                    private: true,
                },
            )
        });
        let wizard_key = format!("wizard:{}", self.id);
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(_) => {
                        if external {
                            // Kubyl made it: it may edit it, and it's loaded as a source.
                            crate::settings::set_opt_in(&path, true, cx);
                            if let Some(m) = ConnectionManager::try_global(cx) {
                                m.update(cx, |m, cx| m.add_sources(vec![path.clone()], cx));
                            }
                        }
                        if let Some(m) = ConnectionManager::try_global(cx) {
                            m.update(cx, |m, cx| m.reload(cx));
                        }
                        NotificationCenter::push(
                            cx,
                            Notification::success(format!("Saved {} (0600)", display_path(&path))),
                        );
                        Kubeconfigs::global(cx).update(cx, |g, cx| g.forget(&wizard_key, cx));
                        if connect {
                            connect_when_loaded(ContextInfo::make_id(&context, &path), cx);
                        }
                        if open_editor {
                            crate::actions::open_editor(
                                path.clone(),
                                Some(context.clone()),
                                false,
                                cx,
                            );
                        }
                        window.close_dialog(cx);
                    }
                    Err(err) => this.error = Some(format!("Couldn't save: {err}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn choose_location(&mut self, cx: &mut Context<Self>) {
        let name = format!("{}.yaml", self.value("name", cx));
        let dir = dirs::home_dir()
            .map(|h| h.join(".kube"))
            .filter(|d| d.is_dir())
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let pick = cx.prompt_for_new_path(&dir, Some(&name));
        self._tasks.push(cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = pick.await {
                this.update(cx, |this, cx| {
                    this.target = Some(path);
                    cx.notify();
                })
                .ok();
            }
        }));
    }
}

/// Connects a new context as soon as phase 01 loaded it (up to 10 s).
pub(crate) fn connect_when_loaded(id: kubyl_core::ClusterId, cx: &mut App) {
    cx.spawn(async move |cx| {
        for _ in 0..50 {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let done = cx.update(|cx| {
                let Some(manager) = ConnectionManager::try_global(cx) else {
                    return true;
                };
                if manager.read(cx).context(&id).is_none() {
                    return false;
                }
                manager.update(cx, |m, cx| m.activate(&id, cx));
                true
            });
            if done {
                break;
            }
        }
    })
    .detach();
}

impl Focusable for Wizard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn input(this: &Wizard, key: &str, mono: bool, colors: &Colors) -> AnyElement {
    let secret = FIELDS.iter().any(|(k, _, masked)| *k == key && *masked);
    widgets::text_input(&this.inputs[key], mono, secret, colors).into_any_element()
}

fn area(this: &Wizard, key: &str, height: f32, colors: &Colors) -> AnyElement {
    div()
        .h(u(height))
        .rounded(u(5.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.input_background)
        .font_family(fonts::MONO)
        .text_size(u(11.5))
        .child(Textarea::new(&this.areas[key]).h_full().appearance(false))
        .into_any_element()
}

fn labelled(label: &str, control: AnyElement, colors: &Colors) -> AnyElement {
    v_flex()
        .gap(u(4.0))
        .child(
            div()
                .text_size(u(12.0))
                .text_color(colors.text_dim)
                .child(label.to_string()),
        )
        .child(control)
        .into_any_element()
}

impl Wizard {
    fn stepper(&self, colors: &Colors) -> impl IntoElement {
        let current = self.step.index();
        let mut row = h_flex()
            .gap(u(8.0))
            .px(u(16.0))
            .py(u(12.0))
            .border_b_1()
            .border_color(colors.border_variant);
        for (ix, step) in Step::ALL.iter().enumerate() {
            if ix > 0 {
                row = row.child(div().flex_1().h(u(1.0)).bg(colors.border));
            }
            let done = ix < current;
            let on = ix == current;
            row = row.child(
                h_flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .size(u(18.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(u(10.5))
                            .map(|this| {
                                if done {
                                    this.border_1().border_color(colors.green).child(
                                        Icon::new(IconName::Check).size(11.0).color(colors.green),
                                    )
                                } else if on {
                                    this.bg(colors.accent)
                                        .text_color(colors.on_accent)
                                        .child((ix + 1).to_string())
                                } else {
                                    this.border_1()
                                        .border_color(colors.text_faint)
                                        .text_color(colors.text_dim)
                                        .child((ix + 1).to_string())
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_size(u(12.0))
                            .when(on, |this| this.font_weight(FontWeight::SEMIBOLD))
                            .text_color(if on || done {
                                colors.text
                            } else {
                                colors.text_dim
                            })
                            .child(step.title()),
                    ),
            );
        }
        row
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        match self.step {
            Step::Name => v_flex()
                .gap(u(12.0))
                .child(labelled(
                    "Name",
                    input(self, "name", true, &colors),
                    &colors,
                ))
                .child(widgets::hint(
                    "Names the file and, until you change them, the cluster, user and context.",
                    &colors,
                ))
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .pt(u(8.0))
                        .child(
                            div()
                                .text_size(u(12.0))
                                .text_color(colors.text_dim)
                                .child("Or start from:"),
                        )
                        .child(
                            Button::new("wiz-sa")
                                .ghost()
                                .icon(IconName::User)
                                .label("A service account…")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                    crate::import::service_account(window, cx);
                                }),
                        )
                        .child(
                            Button::new("wiz-cloud")
                                .ghost()
                                .icon(IconName::Cloud)
                                .label("A cloud CLI…")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                    crate::import::cloud(window, cx);
                                }),
                        ),
                )
                .into_any_element(),
            Step::Cluster => {
                let mode = self.ca_mode;
                let seg = {
                    let weak = weak.clone();
                    widgets::segmented(
                        "wiz-ca",
                        vec![
                            (
                                CaMode::System,
                                SharedString::from("System trust store"),
                                Some(IconName::Shield),
                            ),
                            (CaMode::File, "File".into(), Some(IconName::File)),
                            (CaMode::Paste, "Paste PEM".into(), Some(IconName::Code)),
                            (
                                CaMode::Fetch,
                                "Fetch from server".into(),
                                Some(IconName::Download),
                            ),
                        ],
                        mode,
                        &colors,
                        move |mode, _, cx| {
                            let mode = *mode;
                            weak.update(cx, |this, cx| {
                                this.ca_mode = mode;
                                cx.notify();
                            })
                            .ok();
                        },
                    )
                };
                let server = self.value("server", cx);
                let ca_body: AnyElement = match mode {
                    CaMode::System => widgets::hint(
                        "The server's certificate must be signed by a CA your system trusts (public clouds with a public CA).",
                        &colors,
                    ),
                    CaMode::File => input(self, "ca-file", true, &colors),
                    CaMode::Paste => area(self, "ca-pem", 110.0, &colors),
                    CaMode::Fetch => {
                        let fetched = matches!(self.ca, CaState::Done(_) | CaState::Fetching)
                            && self.ca_server == server;
                        let mut col = v_flex().gap(u(10.0));
                        if fetched {
                            col = col.child(dialogs::ca_result(&self.ca, &server, &colors));
                        } else if let CaState::Failed(err) = &self.ca
                            && !err.is_empty()
                        {
                            col = col.child(dialogs::ca_result(&self.ca, &server, &colors));
                        }
                        if self.fetched_pem().is_some() && self.ca_server == server {
                            let weak = weak.clone();
                            let cluster = self.value("cluster", cx);
                            col = col.child(widgets::checkbox(
                                "wiz-ca-confirm",
                                self.ca_confirmed,
                                format!("I compared the fingerprint; trust this CA for {cluster}"),
                                &colors,
                                move |_, _, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.ca_confirmed = !this.ca_confirmed;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                            ));
                        } else if !matches!(self.ca, CaState::Fetching) {
                            let weak = weak.clone();
                            col = col.child(
                                Button::new("wiz-fetch")
                                    .icon(IconName::Download)
                                    .label("Fetch the CA from the server")
                                    .disabled(crate::tls::Target::parse(&server, None).is_err())
                                    .on_click(move |_, _, cx| {
                                        weak.update(cx, |this, cx| this.fetch_ca(cx)).ok();
                                    }),
                            );
                        }
                        col.into_any_element()
                    }
                };
                let advanced = self.advanced;
                let insecure = self.insecure;
                v_flex()
                    .gap(u(12.0))
                    .child(
                        h_flex()
                            .gap(u(12.0))
                            .child(div().w(u(200.0)).child(labelled("Cluster name", input(self, "cluster", true, &colors), &colors)))
                            .child(div().flex_1().child(labelled("API server", input(self, "server", true, &colors), &colors))),
                    )
                    .child(labelled("Certificate authority", seg.into_any_element(), &colors))
                    .child(ca_body)
                    .child(
                        h_flex()
                            .id("wiz-advanced")
                            .gap(u(6.0))
                            .cursor_pointer()
                            .on_click({
                                let weak = weak.clone();
                                move |_, _, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.advanced = !this.advanced;
                                        cx.notify();
                                    })
                                    .ok();
                                }
                            })
                            .child(Icon::new(if advanced { IconName::ChevronDown } else { IconName::ChevronRight }).size(12.0).color(colors.text_dim))
                            .child(div().text_size(u(12.5)).child("Advanced"))
                            .when(!advanced, |this| {
                                this.child(div().text_size(u(11.5)).text_color(colors.text_dim).child("TLS server name · Proxy URL · Skip TLS verification"))
                                    .child(div().text_size(u(11.5)).text_color(colors.red).child("(insecure)"))
                            }),
                    )
                    .when(advanced, |this| {
                        let weak = weak.clone();
                        this.child(labelled("TLS server name", input(self, "tls-name", true, &colors), &colors))
                            .child(labelled("Proxy URL", input(self, "proxy", true, &colors), &colors))
                            .child(widgets::option_row(
                                "wiz-insecure",
                                "Skip TLS verification",
                                "Insecure: anyone on the network path can read your credentials. Fetching the CA is safer.",
                                insecure,
                                mode == CaMode::System,
                                &colors,
                                move |_, _, cx| {
                                    weak.update(cx, |this, cx| {
                                        this.insecure = !this.insecure;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                            ))
                            .when(insecure && mode == CaMode::System, |this| {
                                this.child(widgets::notice(Severity::Error, "TLS won't be verified for this cluster.", &colors))
                            })
                    })
                    .into_any_element()
            }
            Step::Credentials => self.credentials(window, cx),
            Step::Context => v_flex()
                .gap(u(12.0))
                .child(labelled(
                    "Context name",
                    input(self, "context", true, &colors),
                    &colors,
                ))
                .child(labelled(
                    "Namespace",
                    input(self, "namespace", true, &colors),
                    &colors,
                ))
                .child(widgets::hint(
                    format!(
                        "{} with {}; it becomes the file's current context.",
                        self.value("cluster", cx),
                        if self.auth == Auth::None {
                            "no user".into()
                        } else {
                            self.value("user", cx)
                        }
                    ),
                    &colors,
                ))
                .into_any_element(),
            Step::Test => {
                let key = self.test_key(cx);
                let global = Kubeconfigs::global(cx);
                let report = global.read(cx).report(&key).cloned();
                let sign_in = global.read(cx).sign_in_state(&key).cloned();
                match report {
                    None => widgets::hint("Starting…", &colors),
                    Some(report) => {
                        let mut handlers = panel::Handlers::default();
                        {
                            let weak = weak.clone();
                            handlers.retest = Some(Rc::new(move |window, cx| {
                                weak.update(cx, |this, cx| this.run_test(window, cx)).ok();
                            }));
                        }
                        {
                            let weak = weak.clone();
                            handlers.fetch_ca = Some(Rc::new(move |_, cx| {
                                weak.update(cx, |this, cx| {
                                    this.step = Step::Cluster;
                                    this.ca_mode = CaMode::Fetch;
                                    this.fetch_ca(cx);
                                })
                                .ok();
                            }));
                        }
                        {
                            let weak = weak.clone();
                            handlers.edit_user = Some(Rc::new(move |_, cx| {
                                weak.update(cx, |this, cx| {
                                    this.step = Step::Credentials;
                                    cx.notify();
                                })
                                .ok();
                            }));
                        }
                        if let Some(oidc) = report.oidc.clone() {
                            let key2 = key.clone();
                            handlers.sign_in = Some(Rc::new(move |_, cx| {
                                let (key, oidc) = (key2.clone(), oidc.clone());
                                Kubeconfigs::global(cx)
                                    .update(cx, |g, cx| g.sign_in(key, oidc, cx));
                            }));
                        }
                        {
                            let weak = weak.clone();
                            handlers.namespace = Some(Rc::new(move |ns, window, cx| {
                                let ns = ns.to_string();
                                weak.update(cx, |this, cx| {
                                    this.set("namespace", &ns, window, cx);
                                    cx.notify();
                                })
                                .ok();
                            }));
                        }
                        div()
                            .id("wiz-test")
                            .max_h(u(430.0))
                            .overflow_y_scroll()
                            .child(panel::report(
                                "wiz-report",
                                &report,
                                Some("not saved yet".into()),
                                sign_in.as_ref(),
                                &handlers,
                                &colors,
                            ))
                            .into_any_element()
                    }
                }
            }
            Step::Save => {
                let path = self.target_path(cx);
                let owned = self.target.is_none();
                let weak2 = weak.clone();
                let weak3 = weak.clone();
                v_flex()
                    .gap(u(12.0))
                    .child(labelled(
                        "Save to",
                        h_flex()
                            .gap(u(8.0))
                            .child(div().flex_1().font_family(fonts::MONO).text_size(u(12.5)).child(display_path(&path)))
                            .child({
                                let weak = weak.clone();
                                Button::new("wiz-location").ghost().label("Choose a location…").on_click(move |_, _, cx| {
                                    weak.update(cx, |this, cx| this.choose_location(cx)).ok();
                                })
                            })
                            .into_any_element(),
                        &colors,
                    ))
                    .child(widgets::hint(
                        if owned {
                            "A Kubyl-owned kubeconfig (mode 0600): Kubyl loads it and edits it freely. kubectl can use it with --kubeconfig."
                        } else {
                            "Kubyl adds this file as a source and may edit it (you created it here). Mode 0600."
                        },
                        &colors,
                    ))
                    .child(widgets::checkbox("wiz-connect", self.connect, "Connect to it after saving", &colors, move |_, _, cx| {
                        weak2.update(cx, |this, cx| {
                            this.connect = !this.connect;
                            cx.notify();
                        })
                        .ok();
                    }))
                    .child(widgets::checkbox("wiz-open", self.open_editor, "Open it in the kubeconfig editor", &colors, move |_, _, cx| {
                        weak3.update(cx, |this, cx| {
                            this.open_editor = !this.open_editor;
                            cx.notify();
                        })
                        .ok();
                    }))
                    .into_any_element()
            }
        }
    }

    fn credentials(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let auth = self.auth;
        let picker = {
            let weak = weak.clone();
            MenuButton::new("wiz-auth")
                .outline()
                .child(
                    h_flex()
                        .gap(u(6.0))
                        .text_size(u(12.5))
                        .child(auth.label())
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for choice in Auth::ALL {
                        let weak = weak.clone();
                        menu = menu.item(PopupMenuItem::new(choice.label()).on_click(
                            move |_, _, cx| {
                                weak.update(cx, |this, cx| {
                                    this.auth = choice;
                                    cx.notify();
                                })
                                .ok();
                            },
                        ));
                    }
                    menu
                })
        };
        let pem_seg = |id: &'static str,
                       which: &'static str,
                       value: PemInput,
                       weak: gpui::WeakEntity<Wizard>| {
            widgets::segmented(
                id,
                vec![
                    (
                        PemInput::File,
                        SharedString::from("File"),
                        Some(IconName::File),
                    ),
                    (PemInput::Paste, "Paste".into(), Some(IconName::Code)),
                ],
                value,
                &colors,
                move |v, _, cx| {
                    let v = *v;
                    weak.update(cx, |this, cx| {
                        if which == "cert" {
                            this.cert_input = v;
                        } else {
                            this.key_input = v;
                        }
                        cx.notify();
                    })
                    .ok();
                },
            )
        };
        let mut col = v_flex().gap(u(12.0)).child(
            h_flex()
                .gap(u(12.0))
                .child(div().w(u(220.0)).child(labelled(
                    "User name",
                    input(self, "user", true, &colors),
                    &colors,
                )))
                .child(labelled(
                    "Authentication",
                    picker.into_any_element(),
                    &colors,
                )),
        );
        match auth {
            Auth::ClientCertificate => {
                let cert = match self.cert_input {
                    PemInput::File => input(self, "cert-file", true, &colors),
                    PemInput::Paste => area(self, "cert-pem", 90.0, &colors),
                };
                let key = match self.key_input {
                    PemInput::File => input(self, "key-file", true, &colors),
                    PemInput::Paste => input(self, "key-pem", true, &colors),
                };
                let cert_info = match self.cert_input {
                    PemInput::Paste => certs::certificates(&self.area("cert-pem", cx)).ok(),
                    PemInput::File => std::fs::read_to_string(model::resolve_path(
                        &self.value("cert-file", cx),
                        None,
                    ))
                    .ok()
                    .and_then(|p| certs::certificates(&p).ok()),
                };
                col = col
                    .child(labelled(
                        "Client certificate",
                        v_flex()
                            .gap(u(6.0))
                            .child(pem_seg("wiz-cert", "cert", self.cert_input, weak.clone()))
                            .child(cert)
                            .into_any_element(),
                        &colors,
                    ))
                    .children(
                        cert_info
                            .and_then(|c| c.into_iter().next())
                            .map(|c| crate::forms::cert_card(&c, &colors)),
                    )
                    .child(labelled(
                        "Client key",
                        v_flex()
                            .gap(u(6.0))
                            .child(pem_seg("wiz-key", "key", self.key_input, weak.clone()))
                            .child(key)
                            .into_any_element(),
                        &colors,
                    ));
            }
            Auth::Token => {
                col = col.child(labelled("Token", input(self, "token", true, &colors), &colors)).child(widgets::hint(
                    "For service accounts prefer a TokenRequest with an expiry (Kubeconfig: New from Service Account…) over a long-lived token Secret.",
                    &colors,
                ));
            }
            Auth::TokenFile => {
                col = col.child(labelled(
                    "Token file",
                    input(self, "token-file", true, &colors),
                    &colors,
                ))
            }
            Auth::Exec => {
                let presets = {
                    let weak = weak.clone();
                    MenuButton::new("wiz-preset")
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
                                            let exec = preset.exec();
                                            let command = exec["command"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string();
                                            let args = exec["args"]
                                                .as_array()
                                                .map(|a| {
                                                    a.iter()
                                                        .filter_map(|v| v.as_str())
                                                        .collect::<Vec<_>>()
                                                        .join("\n")
                                                })
                                                .unwrap_or_default();
                                            this.set("exec-command", &command, window, cx);
                                            this.areas["exec-args"]
                                                .update(cx, |s, cx| s.set_value(args, window, cx));
                                            this.exec = Some(exec);
                                            cx.notify();
                                        })
                                        .ok();
                                    },
                                ));
                            }
                            menu
                        })
                };
                col = col
                    .child(h_flex().gap(u(8.0)).child(div().flex_1().child(labelled("Command", input(self, "exec-command", true, &colors), &colors))).child(presets))
                    .child(labelled("Arguments", area(self, "exec-args", 90.0, &colors), &colors))
                    .child(widgets::hint(
                        "The test asks before it runs the plugin. Environment, interactive mode and cluster info can be set in the editor.",
                        &colors,
                    ));
            }
            Auth::Oidc => {
                col = col
                    .child(labelled(
                        "Issuer URL",
                        input(self, "oidc-issuer", true, &colors),
                        &colors,
                    ))
                    .child(labelled(
                        "Client ID",
                        input(self, "oidc-client-id", true, &colors),
                        &colors,
                    ))
                    .child(labelled(
                        "Client secret",
                        input(self, "oidc-client-secret", true, &colors),
                        &colors,
                    ));
            }
            Auth::None => col = col.child(widgets::hint("Requests are anonymous.", &colors)),
        }
        col.into_any_element()
    }
}

impl Render for Wizard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let weak = cx.entity().downgrade();
        let step = self.step;
        let blocker = self.blocker(cx);
        let body = self.body(window, cx);
        let path = self.target_path(cx);
        let report_failed = step == Step::Test
            && Kubeconfigs::global(cx)
                .read(cx)
                .report(&self.test_key(cx))
                .is_some_and(|r| r.done && !r.passed());
        let mut buttons = vec![dialogs::cancel_button("wiz-cancel")];
        if step != Step::Name {
            let weak = weak.clone();
            buttons.push(
                Button::new("wiz-back")
                    .icon(IconName::ArrowLeft)
                    .label("Back")
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |this, cx| {
                            let prev = Step::ALL[this.step.index().saturating_sub(1)];
                            this.go(prev, window, cx);
                        })
                        .ok();
                    })
                    .into_any_element(),
            );
        }
        let next_label = match step {
            Step::Save => {
                if self.saving {
                    "Saving…".to_string()
                } else {
                    "Save".to_string()
                }
            }
            Step::Test if report_failed => "Save anyway".into(),
            _ => format!("Next: {}", Step::ALL[(step.index() + 1).min(5)].title()),
        };
        buttons.push(
            Button::new("wiz-next")
                .primary()
                .label(next_label)
                .disabled(blocker.is_some() || self.saving)
                .on_click(move |_, window, cx| {
                    weak.update(cx, |this, cx| {
                        if this.step == Step::Save {
                            this.save(window, cx);
                        } else {
                            let next = Step::ALL[this.step.index() + 1];
                            this.go(next, window, cx);
                        }
                    })
                    .ok();
                })
                .into_any_element(),
        );
        let note = h_flex()
            .gap(u(6.0))
            .max_w(u(300.0))
            .child(Icon::new(IconName::Lock).size(12.0).color(colors.text_dim))
            .child(
                div()
                    .text_size(u(11.0))
                    .text_color(colors.text_dim)
                    .child(format!("Saved to {} (0600)", display_path(&path))),
            )
            .into_any_element();
        v_flex()
            .track_focus(&self.focus)
            .text_color(colors.text)
            .child(dialogs::header(
                IconName::FilePlus,
                "New kubeconfig",
                Some(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(format!("Step {} of 6", step.index() + 1))
                        .into_any_element(),
                ),
                &colors,
            ))
            .child(self.stepper(&colors))
            .child(
                v_flex()
                    .p(u(16.0))
                    .gap(u(10.0))
                    .child(body)
                    .children(
                        blocker
                            .filter(|_| step != Step::Name)
                            .map(|b| widgets::hint(b, &colors)),
                    )
                    .children(
                        self.error
                            .clone()
                            .map(|e| widgets::notice(Severity::Error, e, &colors)),
                    ),
            )
            .child(dialogs::footer(Some(note), buttons, &colors))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn setup(cx: &mut TestAppContext) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            gpui_component::init(cx);
            Kubeconfigs::install_with(
                crate::state::Dirs {
                    owned: dir.path().join("kubeconfigs"),
                    backups: dir.path().join("backups"),
                },
                cx,
            );
        });
        dir
    }

    #[gpui::test]
    fn builds_a_kubeconfig_from_the_steps(cx: &mut TestAppContext) {
        let _dir = setup(cx);
        let (wizard, cx) = cx.add_window_view(Wizard::new);
        wizard.update_in(cx, |this, window, cx| {
            this.set("name", "lab", window, cx);
            this.input_changed("name", "lab".into(), window, cx);
            assert_eq!(this.value("cluster", cx), "lab");
            assert_eq!(this.value("user", cx), "lab-admin");
            this.go(Step::Cluster, window, cx);
            this.set("server", "https://127.0.0.1:6443", window, cx);
            // Fetch mode: can't go on without a fetched, confirmed CA.
            assert!(this.blocker(cx).is_some());
            this.ca_mode = CaMode::System;
            assert_eq!(this.blocker(cx), None);
            this.go(Step::Credentials, window, cx);
            this.auth = Auth::Token;
            this.set("token", "s3cret", window, cx);
            assert_eq!(this.blocker(cx), None);
            let doc = this.doc(cx);
            assert_eq!(doc.current_context(), Some("lab"));
            assert_eq!(
                doc.context_refs("lab"),
                (Some("lab".into()), Some("lab-admin".into()))
            );
            assert_eq!(
                doc.body(Kind::User, "lab-admin").unwrap()["token"],
                "s3cret"
            );
            assert!(doc.has_inline_credentials());
            let path = this.target_path(cx);
            assert!(path.ends_with("kubeconfigs/lab.yaml"), "{path:?}");
        });
    }
}
