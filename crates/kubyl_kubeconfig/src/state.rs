//! The app-wide [`Kubeconfigs`] entity: connection tests, OIDC sign-ins started from a test,
//! exec-plugin consent, and unsaved new documents.
//!
//! Work that has to outlive a dialog or a tab lives here: closing the wizard doesn't cancel a
//! test, and a sign-in waiting for the browser keeps waiting.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use kubyl_core::{Notification, NotificationCenter, spawn_kube};
use kubyl_kube::auth::OidcAuth;
use kubyl_kube::auth::oidc::{SignInEvent, SignInMethod};

use crate::conntest::{self, Input, Report};
use crate::model::{self, Doc, ExecSpec};

/// Which test: a document (its path, or `draft:<n>` / `wizard:<n>`) and a context.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TestKey {
    pub doc: String,
    pub context: String,
}

impl TestKey {
    pub fn new(doc: impl Into<String>, context: impl Into<String>) -> Self {
        Self {
            doc: doc.into(),
            context: context.into(),
        }
    }
}

struct TestRun {
    report: Report,
    input: Input,
    _tasks: Vec<Task<()>>,
}

/// The progress of an OIDC sign-in started from a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInState {
    Starting,
    WaitingForBrowser {
        url: String,
    },
    DeviceCode {
        user_code: String,
        verification_uri: String,
    },
    Failed(String),
}

struct SignInRun {
    state: SignInState,
    _tasks: Vec<Task<()>>,
}

/// A new document that isn't saved yet (an editor tab shows it).
#[derive(Clone)]
pub struct Draft {
    pub doc: Doc,
    /// Where it's saved by default (a Kubyl-owned path).
    pub path: PathBuf,
    pub title: String,
}

#[derive(Default)]
pub struct Kubeconfigs {
    tests: HashMap<TestKey, TestRun>,
    sign_ins: HashMap<TestKey, SignInRun>,
    /// Exec plugins the user agreed to run in this session ([`ExecSpec::consent_key`]).
    consents: HashSet<u64>,
    drafts: HashMap<u64, Draft>,
    /// "Select this context (and test it)" for an editor that opens or is open.
    pending: HashMap<PathBuf, (Option<String>, bool)>,
    next_id: u64,
}

struct GlobalKubeconfigs(Entity<Kubeconfigs>);

impl Global for GlobalKubeconfigs {}

impl Kubeconfigs {
    pub fn install(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalKubeconfigs(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalKubeconfigs>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalKubeconfigs>().map(|g| g.0.clone())
    }

    /// A fresh id for drafts and wizards.
    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    // ----- Tests -----

    pub fn report(&self, key: &TestKey) -> Option<&Report> {
        self.tests.get(key).map(|t| &t.report)
    }

    /// Every report of a document, by context.
    pub fn reports_of<'a>(&'a self, doc: &'a str) -> impl Iterator<Item = (&'a str, &'a Report)> {
        self.tests
            .iter()
            .filter(move |(k, _)| k.doc == doc)
            .map(|(k, t)| (k.context.as_str(), &t.report))
    }

    pub fn is_running(&self, key: &TestKey) -> bool {
        self.report(key).is_some_and(|r| !r.done)
    }

    /// Starts (or restarts) a test. The caller has checked exec consent
    /// ([`Self::needs_consent`]) and sets `input.allow_exec`.
    pub fn test(&mut self, key: TestKey, input: Input, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded::<Report>();
        let run_input = input.clone();
        let run = spawn_kube(cx, async move {
            conntest::run(run_input, tx).await;
        });
        let task_key = key.clone();
        let updates = cx.spawn(async move |this, cx| {
            while let Some(mut report) = rx.next().await {
                // Only the latest state matters.
                while let Ok(next) = rx.try_recv() {
                    report = next;
                }
                let alive = this.update(cx, |this, cx| {
                    if let Some(run) = this.tests.get_mut(&task_key) {
                        run.report = report;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let report = Report::new(&key.context);
        self.tests.insert(
            key,
            TestRun {
                report,
                input,
                _tasks: vec![run, updates],
            },
        );
        cx.notify();
    }

    /// Runs the last test of `key` again (after a sign-in or a fix).
    pub fn retest(&mut self, key: &TestKey, cx: &mut Context<Self>) {
        if let Some(input) = self.tests.get(key).map(|t| t.input.clone()) {
            self.test(key.clone(), input, cx);
        }
    }

    /// Forgets the tests of a document (its tab closed, or it was saved under a new path).
    pub fn forget(&mut self, doc: &str, cx: &mut Context<Self>) {
        self.tests.retain(|k, _| k.doc != doc);
        self.sign_ins.retain(|k, _| k.doc != doc);
        cx.notify();
    }

    // ----- Exec consent -----

    /// The exec plugin of `context` when running it needs the user's OK: it isn't the one the
    /// saved, loaded file has (`saved`), and the user didn't agree to it in this session.
    /// `interactiveMode` doesn't matter.
    pub fn needs_consent(&self, doc: &Doc, saved: Option<&Doc>, context: &str) -> Option<ExecSpec> {
        let (user, spec) = model::exec_of(doc, context)?;
        let saved_spec = saved
            .and_then(|s| s.body(model::Kind::User, &user))
            .and_then(|body| body.get("exec"))
            .and_then(ExecSpec::read);
        if saved_spec.as_ref() == Some(&spec) || self.consents.contains(&spec.consent_key()) {
            return None;
        }
        Some(spec)
    }

    pub fn consent(&mut self, spec: &ExecSpec) {
        self.consents.insert(spec.consent_key());
    }

    // ----- OIDC sign-in -----

    pub fn sign_in_state(&self, key: &TestKey) -> Option<&SignInState> {
        self.sign_ins.get(key).map(|s| &s.state)
    }

    /// Signs in with the report's OIDC session in the browser, then runs the test again.
    pub fn sign_in(&mut self, key: TestKey, auth: Arc<OidcAuth>, cx: &mut Context<Self>) {
        let (tx, mut rx) = mpsc::unbounded::<SignInEvent>();
        let sign_in = spawn_kube(
            cx,
            async move { auth.sign_in(SignInMethod::Browser, tx).await },
        );
        let events_key = key.clone();
        let events = cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let state = match event {
                    SignInEvent::WaitingForBrowser { url, .. } => {
                        SignInState::WaitingForBrowser { url }
                    }
                    SignInEvent::DeviceCode {
                        user_code,
                        verification_uri,
                        ..
                    } => SignInState::DeviceCode {
                        user_code,
                        verification_uri,
                    },
                };
                let alive = this.update(cx, |this, cx| {
                    if let Some(run) = this.sign_ins.get_mut(&events_key) {
                        run.state = state;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let task_key = key.clone();
        let done = cx.spawn(async move |this, cx| {
            let result = sign_in.await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.sign_ins.remove(&task_key);
                    this.retest(&task_key, cx);
                }
                Err(err) => {
                    NotificationCenter::push(
                        cx,
                        Notification::error(format!("Sign-in failed: {err}")),
                    );
                    if let Some(run) = this.sign_ins.get_mut(&task_key) {
                        run.state = SignInState::Failed(err.to_string());
                    }
                    cx.notify();
                }
            })
            .ok();
        });
        self.sign_ins.insert(
            key,
            SignInRun {
                state: SignInState::Starting,
                _tasks: vec![events, done],
            },
        );
        cx.notify();
    }

    pub fn cancel_sign_in(&mut self, key: &TestKey, cx: &mut Context<Self>) {
        self.sign_ins.remove(key);
        cx.notify();
    }

    // ----- Opening editors -----

    /// Asks the editor of `path` to select `context` and optionally test it.
    pub fn request(
        &mut self,
        path: PathBuf,
        context: Option<String>,
        test: bool,
        cx: &mut Context<Self>,
    ) {
        self.pending.insert(path, (context, test));
        cx.notify();
    }

    pub fn take_pending(&mut self, path: &std::path::Path) -> Option<(Option<String>, bool)> {
        self.pending.remove(path)
    }

    // ----- Drafts -----

    pub fn add_draft(&mut self, draft: Draft) -> u64 {
        let id = self.next_id();
        self.drafts.insert(id, draft);
        id
    }

    pub fn take_draft(&mut self, id: u64) -> Option<Draft> {
        self.drafts.remove(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_is_needed_for_unsaved_exec_plugins() {
        let saved = Doc::parse(
            "contexts:\n- name: c\n  context: {cluster: x, user: u}\nusers:\n- name: u\n  user:\n    exec: {command: aws, args: [eks, get-token], apiVersion: client.authentication.k8s.io/v1beta1}\n",
        )
        .unwrap();
        let mut state = Kubeconfigs::default();
        // Saved and unchanged: phase 01 runs it anyway.
        assert!(state.needs_consent(&saved, Some(&saved), "c").is_none());
        // Edited args: consent.
        let mut edited = saved.clone();
        let body = edited.body_mut(model::Kind::User, "u").unwrap();
        body["exec"]["args"] = serde_json::json!(["eks", "get-token", "--profile", "evil"]);
        let spec = state.needs_consent(&edited, Some(&saved), "c").unwrap();
        assert_eq!(spec.args.len(), 4);
        // A new document (wizard, import): consent.
        assert!(state.needs_consent(&saved, None, "c").is_some());
        state.consent(&spec);
        assert!(state.needs_consent(&edited, Some(&saved), "c").is_none());
        // No exec plugin: nothing to ask.
        assert!(state.needs_consent(&Doc::empty(), None, "c").is_none());
    }
}
