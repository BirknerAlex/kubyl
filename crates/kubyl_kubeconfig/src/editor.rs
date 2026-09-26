//! The kubeconfig editor tab (board 11): one file at a time, a list of contexts, clusters and
//! users on the left, a form or the raw YAML on the right, "Test connection" from the unsaved
//! edits, and saving with a preview, a backup and a changed-on-disk check.
//!
//! Rendering is in [`crate::editor_ui`], the forms in [`crate::forms`], the YAML tab in
//! [`crate::yaml_tab`], the dialogs in [`crate::dialogs`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, SharedString, Subscription,
    Task, Window, actions,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::{ClusterId, Notification, NotificationCenter, TabView, ViewKind, ViewRequest};
use kubyl_kube::ConnectionManager;
use kubyl_kube::kubeconfig::{ContextInfo, SourceKind};
use kubyl_kube::settings::display_path;
use kubyl_settings::Settings;

use crate::conntest::Input;
use crate::files::{self, Snapshot};
use crate::forms::Form;
use crate::model::{Doc, EntryRef, Kind};
use crate::settings::KubeconfigSettings;
use crate::state::{Draft, Kubeconfigs, TestKey};
use crate::validate::{self, Problem};
use crate::yaml_tab::YamlTab;

/// The view kind of the editor tab.
pub const KIND: &str = "kubeconfig";

pub fn view_kind() -> ViewKind {
    ViewKind::Custom(KIND.into())
}

/// The key context of the editor (its actions bind here).
pub const CONTEXT: &str = "KubeconfigEditor";

/// Paths of unsaved new documents: `kubyl-draft:<id>`.
pub const DRAFT_PREFIX: &str = "kubyl-draft:";

actions!(
    kubeconfig_editor,
    [
        /// Saves the kubeconfig (shows the preview first).
        Save,
        /// Tests the selected context.
        TestConnection,
        /// Tests every context of the file.
        TestAll,
        /// Shows the form.
        ShowForm,
        /// Shows the raw YAML.
        ShowYaml,
        /// Throws away the unsaved changes.
        Revert,
        /// Reveals or masks secret values in the YAML tab.
        ToggleSecrets,
    ]
);

/// How often the file is checked for changes by other tools.
const DISK_POLL: Duration = Duration::from_secs(2);
const VALIDATE_DELAY: Duration = Duration::from_millis(250);
const REVEAL_RETRY: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Form,
    Yaml,
}

pub struct KubeconfigEditor {
    /// The file; for a draft, where it's saved by default.
    pub(crate) path: PathBuf,
    /// Kubyl's own and backup folders.
    pub(crate) dirs: crate::state::Dirs,
    /// An unsaved new document (`kubyl-draft:<id>`).
    pub(crate) draft: Option<u64>,
    pub(crate) title: String,
    /// The file as loaded or last saved (`None`: a draft, or not loaded yet).
    pub(crate) snapshot: Option<Snapshot>,
    /// The document as loaded or saved.
    pub(crate) base: Doc,
    /// The document being edited.
    pub(crate) doc: Doc,
    pub(crate) load_error: Option<String>,
    pub(crate) loading: bool,
    pub(crate) saving: bool,
    pub(crate) selection: Option<EntryRef>,
    pub(crate) tab: Tab,
    pub(crate) problems: Vec<Problem>,
    /// The file changed on disk while there are unsaved changes.
    pub(crate) disk: Option<Snapshot>,
    /// Contexts renamed since the last save (old, new): their Kubyl overrides move along.
    pub(crate) renames: Vec<(String, String)>,
    pub(crate) filter: Entity<InputState>,
    pub(crate) form: Form,
    pub(crate) yaml: YamlTab,
    /// The right panel shows the selected context's test.
    pub(crate) show_test: bool,
    /// Changed lines against the file (the toolbar's "N unsaved changes").
    pub(crate) change_count: usize,
    pub(crate) focus: FocusHandle,
    validate_task: Option<Task<()>>,
    reveal_task: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl KubeconfigEditor {
    /// The editor for `path`, or for a draft (`kubyl-draft:<id>`).
    pub fn new(path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft_id = path
            .to_str()
            .and_then(|p| p.strip_prefix(DRAFT_PREFIX))
            .and_then(|id| id.parse::<u64>().ok());
        let draft: Option<Draft> = draft_id.and_then(|id| {
            Kubeconfigs::try_global(cx).and_then(|k| k.update(cx, |k, _| k.take_draft(id)))
        });
        let filter = cx
            .new(|cx| InputState::new(window, cx).placeholder("Filter contexts, clusters, users"));
        let filter_sub = cx.subscribe_in(&filter, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut subscriptions = vec![filter_sub];
        if let Some(global) = Kubeconfigs::try_global(cx) {
            subscriptions.push(cx.observe_in(&global, window, |this, _, window, cx| {
                this.take_pending(window, cx);
                cx.notify();
            }));
        }
        subscriptions.push(cx.observe_global_in::<Settings>(window, |_, _, cx| cx.notify()));
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.observe(&manager, |_, _, cx| cx.notify()));
        }
        let yaml = YamlTab::new(cx.weak_entity(), window, cx);
        let dirs = Kubeconfigs::dirs(cx);
        let mut this = Self {
            dirs,
            path: draft
                .as_ref()
                .map(|d| d.path.clone())
                .unwrap_or_else(|| path.clone()),
            draft: draft.as_ref().map(|_| draft_id.unwrap_or_default()),
            title: draft
                .as_ref()
                .map(|d| d.title.clone())
                .unwrap_or_else(|| file_title(&path)),
            snapshot: None,
            base: Doc::empty(),
            doc: draft
                .as_ref()
                .map(|d| d.doc.clone())
                .unwrap_or_else(Doc::empty),
            load_error: None,
            loading: draft.is_none(),
            saving: false,
            selection: None,
            tab: Tab::Form,
            problems: Vec::new(),
            disk: None,
            renames: Vec::new(),
            filter,
            form: Form::default(),
            yaml,
            show_test: false,
            change_count: 0,
            focus: cx.focus_handle(),
            validate_task: None,
            reveal_task: None,
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
        };
        if draft_id.is_some() && draft.is_none() {
            this.load_error = Some("This new kubeconfig was never saved.".into());
            this.loading = false;
        } else if draft.is_some() {
            this.select_first(window, cx);
            this.changed(window, cx);
        } else {
            this.load(window, cx);
            this.start_disk_poll(window, cx);
        }
        this
    }

    /// Where tests, sign-ins and pending selections of this document are keyed.
    pub fn doc_key(&self) -> String {
        match self.draft {
            Some(id) => format!("{DRAFT_PREFIX}{id}"),
            None => self.path.display().to_string(),
        }
    }

    pub fn is_draft(&self) -> bool {
        self.draft.is_some()
    }

    /// Kubyl created the file (or will): edited freely.
    pub fn is_owned(&self) -> bool {
        self.draft.is_some() || files::is_owned(&self.path, &self.dirs.owned)
    }

    /// Saving writes the file in place.
    pub fn is_editable(&self, cx: &App) -> bool {
        self.is_owned() || Settings::get::<KubeconfigSettings>(cx).opted_in(&self.path)
    }

    pub fn is_dirty(&self) -> bool {
        self.draft.is_some() || (self.snapshot.is_some() && self.doc != self.base)
    }

    /// The saved document, if Kubyl loads this file as a source (then phase 01 already runs
    /// its exec plugins; consent is only asked for changed ones).
    pub(crate) fn saved_and_loaded(&self, cx: &App) -> Option<&Doc> {
        let manager = ConnectionManager::try_global(cx)?;
        let loaded = manager
            .read(cx)
            .sources()
            .iter()
            .flat_map(|s| s.files.iter())
            .any(|f| f.path == self.path);
        (loaded && self.snapshot.is_some()).then_some(&self.base)
    }

    // ----- Loading -----

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self.loading = true;
        let read = cx
            .background_executor()
            .spawn(async move { files::read(&path) });
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) if snapshot.hash.is_none() => {
                        this.load_error =
                            Some(format!("{} doesn't exist.", display_path(&this.path)));
                    }
                    Ok(snapshot) => this.apply_snapshot(snapshot, window, cx),
                    Err(err) => {
                        this.load_error =
                            Some(format!("Couldn't read {}: {err}", display_path(&this.path)))
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Takes a file's contents as the new base (after loading, reloading or saving).
    pub(crate) fn apply_snapshot(
        &mut self,
        snapshot: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match Doc::parse(&snapshot.text) {
            Ok(doc) => {
                self.load_error = None;
                self.base = doc.clone();
                self.doc = doc;
                self.snapshot = Some(snapshot);
                self.disk = None;
                self.renames.clear();
                if self
                    .selection
                    .as_ref()
                    .is_none_or(|s| !self.doc.contains(s.kind, &s.name))
                {
                    self.select_first(window, cx);
                } else {
                    self.rebuild_form(window, cx);
                }
                self.take_pending(window, cx);
                self.changed(window, cx);
            }
            Err(err) => {
                self.load_error = Some(format!(
                    "{} isn't a valid kubeconfig: {err}",
                    display_path(&self.path)
                ));
                self.snapshot = Some(snapshot);
            }
        }
    }

    fn select_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first = Kind::ALL.iter().find_map(|k| {
            self.doc
                .names(*k)
                .into_iter()
                .next()
                .map(|n| EntryRef::new(*k, n))
        });
        self.select(first, window, cx);
    }

    /// Checks the file for changes by other tools. Without unsaved changes the editor just
    /// reloads; with them it shows the banner.
    fn start_disk_poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.path.clone();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(DISK_POLL).await;
                let path = path.clone();
                let read = cx
                    .background_executor()
                    .spawn(async move { files::read(&path).ok() })
                    .await;
                let Some(disk) = read else { continue };
                let alive = this.update_in(cx, |this, window, cx| {
                    if this.saving || this.draft.is_some() || this.loading {
                        return;
                    }
                    let known = this.snapshot.as_ref().and_then(|s| s.hash.clone());
                    let seen = this.disk.as_ref().and_then(|s| s.hash.clone());
                    if disk.hash.is_none() || disk.hash == known || disk.hash == seen {
                        return;
                    }
                    this.disk = Some(disk);
                    // Without local changes, just take the new file.
                    this.sync_disk(window, cx);
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// The file changed on disk and there are no local changes: take the new contents.
    pub(crate) fn sync_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(disk) = self.disk.clone()
            && !self.is_dirty()
        {
            self.apply_snapshot(disk, window, cx);
            NotificationCenter::push(
                cx,
                Notification::info(format!(
                    "{} changed on disk and was reloaded.",
                    display_path(&self.path)
                )),
            );
        }
    }

    /// "Reload": throws the local changes away and takes the file from disk.
    pub(crate) fn reload_from_disk(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(disk) = self.disk.take() {
            self.apply_snapshot(disk, window, cx);
        } else {
            self.load(window, cx);
        }
        cx.notify();
    }

    /// "Keep mine": the edits stay and will be written over the new file (the save preview
    /// shows the difference to what's on disk now).
    pub(crate) fn keep_mine(&mut self, cx: &mut Context<Self>) {
        if let Some(disk) = self.disk.take() {
            // Changes are now relative to the file on disk.
            if let Ok(doc) = Doc::parse(&disk.text) {
                self.base = doc;
            }
            self.snapshot = Some(disk);
        }
        cx.notify();
    }

    // ----- Editing -----

    /// Call after every change of `doc`: validation, the YAML tab, the tab's dirty dot.
    pub(crate) fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change_count = match &self.snapshot {
            Some(snapshot) if self.doc != self.base => {
                let written = crate::yaml::write(&snapshot.text, &self.doc.0);
                kubyl_yaml::diff::diff(&snapshot.text, &written.text, 0).change_count()
            }
            _ => 0,
        };
        let doc = self.doc.clone();
        let dir = self.path.parent().map(Path::to_path_buf);
        self.validate_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(VALIDATE_DELAY).await;
            let problems = cx
                .background_executor()
                .spawn(async move { validate::validate(&doc, dir.as_deref()) })
                .await;
            this.update(cx, |this, cx| {
                this.problems = problems;
                this.yaml.refresh_diagnostics(&this.doc, &this.problems, cx);
                cx.notify();
            })
            .ok();
        }));
        if self.tab == Tab::Yaml && !self.yaml.editing {
            self.show_yaml(window, cx);
        }
        cx.notify();
    }

    /// Edits the document and runs [`Self::changed`].
    pub(crate) fn edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Doc),
    ) {
        f(&mut self.doc);
        self.changed(window, cx);
    }

    pub(crate) fn select(
        &mut self,
        entry: Option<EntryRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selection = entry;
        self.rebuild_form(window, cx);
        cx.notify();
    }

    pub(crate) fn rebuild_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = Form::build(self, window, cx);
        self.form = form;
    }

    pub(crate) fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        match tab {
            Tab::Yaml => {
                self.show_yaml(window, cx);
                self.yaml.refresh_diagnostics(&self.doc, &self.problems, cx);
                if let Some(entry) = self.selection.clone() {
                    // The input scrolls only once it was laid out.
                    self.reveal_task = Some(cx.spawn_in(window, async move |this, cx| {
                        for _ in 0..50 {
                            let done = this
                                .update_in(cx, |this, window, cx| {
                                    this.yaml.reveal(&this.doc, &entry, window, cx)
                                })
                                .unwrap_or(true);
                            if done {
                                break;
                            }
                            cx.background_executor().timer(REVEAL_RETRY).await;
                        }
                    }));
                }
                self.yaml.focus(window, cx);
            }
            Tab::Form => {
                self.rebuild_form(window, cx);
                self.focus.focus(window, cx);
            }
        }
        cx.notify();
    }

    /// Adds a new entry of `kind` with a starting body and selects it.
    pub(crate) fn add(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.doc.unique_name(
            kind,
            match kind {
                Kind::Context => "new-context",
                Kind::Cluster => "new-cluster",
                Kind::User => "new-user",
            },
        );
        let mut body = serde_json::Map::new();
        match kind {
            Kind::Cluster => {
                body.insert(
                    "server".into(),
                    serde_json::Value::String("https://".into()),
                );
            }
            Kind::Context => {
                if let Some(c) = self.doc.names(Kind::Cluster).into_iter().next() {
                    body.insert("cluster".into(), serde_json::Value::String(c));
                }
                if let Some(u) = self.doc.names(Kind::User).into_iter().next() {
                    body.insert("user".into(), serde_json::Value::String(u));
                }
            }
            Kind::User => {}
        }
        if self.doc.add(kind, &name, body).is_ok() {
            self.select(Some(EntryRef::new(kind, name)), window, cx);
            self.changed(window, cx);
        }
    }

    pub(crate) fn duplicate(
        &mut self,
        entry: &EntryRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(name) = self.doc.duplicate(entry.kind, &entry.name) {
            self.select(Some(EntryRef::new(entry.kind, name)), window, cx);
            self.changed(window, cx);
        }
    }

    pub(crate) fn rename(
        &mut self,
        entry: &EntryRef,
        new: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new = new.trim().to_string();
        match self.doc.rename(entry.kind, &entry.name, &new) {
            Ok(()) => {
                if entry.kind == Kind::Context && entry.name != new {
                    // A → B → C moves A's overrides to C.
                    match self.renames.iter_mut().find(|(_, to)| *to == entry.name) {
                        Some(pair) => pair.1 = new.clone(),
                        None => self.renames.push((entry.name.clone(), new.clone())),
                    }
                }
                self.select(Some(EntryRef::new(entry.kind, new)), window, cx);
                self.changed(window, cx);
            }
            Err(err) => NotificationCenter::push(cx, Notification::error(err)),
        }
    }

    pub(crate) fn delete(&mut self, entry: &EntryRef, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.remove(entry.kind, &entry.name) {
            if self.selection.as_ref() == Some(entry) {
                self.select_first(window, cx);
            }
            self.changed(window, cx);
        }
    }

    pub(crate) fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft.is_some() {
            return;
        }
        self.doc = self.base.clone();
        self.renames.clear();
        if self
            .selection
            .as_ref()
            .is_none_or(|s| !self.doc.contains(s.kind, &s.name))
        {
            self.select_first(window, cx);
        } else {
            self.rebuild_form(window, cx);
        }
        self.changed(window, cx);
    }

    // ----- Kubyl's overrides (settings.json) -----

    /// The cluster id phase 01 gives a context of this file, when it's loaded.
    pub(crate) fn cluster_id(&self, context: &str, cx: &App) -> Option<ClusterId> {
        let manager = ConnectionManager::try_global(cx)?;
        let id = ContextInfo::make_id(context, &self.path);
        manager.read(cx).context(&id).map(|c| c.id.clone())
    }

    // ----- Testing -----

    /// Tests `context` from the edited document. Asks before running an exec plugin that
    /// isn't saved.
    pub(crate) fn test(&mut self, context: String, window: &mut Window, cx: &mut Context<Self>) {
        let key = TestKey::new(self.doc_key(), context.clone());
        let doc = self.doc.clone();
        let global = Kubeconfigs::global(cx);
        let needs = global
            .read(cx)
            .needs_consent(&doc, self.saved_and_loaded(cx), &context);
        let input = Input {
            doc,
            context: context.clone(),
            file: self.path.clone(),
            allow_exec: true,
        };
        self.show_test = true;
        match needs {
            None => global.update(cx, |g, cx| g.test(key, input, cx)),
            Some(spec) => {
                let contexts = vec![context];
                crate::dialogs::consent(
                    vec![(contexts, spec)],
                    move |_, cx| global.update(cx, |g, cx| g.test(key.clone(), input.clone(), cx)),
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Tests every context; asks once for every exec plugin that needs consent.
    pub(crate) fn test_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.doc.clone();
        let global = Kubeconfigs::global(cx);
        let saved = self.saved_and_loaded(cx).cloned();
        let contexts = doc.names(Kind::Context);
        let mut ask: Vec<(Vec<String>, crate::model::ExecSpec)> = Vec::new();
        for context in &contexts {
            if let Some(spec) = global.read(cx).needs_consent(&doc, saved.as_ref(), context) {
                match ask.iter_mut().find(|(_, s)| *s == spec) {
                    Some((names, _)) => names.push(context.clone()),
                    None => ask.push((vec![context.clone()], spec)),
                }
            }
        }
        let key = self.doc_key();
        let file = self.path.clone();
        let run = move |cx: &mut App| {
            global.update(cx, |g, cx| {
                for context in &contexts {
                    let input = Input {
                        doc: doc.clone(),
                        context: context.clone(),
                        file: file.clone(),
                        allow_exec: true,
                    };
                    g.test(TestKey::new(key.clone(), context.clone()), input, cx);
                }
            });
        };
        if ask.is_empty() {
            run(cx);
        } else {
            crate::dialogs::consent(ask, move |_, cx| run(cx), window, cx);
        }
    }

    /// Picks up "select this context" (and "test it") requests from the palette and the
    /// Clusters tab.
    fn take_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(global) = Kubeconfigs::try_global(cx) else {
            return;
        };
        let path = self.path.clone();
        let Some((context, test)) = global.update(cx, |g, _| g.take_pending(&path)) else {
            return;
        };
        if let Some(context) = context
            && self.doc.contains(Kind::Context, &context)
        {
            self.select(
                Some(EntryRef::new(Kind::Context, context.clone())),
                window,
                cx,
            );
            if test {
                self.test(context, window, cx);
            }
        }
    }

    // ----- Saving -----

    /// Writes the document to its file: what the save preview showed (`text`), with a backup,
    /// atomically, refusing when the file changed since it was loaded. `opt_in` turns editing
    /// on for a file Kubyl doesn't own first.
    pub fn save_file(
        &mut self,
        text: String,
        opt_in: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<files::Saved, files::SaveError>> {
        if opt_in {
            crate::settings::set_opt_in(&self.path, true, cx);
        }
        if !self.is_editable(cx) && !opt_in {
            return Task::ready(Err(files::SaveError::Io(
                "Kubyl doesn't edit this file until you allow it".into(),
            )));
        }
        let keep = Settings::get::<KubeconfigSettings>(cx).backups_kept;
        let options = files::SaveOptions {
            expected: if self.draft.is_some() {
                None
            } else {
                self.snapshot.as_ref().and_then(|s| s.hash.clone())
            },
            backups: Some(files::Backups {
                dir: self.dirs.backups.clone(),
                keep,
            }),
            private: self.draft.is_some() || self.doc.has_inline_credentials(),
        };
        let path = self.path.clone();
        let write_text = text.clone();
        let write = cx
            .background_executor()
            .spawn(async move { files::save(&path, &write_text, &options) });
        self.saving = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match &result {
                    Ok(saved) => this.saved(saved.clone(), text, window, cx),
                    Err(files::SaveError::Changed { .. }) => {
                        // Show the banner right away instead of at the next poll.
                        let path = this.path.clone();
                        if let Ok(disk) = files::read(&path) {
                            this.disk = Some(disk);
                        }
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
            result
        })
    }

    /// The text a save writes now (comments kept where possible).
    pub fn text_to_save(&self) -> crate::yaml::Written {
        match &self.snapshot {
            Some(snapshot) => crate::yaml::write(&snapshot.text, &self.doc.0),
            None => crate::yaml::Written {
                text: crate::yaml::render(&self.doc.0),
                in_place: false,
                lost_comments: Vec::new(),
            },
        }
    }

    /// Finishes a save: new base, overrides moved, phase 01 reloads.
    pub(crate) fn saved(
        &mut self,
        saved: files::Saved,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let old_key = self.doc_key();
        let was_draft = self.draft.take().is_some();
        if was_draft {
            Kubeconfigs::global(cx).update(cx, |g, cx| g.forget(&old_key, cx));
            self.title = file_title(&self.path);
            self.start_disk_poll(window, cx);
        }
        let renames = std::mem::take(&mut self.renames);
        self.apply_snapshot(
            Snapshot {
                text,
                hash: Some(saved.hash.clone()),
            },
            window,
            cx,
        );
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let path = self.path.clone();
            manager.update(cx, |m, cx| {
                for (old, new) in &renames {
                    m.move_context_settings(
                        &ContextInfo::make_id(old, &path),
                        &ContextInfo::make_id(new, &path),
                        cx,
                    );
                }
                // The watcher notices too; a new file in a new folder may not be watched yet.
                m.reload(cx);
            });
        }
        let mut message = format!("Saved {}", display_path(&self.path));
        if let Some(backup) = &saved.backup {
            message.push_str(&format!(" · backup {}", display_path(backup)));
        }
        NotificationCenter::push(cx, Notification::success(message));
        cx.notify();
    }

    /// After "Save as a Kubyl copy": the editor shows the copy, which replaced the original
    /// as a source.
    pub(crate) fn moved_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let old_key = self.doc_key();
        Kubeconfigs::global(cx).update(cx, |g, cx| g.forget(&old_key, cx));
        self.path = path;
        self.draft = None;
        self.title = file_title(&self.path);
        cx.notify();
    }
}

/// The tab title for a path: the file name (`config`, `eks-prod.yaml`).
pub fn file_title(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| display_path(path))
}

/// Where a file comes from, for the toolbar and "Save as a Kubyl copy".
pub(crate) fn source_kind(path: &Path, cx: &App) -> Option<SourceKind> {
    let manager = ConnectionManager::try_global(cx)?;
    manager
        .read(cx)
        .sources()
        .iter()
        .find(|s| s.files.iter().any(|f| f.path == path))
        .map(|s| s.spec.kind)
}

impl Focusable for KubeconfigEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for KubeconfigEditor {
    fn tab_title(&self, _: &App) -> SharedString {
        self.title.clone().into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(kubyl_ui::IconName::File.path())
    }

    fn is_dirty(&self, _: &App) -> bool {
        KubeconfigEditor::is_dirty(self)
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        // Drafts can't come back after a restart.
        self.draft
            .is_none()
            .then(|| ViewRequest::for_path(view_kind(), self.path.clone()))
    }
}
