//! The YAML editor tab (`ViewKind::Yaml`): state, loading, live updates, analysis and the
//! apply workflow. Rendering is in [`crate::ui`].

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, HighlightStyle, SharedString,
    Subscription, Task, WeakEntity, Window, actions,
};
use gpui_component::highlighter::{Diagnostic, DiagnosticSeverity};
use gpui_component::input::{
    EditorState, InputEvent, InputState, Position, RopeExt as _, TextDecoration,
    TextDecorationCollection,
};
use kubyl_core::{
    ActiveContext, ClusterId, Gvk, Gvr, Notification, NotificationCenter, ResourceRef, TabView,
    ViewKind, ViewRequest, spawn_kube,
};
use kubyl_explorer::dialogs::{self, ConfirmSpec};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::ops::{self, Revision};
use kubyl_resources::{ResourceSelection, ResourceStores, Selected, StoreHandle, StoreKey};
use kubyl_settings::Settings;
use serde_json::Value;

use crate::apply::{self, DocResult, Outcome};
use crate::diff::{self, LineDiff, Merge};
use crate::intel::{self, RefKind};
use crate::lsp::YamlLsp;
use crate::parse::{self, Path};
use crate::render::{self, RenderOptions, SecretValues};
use crate::schema::{self, Schema, Schemas};
use crate::settings::{ApplyHistory, HistoryEntry, YamlSettings};
use crate::templates;
use crate::validate::{self, Problem, Severity};

/// Key context of the editor tab. Its bindings use modifiers only: the buffer takes text.
pub const CONTEXT: &str = "YamlEditor";

actions!(
    yaml,
    [
        /// Server-side apply of the buffer.
        Apply,
        /// Apply with `dryRun=All`: the server's validation and admission webhooks answer.
        DryRun,
        /// Shows the diff against the live object.
        ShowDiff,
        /// Shows the Problems tab.
        ShowProblems,
        /// Shows the revision history.
        ShowHistory,
        /// Puts the live object back into the buffer.
        Revert,
        /// Shows or masks the decoded values of a Secret.
        ToggleSecrets,
        /// Shows or hides `metadata.managedFields`.
        ToggleManagedFields,
        /// Moves the cursor to the next problem.
        NextProblem,
        /// Unified or side-by-side diff.
        ToggleSideBySide,
        /// Opens the template and kind picker of a new resource.
        ShowTemplates,
        /// Shows or hides the bottom panel.
        TogglePanel,
    ]
);

const ANALYZE_DELAY: Duration = Duration::from_millis(120);
const REFETCH_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Diff,
    Problems,
    History,
    Results,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// Waiting for the cluster connection or discovery.
    Waiting,
    Loading,
    Ready,
    /// The object was deleted on the server.
    Gone,
    Failed(String),
    /// A new resource (nothing to load).
    New,
}

/// A related object (owner, secret, issuer…).
#[derive(Clone, Debug)]
pub struct Related {
    pub relation: &'static str,
    pub title: String,
    pub target: Option<ResourceRef>,
}

/// Where the code lenses go (byte offsets of the lines they annotate).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lenses {
    pub metadata: Option<usize>,
    pub status: Option<usize>,
}

struct Analysis {
    generation: u64,
    problems: Vec<Problem>,
    diff: LineDiff,
    lenses: Lenses,
    status_range: Option<Range<usize>>,
    first_gvk: Option<Gvk>,
    namespace: Option<String>,
}

pub struct YamlEditor {
    pub(crate) cluster: Option<ClusterId>,
    /// The namespace new objects go to (the header shows it).
    pub(crate) namespace: Option<String>,
    /// The object being edited; `None` for a new resource.
    pub(crate) object: Option<ResourceRef>,
    pub(crate) gvr: Option<Gvr>,
    pub(crate) gvk: Option<Gvk>,
    pub(crate) editor: Entity<EditorState>,
    focus: FocusHandle,
    weak: WeakEntity<Self>,

    // Live object.
    store: Option<StoreHandle>,
    pub(crate) live: Option<Arc<Value>>,
    live_rv: Option<String>,
    pub(crate) live_text: String,
    base_text: String,
    pub(crate) secret: Option<SecretValues>,
    pub(crate) options: RenderOptions,
    pub(crate) managers: Vec<String>,
    managed_fields: Option<Value>,
    pub(crate) state: LoadState,
    /// A newer version of the object arrived while the buffer has edits.
    pub(crate) pending: Option<(String, Option<SecretValues>, Arc<Value>)>,
    loaded: bool,
    fetch: Option<Task<()>>,
    refresh_after_apply: bool,

    // Analysis of the buffer.
    generation: u64,
    pub(crate) problems: Vec<Problem>,
    pub(crate) server_problems: Vec<Problem>,
    pub(crate) diff: LineDiff,
    pub(crate) lenses: Lenses,
    /// `namespace` of the first document, for the header.
    pub(crate) doc_namespace: Option<String>,
    analyze: Option<Task<()>>,
    status_dim: Option<TextDecorationCollection>,

    // Bottom panel.
    pub(crate) panel_tab: PanelTab,
    pub(crate) panel_open: bool,
    pub(crate) results: Vec<DocResult>,
    pub(crate) results_dry_run: bool,
    pub(crate) busy: Option<&'static str>,
    pub(crate) revisions: Option<Result<Vec<Revision>, String>>,
    pub(crate) local_history: Vec<HistoryEntry>,
    history_task: Option<Task<()>>,
    apply_task: Option<Task<()>>,

    // Sidebar.
    pub(crate) related: Vec<Related>,
    ref_stores: Vec<StoreHandle>,
    pub(crate) expanded: HashSet<String>,
    pub(crate) cursor_path: Option<String>,

    // New resource.
    /// A note shown above a draft's buffer (where its text came from), see [`crate::open_draft`].
    pub(crate) draft_note: Option<SharedString>,
    pub(crate) picker_open: bool,
    pub(crate) picker_query: Entity<InputState>,
    generated: Option<(Gvk, String)>,

    _subscriptions: Vec<Subscription>,
}

impl YamlEditor {
    /// `target`: an object to edit, or a list (kind + namespace) / nothing for a new resource.
    pub fn new(target: Option<ResourceRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("yaml")
                .line_number(true)
                .folding(true)
                .searchable(true)
                .soft_wrap(false);
            let lsp = Rc::new(YamlLsp { view: weak.clone() });
            state.lsp_mut().hover_provider = Some(lsp.clone());
            state.lsp_mut().completion_provider = Some(lsp);
            state.lsp_mut().completion_menu.max_width = gpui::px(420.);
            state
        });
        let picker_query = cx.new(|cx| InputState::new(window, cx).placeholder("Any kind…"));
        let active = ActiveContext::global(cx).clone();
        let target_key = target.clone().filter(|t| !t.is_object());
        let (cluster, namespace, object, gvr) = match target {
            Some(t) if t.is_object() => (
                Some(t.cluster.clone()),
                t.namespace.clone(),
                Some(t.clone()),
                Some(t.gvr.clone()),
            ),
            Some(t) => (
                Some(t.cluster.clone()),
                t.namespace
                    .clone()
                    .or_else(|| active.namespace.as_ref().map(|n| n.to_string())),
                None,
                Some(t.gvr.clone()).filter(|g| !g.resource.is_empty()),
            ),
            None => (
                active.cluster.as_ref().map(|c| c.id.clone()),
                active.namespace.as_ref().map(|n| n.to_string()),
                None,
                None,
            ),
        };

        let mut subscriptions = vec![
            cx.subscribe_in(
                &editor,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.on_change(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &picker_query,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => {
                        if let Some(gvr) =
                            this.picker_matches(cx).first().map(|(g, _, _)| g.clone())
                        {
                            this.start_kind(&gvr, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
        ];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe_in(
                &manager,
                window,
                |this, _, event: &ConnectionEvent, window, cx| match event {
                    ConnectionEvent::DiscoveryChanged(id) | ConnectionEvent::StateChanged(id)
                        if Some(id) == this.cluster.as_ref() =>
                    {
                        if this.object.is_some() && !this.loaded {
                            this.load(window, cx);
                        }
                        this.schedule_analysis(cx);
                    }
                    _ => {}
                },
            ));
        }
        if let Some(schemas) = Schemas::global(cx) {
            subscriptions.push(cx.observe_in(&schemas, window, |this, _, window, cx| {
                this.regenerate_skeleton(window, cx);
                this.schedule_analysis(cx);
                cx.notify();
            }));
        }
        let focus = cx.focus_handle();
        subscriptions
            .push(cx.on_focus_in(&focus, window, |this, _, cx| this.publish_selection(cx)));

        let settings = Settings::get::<YamlSettings>(cx).clone();
        let mut this = Self {
            cluster,
            namespace,
            state: if object.is_some() {
                LoadState::Loading
            } else {
                LoadState::New
            },
            object,
            gvr,
            gvk: None,
            editor,
            focus,
            weak,
            store: None,
            live: None,
            live_rv: None,
            live_text: String::new(),
            base_text: String::new(),
            secret: None,
            options: RenderOptions {
                hide_managed_fields: settings.hide_managed_fields,
                reveal_secrets: false,
            },
            managers: Vec::new(),
            managed_fields: None,
            pending: None,
            loaded: false,
            fetch: None,
            refresh_after_apply: false,
            generation: 0,
            problems: Vec::new(),
            server_problems: Vec::new(),
            diff: LineDiff::default(),
            lenses: Lenses::default(),
            doc_namespace: None,
            analyze: None,
            status_dim: None,
            panel_tab: PanelTab::Diff,
            panel_open: true,
            results: Vec::new(),
            results_dry_run: false,
            busy: None,
            revisions: None,
            local_history: Vec::new(),
            history_task: None,
            apply_task: None,
            related: Vec::new(),
            ref_stores: Vec::new(),
            expanded: ["spec".to_string()].into_iter().collect(),
            cursor_path: None,
            draft_note: None,
            picker_open: false,
            picker_query,
            generated: None,
            _subscriptions: subscriptions,
        };
        if this.object.is_some() {
            this.load(window, cx);
        } else if let Some(draft) = target_draft(&target_key, cx) {
            this.gvk = this.kind_of_gvr(cx);
            this.draft_note = draft.note;
            this.panel_tab = PanelTab::Problems;
            this.set_buffer(&draft.text, false, window, cx);
        } else {
            this.gvk = this.kind_of_gvr(cx);
            match this.gvr.clone() {
                Some(gvr) => this.start_kind(&gvr, window, cx),
                None => {
                    this.picker_open = true;
                    this.panel_tab = PanelTab::Problems;
                }
            }
        }
        this
    }

    // ----- Identity -----

    pub(crate) fn kind(&self) -> String {
        self.gvk
            .as_ref()
            .map(|g| g.kind.clone())
            .unwrap_or_else(|| "Resource".into())
    }

    fn kind_of_gvr(&self, cx: &App) -> Option<Gvk> {
        let gvr = self.gvr.as_ref()?;
        let discovery = self.discovery(cx)?;
        kubyl_resources::store::find_resource(&discovery.resources, gvr).map(|r| r.gvk.clone())
    }

    pub(crate) fn discovery(&self, cx: &App) -> Option<Arc<kubyl_kube::discovery::Discovery>> {
        ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(self.cluster.as_ref()?)
    }

    pub(crate) fn cluster_name(&self, cx: &App) -> SharedString {
        match (&self.cluster, ConnectionManager::try_global(cx)) {
            (Some(id), Some(m)) => m.read(cx).display_name(id),
            (Some(id), None) => id.to_string().into(),
            _ => "No cluster".into(),
        }
    }

    pub(crate) fn caps(&self, cx: &App) -> kubyl_core::ClusterCaps {
        match (&self.cluster, ConnectionManager::try_global(cx)) {
            (Some(id), Some(m)) => m.read(cx).caps(id),
            _ => Default::default(),
        }
    }

    pub(crate) fn text(&self, cx: &App) -> String {
        self.editor.read(cx).text().to_string()
    }

    pub(crate) fn is_new(&self) -> bool {
        self.object.is_none()
    }

    /// The namespace the buffer's first document goes to.
    pub(crate) fn target_namespace(&self) -> Option<String> {
        self.doc_namespace
            .clone()
            .or_else(|| self.namespace.clone())
    }

    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.object.clone() else {
            return;
        };
        let caps = self.caps(cx);
        ResourceSelection::set(
            cx,
            ResourceSelection {
                items: vec![Selected {
                    target,
                    kind: self.kind(),
                    object: self.live.clone(),
                    store: self.store.as_ref().map(|s| s.entity().clone()),
                }],
                caps,
            },
        );
    }

    // ----- Loading -----

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.object.clone() else {
            return;
        };
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let (client, discovery) = {
            let m = manager.read(cx);
            (m.client(&target.cluster), m.discovery(&target.cluster))
        };
        let (Some(client), Some(discovery)) = (client, discovery) else {
            manager.update(cx, |m, cx| m.ensure_connected(&target.cluster, cx));
            self.state = LoadState::Waiting;
            cx.notify();
            return;
        };
        let Some(info) = kubyl_resources::store::find_resource(&discovery.resources, &target.gvr)
        else {
            self.state = LoadState::Failed(format!("The cluster doesn't serve {}.", target.gvr));
            cx.notify();
            return;
        };
        self.gvk = Some(info.gvk.clone());
        if self.store.is_none() {
            let key = StoreKey::new(
                target.cluster.clone(),
                target.gvr.clone(),
                target.namespace.clone(),
            )
            .fields(format!(
                "metadata.name={}",
                target.name.clone().unwrap_or_default()
            ));
            let handle = ResourceStores::acquire(cx, key);
            self._subscriptions.push(cx.observe_in(
                handle.entity(),
                window,
                |this, store, window, cx| this.on_store_change(store, window, cx),
            ));
            self.store = Some(handle);
        }
        let resource = kubyl_resources::store::api_resource(info);
        let (namespace, name) = (
            target.namespace.clone(),
            target.name.clone().unwrap_or_default(),
        );
        // A GET of our own: the watch caches drop managedFields.
        let task = spawn_kube(cx, async move {
            let api: kube::Api<kube::api::DynamicObject> = match &namespace {
                Some(ns) => kube::Api::namespaced_with(client, ns, &resource),
                None => kube::Api::all_with(client, &resource),
            };
            match api.get(&name).await {
                Ok(object) => serde_json::to_value(object).map_err(|e| e.to_string()),
                Err(kube::Error::Api(status)) => Err(status.message.clone()),
                Err(err) => Err(err.to_string()),
            }
        });
        self.fetch = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| this.on_fetched(result, window, cx))
                .ok();
        }));
    }

    fn on_store_change(
        &mut self,
        store: Entity<kubyl_resources::ResourceStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = &self.object else {
            return;
        };
        let key = kubyl_resources::object_key(
            target.namespace.as_deref(),
            target.name.as_deref().unwrap_or_default(),
        );
        let store = store.read(cx);
        match store.get(&key) {
            Some(object) => {
                let rv = object
                    .pointer("/metadata/resourceVersion")
                    .and_then(Value::as_str)
                    .map(String::from);
                // Refetch on changes, and when the object appears after a failed first load.
                if rv != self.live_rv && (self.loaded || self.state == LoadState::Gone) {
                    // Refetch (the store drops managedFields), debounced.
                    self.fetch = Some(cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(REFETCH_DELAY).await;
                        this.update_in(cx, |this, window, cx| this.load(window, cx))
                            .ok();
                    }));
                }
                if self.state == LoadState::Gone {
                    self.state = LoadState::Ready;
                    cx.notify();
                }
            }
            None if store.status().is_ready() && self.loaded => {
                self.state = LoadState::Gone;
                cx.notify();
            }
            None => {}
        }
    }

    fn on_fetched(
        &mut self,
        result: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let object = match result {
            Ok(object) => object,
            Err(err) => {
                self.state = if err.contains("not found") {
                    LoadState::Gone
                } else if self.loaded {
                    // Keep the buffer; the next change retries.
                    self.state.clone()
                } else {
                    LoadState::Failed(err)
                };
                cx.notify();
                return;
            }
        };
        self.live_rv = object
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .map(String::from);
        self.managers = render::managers(&object);
        self.managed_fields = object.pointer("/metadata/managedFields").cloned();
        let (text, secret) = render::render(&object, self.options);
        let object = Arc::new(object);
        if !self.loaded {
            self.loaded = true;
            self.state = LoadState::Ready;
            self.live = Some(object);
            self.live_text = text.clone();
            self.base_text = text.clone();
            self.secret = secret;
            self.set_buffer(&text, false, window, cx);
            self.load_related(cx);
            self.load_history(cx);
            self.publish_selection(cx);
        } else if self.refresh_after_apply {
            // Our own apply: keep the user's text (comments, order), rebase on the result.
            self.refresh_after_apply = false;
            self.live = Some(object);
            self.live_text = text.clone();
            self.base_text = text;
            self.secret = secret;
            self.load_related(cx);
            self.load_history(cx);
        } else if !self.is_dirty_now(cx) {
            self.live = Some(object);
            self.live_text = text.clone();
            self.base_text = text.clone();
            self.secret = secret;
            self.pending = None;
            if self.text(cx) != text {
                self.set_buffer(&text, true, window, cx);
            }
        } else if diff::diff(&self.live_text, &text, 0).is_empty() {
            // Only server-managed fields changed (status, resourceVersion).
            self.live = Some(object);
            self.live_text = text;
        } else {
            self.pending = Some((text, secret, object));
        }
        self.state = LoadState::Ready;
        self.schedule_analysis(cx);
        cx.notify();
    }

    /// Replaces the buffer. `keep_view`: keep the scroll position and cursor (silent refresh).
    fn set_buffer(
        &mut self,
        text: &str,
        keep_view: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_string();
        self.editor.update(cx, |state, cx| {
            let scroll = state.scroll_offset();
            let cursor = state.cursor();
            state.set_value(text.clone(), window, cx);
            if keep_view {
                let at = cursor.min(text.len());
                state.set_selected_range(at..at, cx);
                state.set_scroll_offset(scroll, cx);
            }
        });
        self.on_change(window, cx);
    }

    /// Replaces the buffer and keeps the change undoable.
    fn replace_buffer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_string();
        self.editor
            .update(cx, |state, cx| state.replace_all(text, window, cx));
        self.on_change(window, cx);
    }

    fn is_dirty_now(&self, cx: &App) -> bool {
        if self.is_new() {
            return false;
        }
        !diff::diff(&self.live_text, &self.text(cx), 0).is_empty()
    }

    // ----- Live changes while editing -----

    pub(crate) fn reload_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, secret, object)) = self.pending.take() {
            self.live = Some(object);
            self.live_text = text.clone();
            self.base_text = text.clone();
            self.secret = secret;
            self.replace_buffer(&text, window, cx);
        }
    }

    pub(crate) fn merge_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((theirs, secret, object)) = self.pending.clone() else {
            return;
        };
        match diff::merge(&self.base_text, &self.text(cx), &theirs) {
            Merge::Clean(merged) => {
                self.pending = None;
                self.live = Some(object);
                self.live_text = theirs.clone();
                self.base_text = theirs;
                self.secret = secret;
                self.replace_buffer(&merged, window, cx);
                toast(
                    cx,
                    Notification::success("Merged the server's changes into your edits."),
                );
            }
            Merge::Conflict => toast(
                cx,
                Notification::warning(
                    "Your edits and the server's changes touch the same lines. Reload or keep yours.",
                ),
            ),
        }
    }

    pub(crate) fn keep_mine(&mut self, cx: &mut Context<Self>) {
        if let Some((text, _, object)) = self.pending.take() {
            self.live = Some(object);
            self.live_text = text.clone();
            self.base_text = text;
            self.schedule_analysis(cx);
            cx.notify();
        }
    }

    // ----- Analysis -----

    fn on_change(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        if !self.server_problems.is_empty() {
            self.server_problems.clear();
        }
        self.schedule_analysis(cx);
        cx.notify();
    }

    pub(crate) fn schedule_analysis(&mut self, cx: &mut Context<Self>) {
        self.analyze = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ANALYZE_DELAY).await;
            let Ok(task) = this.update(cx, |this, cx| this.analysis_task(cx)) else {
                return;
            };
            let analysis = task.await;
            this.update(cx, |this, cx| this.apply_analysis(analysis, cx))
                .ok();
        }));
    }

    fn analysis_task(&mut self, cx: &mut Context<Self>) -> Task<Analysis> {
        let text = self.text(cx);
        let validate = Settings::get::<YamlSettings>(cx).validate;
        let mut schemas: Vec<(Gvk, Arc<Value>)> = Vec::new();
        if validate {
            for gvk in document_gvks(&text) {
                if !schemas.iter().any(|(g, _)| g == &gvk)
                    && let Some((doc, _)) = self.schema_lookup(&gvk, cx)
                {
                    schemas.push((gvk, doc));
                }
            }
        }
        let live = (!self.is_new() && self.loaded).then(|| self.live_text.clone());
        let generation = self.generation;
        cx.background_spawn(async move { analyze(generation, &text, live.as_deref(), &schemas) })
    }

    fn apply_analysis(&mut self, analysis: Analysis, cx: &mut Context<Self>) {
        if analysis.generation != self.generation {
            return;
        }
        self.problems = analysis.problems;
        self.diff = analysis.diff;
        self.lenses = analysis.lenses;
        self.doc_namespace = analysis.namespace;
        if self.is_new() && analysis.first_gvk.is_some() {
            self.gvk = analysis.first_gvk;
        }
        self.push_diagnostics(cx);
        let decorations: Vec<TextDecoration> = analysis
            .status_range
            .into_iter()
            .map(|range| {
                TextDecoration::new(
                    range,
                    HighlightStyle {
                        fade_out: Some(0.45),
                        ..Default::default()
                    },
                )
            })
            .collect();
        match &self.status_dim {
            Some(collection) => collection.set(decorations, cx),
            None => {
                let collection = self.editor.update(cx, |state, cx| {
                    state.create_decorations_collection(decorations, cx)
                });
                self.status_dim = Some(collection);
            }
        }
        self.update_cursor_path(cx);
        cx.notify();
    }

    /// Every problem: syntax, schema and the last dry run/apply.
    pub(crate) fn all_problems(&self) -> Vec<&Problem> {
        let mut all: Vec<&Problem> = self
            .problems
            .iter()
            .chain(self.server_problems.iter())
            .collect();
        all.sort_by_key(|p| (p.range.start, p.severity));
        all.dedup_by(|a, b| a.range == b.range && a.message == b.message);
        all
    }

    fn push_diagnostics(&mut self, cx: &mut Context<Self>) {
        let problems: Vec<Problem> = self.all_problems().into_iter().cloned().collect();
        self.editor.update(cx, |state, cx| {
            let rope = state.text().clone();
            let len = rope.len();
            if let Some(set) = state.diagnostics_mut() {
                set.reset(&rope);
                for p in problems {
                    let start = p.range.start.min(len);
                    let mut end = p.range.end.min(len);
                    if end <= start {
                        // Give point diagnostics a character to underline.
                        end = rope
                            .to_string()
                            .get(start..)
                            .and_then(|s| s.chars().next())
                            .map_or(start, |c| start + c.len_utf8());
                    }
                    let severity = match p.severity {
                        Severity::Error => DiagnosticSeverity::Error,
                        Severity::Warning => DiagnosticSeverity::Warning,
                        Severity::Info => DiagnosticSeverity::Info,
                    };
                    set.push(
                        Diagnostic::new(
                            rope.offset_to_position(start)..rope.offset_to_position(end),
                            p.message,
                        )
                        .with_severity(severity)
                        .with_source("kubyl"),
                    );
                }
            }
            cx.notify();
        });
    }

    fn update_cursor_path(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let cursor = self.editor.read(cx).cursor();
        let parsed = parse::parse(&text);
        self.cursor_path = parsed
            .doc_at(cursor)
            .and_then(|d| d.root.as_ref())
            .map(|root| root.path_at(cursor).0)
            .filter(|p| !p.0.is_empty())
            .map(|p| {
                let mut p = p;
                while matches!(p.0.last(), Some(parse::Seg::Index(_))) {
                    p.0.pop();
                }
                p.to_string()
            });
    }

    // ----- Schemas -----

    /// The OpenAPI document of `gvk` and its source label; starts loading it if needed.
    pub(crate) fn schema_lookup(
        &mut self,
        gvk: &Gvk,
        cx: &mut Context<Self>,
    ) -> Option<(Arc<Value>, String)> {
        let cluster = self.cluster.clone()?;
        let schemas = Schemas::global(cx)?;
        let doc = schemas
            .update(cx, |s, cx| s.get(&cluster, &gvk.group, &gvk.version, cx))
            .ok()
            .flatten()?;
        let plural = self
            .discovery(cx)
            .and_then(|d| d.by_gvk(gvk).map(|r| r.gvr.resource.clone()));
        Some((doc, schema::source_label(gvk, plural.as_deref())))
    }

    /// `(apiVersion, kind)` of every kind the cluster serves that can be created.
    pub(crate) fn served_kinds(&self, cx: &App) -> Vec<(String, String)> {
        let Some(discovery) = self.discovery(cx) else {
            return Vec::new();
        };
        let mut kinds: Vec<(String, String)> = discovery
            .preferred()
            .filter(|r| r.supports("create") && !r.gvr.resource.contains('/'))
            .map(|r| (r.gvk.api_version(), r.gvk.kind.clone()))
            .collect();
        kinds.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        kinds.dedup();
        kinds
    }

    fn reference_keys(&self, refs: &RefKind) -> Vec<StoreKey> {
        let Some(cluster) = &self.cluster else {
            return Vec::new();
        };
        let namespace = self.target_namespace();
        refs.gvrs
            .iter()
            .map(|gvr| {
                // Cluster-scoped kinds (ClusterIssuers) ignore the namespace.
                let ns = if gvr.resource.starts_with("cluster") || !refs.namespaced {
                    None
                } else {
                    namespace.clone()
                };
                StoreKey::new(cluster.clone(), gvr.clone(), ns).metadata()
            })
            .collect()
    }

    /// Names of the objects `refs` points at, from loaded caches; `true` if a cache is missing.
    pub(crate) fn reference_names(&self, refs: &RefKind, cx: &App) -> (Vec<String>, bool) {
        let mut names = Vec::new();
        let mut missing = false;
        for key in self.reference_keys(refs) {
            let mut full = key.clone();
            full.mode = kubyl_resources::StoreMode::Full;
            match ResourceStores::peek(cx, &key).or_else(|| ResourceStores::peek(cx, &full)) {
                Some(store) => {
                    names.extend(
                        store.read(cx).objects().values().filter_map(|o| {
                            o.pointer("/metadata/name")?.as_str().map(String::from)
                        }),
                    )
                }
                None => missing = true,
            }
        }
        names.sort();
        names.dedup();
        (names, missing)
    }

    /// Watches the kinds `refs` points at so the next completion lists their names.
    pub(crate) fn watch_references(&mut self, refs: &RefKind, cx: &mut Context<Self>) {
        for key in self.reference_keys(refs) {
            if self.ref_stores.iter().any(|h| h.read(cx).key() == &key) {
                continue;
            }
            self.ref_stores.push(ResourceStores::acquire(cx, key));
        }
    }

    // ----- Related objects and history -----

    fn load_related(&mut self, cx: &mut Context<Self>) {
        let (Some(object), Some(cluster)) = (self.live.clone(), self.cluster.clone()) else {
            return;
        };
        let kind = self.kind();
        let discovery = self.discovery(cx);
        let namespace = self.object.as_ref().and_then(|o| o.namespace.clone());
        let mut refs = kubyl_palette::references::references(&kind, &object);
        refs.extend(extra_references(&object, namespace.as_deref()));
        refs.dedup();
        self.related = refs
            .into_iter()
            .map(|reference| {
                let title = reference.title();
                let target = match &reference.target {
                    kubyl_palette::references::RefTarget::Object {
                        gvk,
                        namespace: ns,
                        name,
                    } => discovery.as_ref().and_then(|d| {
                        let info = d.by_gvk(gvk).or_else(|| {
                            d.preferred()
                                .find(|r| r.gvk.kind == gvk.kind && r.gvk.group == gvk.group)
                        })?;
                        Some(ResourceRef::object(
                            cluster.clone(),
                            info.gvr.clone(),
                            if info.namespaced {
                                ns.clone().or_else(|| namespace.clone())
                            } else {
                                None
                            },
                            name.clone(),
                        ))
                    }),
                    kubyl_palette::references::RefTarget::Filtered { .. } => None,
                };
                Related {
                    relation: reference.relation,
                    title,
                    target,
                }
            })
            .collect();
    }

    pub(crate) fn has_workload_history(&self) -> bool {
        self.object
            .as_ref()
            .is_some_and(|t| ops::WITH_HISTORY.contains(&t.gvr.resource.as_str()))
    }

    pub(crate) fn load_history(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.object.as_ref() else {
            return;
        };
        // The entry's own applies, and those recorded under its contexts' ids before they were
        // grouped (`settings_keys`: the entry id first), combined.
        let clusters = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).settings_keys(&target.cluster))
            .filter(|keys| !keys.is_empty())
            .unwrap_or_else(|| vec![target.cluster.to_string()]);
        let keys: Vec<String> = clusters
            .iter()
            .map(|cluster| {
                ApplyHistory::key(
                    cluster,
                    &target.gvr.group,
                    &target.gvr.resource,
                    target.namespace.as_deref(),
                    target.name.as_deref().unwrap_or_default(),
                )
            })
            .collect();
        self.local_history = ApplyHistory::entries_of(cx, &keys);
        if !self.has_workload_history() {
            return;
        }
        let (Some(target), Some(manager)) =
            (self.object.clone(), ConnectionManager::try_global(cx))
        else {
            return;
        };
        let Some(client) = manager.read(cx).client(&target.cluster) else {
            return;
        };
        let task = spawn_kube(cx, async move {
            ops::workload_history(
                client,
                &target.gvr.resource,
                target.namespace.unwrap_or_default(),
                target.name.unwrap_or_default(),
            )
            .await
        });
        self.history_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.revisions = Some(result);
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn roll_back(&mut self, revision: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.object.clone() else {
            return;
        };
        let caps = self.caps(cx);
        if caps.read_only {
            let message = format!("{} is read-only.", self.cluster_name(cx));
            toast(cx, Notification::error(message));
            return;
        }
        let name = target.name.clone().unwrap_or_default();
        let mut spec = ConfirmSpec::new(
            format!(
                "Roll back {}/{name} to revision {revision}?",
                self.kind().to_lowercase()
            ),
            "Roll back",
        );
        spec.lines = vec![
            format!(
                "{} · {}",
                self.cluster_name(cx),
                target.namespace.clone().unwrap_or_default()
            )
            .into(),
        ];
        spec.typed = caps.production.then(|| name.clone());
        let weak = self.weak.clone();
        dialogs::confirm(
            spec,
            move |_, _, cx| {
                let Some(client) = ConnectionManager::global(cx)
                    .read(cx)
                    .client(&target.cluster)
                else {
                    return;
                };
                let target = target.clone();
                let task = spawn_kube(cx, async move {
                    ops::workload_undo(
                        client,
                        &target.gvr.resource,
                        target.namespace.unwrap_or_default(),
                        target.name.unwrap_or_default(),
                        revision,
                    )
                    .await
                });
                let weak = weak.clone();
                cx.spawn(async move |cx| {
                    let result = task.await;
                    cx.update(|cx| {
                        match result {
                            Ok(()) => toast(
                                cx,
                                Notification::success(format!(
                                    "Rolled back to revision {revision}"
                                )),
                            ),
                            Err(err) => toast(cx, Notification::error(err)),
                        }
                        weak.update(cx, |this, cx| this.load_history(cx)).ok();
                    });
                })
                .detach();
            },
            window,
            cx,
        );
    }

    pub(crate) fn restore_history(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(entry) = self.local_history.get(index).cloned() {
            self.replace_buffer(&entry.yaml, window, cx);
        }
    }

    // ----- Editing helpers -----

    pub(crate) fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_new() {
            return;
        }
        let text = match self.pending.take() {
            Some((text, secret, object)) => {
                self.live = Some(object);
                self.live_text = text.clone();
                self.base_text = text.clone();
                self.secret = secret;
                text
            }
            None => self.live_text.clone(),
        };
        self.replace_buffer(&text, window, cx);
    }

    pub(crate) fn toggle_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(values) = self.secret.clone() else {
            return;
        };
        let reveal = !self.options.reveal_secrets;
        self.options.reveal_secrets = reveal;
        let buffer = self.text(cx);
        let edits = render::toggle_secret_edits(&buffer, &values, reveal);
        self.base_text = render::apply_edits(
            &self.base_text,
            render::toggle_secret_edits(&self.base_text, &values, reveal),
        );
        if let Some(live) = &self.live {
            self.live_text = render::render(live, self.options).0;
        }
        if !edits.is_empty() {
            self.replace_buffer(&render::apply_edits(&buffer, edits), window, cx);
        }
        cx.notify();
    }

    pub(crate) fn toggle_managed_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_new() {
            return;
        }
        let show = self.options.hide_managed_fields;
        self.options.hide_managed_fields = !show;
        let buffer = self.text(cx);
        let edits =
            render::toggle_managed_fields_edits(&buffer, self.managed_fields.as_ref(), show);
        self.base_text = render::apply_edits(
            &self.base_text,
            render::toggle_managed_fields_edits(
                &self.base_text,
                self.managed_fields.as_ref(),
                show,
            ),
        );
        if let Some(live) = &self.live {
            self.live_text = render::render(live, self.options).0;
        }
        if !edits.is_empty() {
            self.replace_buffer(&render::apply_edits(&buffer, edits), window, cx);
        }
        cx.notify();
    }

    /// Moves the cursor to `offset` and focuses the editor.
    pub(crate) fn jump_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            let position: Position = state.text().offset_to_position(offset);
            state.set_cursor_position(position, window, cx);
            state.focus(window, cx);
        });
        self.update_cursor_path(cx);
        cx.notify();
    }

    pub(crate) fn next_problem(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.editor.read(cx).cursor();
        let starts: Vec<usize> = self.all_problems().iter().map(|p| p.range.start).collect();
        let next = starts
            .iter()
            .copied()
            .find(|s| *s > cursor)
            .or(starts.first().copied());
        if let Some(offset) = next {
            self.jump_to(offset, window, cx);
        }
    }

    /// Outline click: jump to the field, or insert it under its parent.
    pub(crate) fn go_to_field(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let parsed = parse::parse(&text);
        let Some(root) = parsed.roots().next() else {
            return;
        };
        if let Some(entry) = root.find_entry(path) {
            self.jump_to(entry.key.span.start, window, cx);
            return;
        }
        // Insert under the deepest existing parent that is a mapping.
        let Some((parse::Seg::Key(name), parent_segs)) = path.0.split_last() else {
            return;
        };
        let parent_path = Path(parent_segs.to_vec());
        let Some(parent) = root.find(&parent_path) else {
            // Jump to the closest existing ancestor instead.
            for len in (1..path.0.len()).rev() {
                if let Some(entry) = root.find_entry(&Path(path.0[..len].to_vec())) {
                    self.jump_to(entry.key.span.start, window, cx);
                    return;
                }
            }
            return;
        };
        let Some(last) = parent.as_map().and_then(|e| e.last()) else {
            return;
        };
        let first = &parent.as_map().expect("map")[0];
        let column = first.key.span.start - parse::line_range(&text, first.key.span.start).start;
        let line_end = text[last.value.span.end..]
            .find('\n')
            .map_or(text.len(), |i| last.value.span.end + i);
        // Block scalars end at the next key; step back to the end of their last line.
        let at = if text[..line_end].trim_end().len() < last.value.span.end {
            text[..last.value.span.end].trim_end().len()
        } else {
            line_end
        };
        let insert = format!("\n{}{name}: ", " ".repeat(column));
        let caret = at + insert.len();
        let mut new_text = text.clone();
        new_text.insert_str(at, &insert);
        self.replace_buffer(&new_text, window, cx);
        self.jump_to(caret, window, cx);
    }

    // ----- New resource -----

    pub(crate) fn picker_matches(&self, cx: &App) -> Vec<(Gvr, Gvk, bool)> {
        let query = self.picker_query.read(cx).value().to_lowercase();
        let Some(discovery) = self.discovery(cx) else {
            return Vec::new();
        };
        let mut out: Vec<(Gvr, Gvk, bool)> = discovery
            .preferred()
            .filter(|r| r.supports("create") && !r.gvr.resource.contains('/'))
            .filter(|r| {
                query.is_empty()
                    || r.gvk.kind.to_lowercase().contains(&query)
                    || r.gvr.resource.contains(&query)
                    || r.short_names.iter().any(|s| s == &query)
                    || r.gvk.group.contains(&query)
            })
            .map(|r| (r.gvr.clone(), r.gvk.clone(), r.namespaced))
            .collect();
        out.sort_by(|a, b| {
            let exact = |g: &Gvk| !g.kind.eq_ignore_ascii_case(&query);
            (exact(&a.1), &a.1.kind).cmp(&(exact(&b.1), &b.1.kind))
        });
        out.dedup_by(|a, b| a.1 == b.1);
        out
    }

    /// Fills the buffer with a template or schema skeleton for `gvr`.
    pub(crate) fn start_kind(&mut self, gvr: &Gvr, window: &mut Window, cx: &mut Context<Self>) {
        let Some(discovery) = self.discovery(cx) else {
            self.picker_open = true;
            cx.notify();
            return;
        };
        let Some(info) = kubyl_resources::store::find_resource(&discovery.resources, gvr)
            .or_else(|| discovery.preferred().find(|r| &r.gvr == gvr))
        else {
            return;
        };
        let gvk = info.gvk.clone();
        let namespace = info
            .namespaced
            .then(|| self.namespace.clone().unwrap_or_else(|| "default".into()));
        self.gvr = Some(info.gvr.clone());
        self.gvk = Some(gvk.clone());
        let text = self.skeleton_text(&gvk, namespace.as_deref(), cx);
        self.generated = Some((gvk, text.clone()));
        self.picker_open = false;
        self.set_buffer(&text, false, window, cx);
        self.editor.update(cx, |state, cx| state.focus(window, cx));
    }

    fn skeleton_text(
        &mut self,
        gvk: &Gvk,
        namespace: Option<&str>,
        cx: &mut Context<Self>,
    ) -> String {
        if let Some(template) = templates::template_for(gvk) {
            return template.text(namespace.unwrap_or("default"));
        }
        let doc = self.schema_lookup(gvk, cx).map(|(doc, _)| doc);
        let schema = doc.as_deref().and_then(|d| Schema::for_gvk(d, gvk));
        templates::skeleton(schema.as_ref(), gvk, namespace)
    }

    /// Once the schema arrives, an untouched skeleton gets the kind's required fields.
    fn regenerate_skeleton(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((gvk, text)) = self.generated.clone() else {
            return;
        };
        if self.text(cx) != text {
            self.generated = None;
            return;
        }
        let namespace = parse::parse(&text).roots().next().and_then(|r| {
            r.find(&Path::keys(&["metadata", "namespace"]))?
                .as_str()
                .map(String::from)
        });
        let fresh = self.skeleton_text(&gvk, namespace.as_deref(), cx);
        if fresh != text {
            self.generated = Some((gvk, fresh.clone()));
            self.set_buffer(&fresh, false, window, cx);
        }
    }

    pub(crate) fn start_template(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(template) = templates::TEMPLATES.get(index) else {
            return;
        };
        let gvk = template.gvk();
        match self
            .discovery(cx)
            .and_then(|d| d.by_gvk(&gvk).map(|r| r.gvr.clone()))
        {
            Some(gvr) => self.start_kind(&gvr, window, cx),
            None => {
                let text = template.text(self.namespace.as_deref().unwrap_or("default"));
                self.gvk = Some(gvk);
                self.picker_open = false;
                self.set_buffer(&text, false, window, cx);
            }
        }
    }

    /// Files dropped on a new-resource tab: their documents fill the buffer.
    pub(crate) fn load_files(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_new() {
            toast(
                cx,
                Notification::info("Drop YAML files on a New Resource tab to apply them."),
            );
            return;
        }
        let read = cx.background_spawn(async move {
            let mut docs = Vec::new();
            for path in paths {
                match std::fs::read_to_string(&path) {
                    Ok(text) => docs.push(text.trim_end().to_string()),
                    Err(err) => return Err(format!("{}: {err}", path.display())),
                }
            }
            Ok(docs.join("\n---\n") + "\n")
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(text) => {
                    this.picker_open = false;
                    this.set_buffer(&text, false, window, cx);
                }
                Err(err) => toast(cx, Notification::error(err)),
            })
            .ok();
        })
        .detach();
    }

    // ----- Dry run and apply -----

    pub(crate) fn run(&mut self, dry_run: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(cluster) = self.cluster.clone() else {
            toast(cx, Notification::error("Pick a cluster first (⌘K → @)."));
            return;
        };
        let caps = self.caps(cx);
        if !dry_run && caps.read_only {
            let message = format!("{} is read-only.", self.cluster_name(cx));
            toast(cx, Notification::error(message));
            return;
        }
        let Some(discovery) = self.discovery(cx) else {
            toast(cx, Notification::error("Not connected."));
            return;
        };
        let text = self.text(cx);
        let parsed = parse::parse(&text);
        let docs = match apply::prepare(
            &parsed,
            &discovery,
            self.namespace.as_deref(),
            self.secret.as_ref(),
        ) {
            Ok(docs) => docs,
            Err(problems) => {
                self.server_problems = problems;
                self.panel_tab = PanelTab::Problems;
                self.panel_open = true;
                self.push_diagnostics(cx);
                cx.notify();
                return;
            }
        };
        let options = apply::Options {
            dry_run,
            force: false,
        };
        let confirm =
            !dry_run && caps.production && Settings::get::<YamlSettings>(cx).confirm_apply_on_prod;
        if !confirm {
            self.execute(cluster, docs, options, text, window, cx);
            return;
        }
        let mut spec = ConfirmSpec::new(format!("Apply to {}?", self.cluster_name(cx)), "Apply");
        spec.lines = if self.is_new() || docs.len() > 1 {
            docs.iter().map(|d| SharedString::from(d.label())).collect()
        } else if self.diff.summary.is_empty() {
            vec!["No changes vs live".into()]
        } else {
            self.diff
                .summary
                .iter()
                .map(|s| SharedString::from(s.clone()))
                .collect()
        };
        spec.note = Some(
            format!(
                "Server-side apply as “kubyl” on {} (production).",
                self.cluster_name(cx)
            )
            .into(),
        );
        spec.typed = Some(match docs.as_slice() {
            [one] => one.name.clone(),
            many => format!("apply {}", many.len()),
        });
        let weak = self.weak.clone();
        dialogs::confirm(
            spec,
            move |_, window, cx| {
                let (cluster, docs, text) = (cluster.clone(), docs.clone(), text.clone());
                weak.update(cx, |this, cx| {
                    this.execute(cluster, docs, options, text, window, cx)
                })
                .ok();
            },
            window,
            cx,
        );
    }

    fn execute(
        &mut self,
        cluster: ClusterId,
        docs: Vec<apply::Prepared>,
        options: apply::Options,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = ConnectionManager::global(cx).read(cx).client(&cluster) else {
            toast(cx, Notification::error("Not connected."));
            return;
        };
        tracing::debug!(
            docs = docs.len(),
            dry_run = options.dry_run,
            force = options.force,
            "apply"
        );
        self.busy = Some(if options.dry_run {
            "Dry run…"
        } else {
            "Applying…"
        });
        cx.notify();
        let task = spawn_kube(cx, apply::apply_all(client, docs.clone(), options));
        self.apply_task = Some(cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            this.update_in(cx, |this, window, cx| {
                this.on_results(cluster, docs, options, text, results, window, cx)
            })
            .ok();
        }));
    }

    #[allow(clippy::too_many_arguments)]
    fn on_results(
        &mut self,
        cluster: ClusterId,
        docs: Vec<apply::Prepared>,
        options: apply::Options,
        text: String,
        results: Vec<DocResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Labels and outcomes only: applied objects may be Secrets.
        tracing::debug!(
            results = ?results
                .iter()
                .map(|r| (&r.label, match &r.outcome {
                    Outcome::Applied(_) => "applied",
                    Outcome::Conflicts(_) => "conflicts",
                    Outcome::Failed { .. } => "failed",
                }))
                .collect::<Vec<_>>(),
            "apply results"
        );
        self.busy = None;
        self.results_dry_run = options.dry_run;
        let parsed = parse::parse(&text);
        self.server_problems = if self.text(cx) == text {
            apply::server_problems(&parsed, &results)
        } else {
            Vec::new()
        };
        self.push_diagnostics(cx);
        let verb = if options.dry_run {
            "Dry run passed"
        } else {
            "Applied"
        };
        let applied: Vec<DocResult> = results
            .iter()
            .filter(|r| matches!(r.outcome, Outcome::Applied(_)))
            .cloned()
            .collect();
        let failed = results.len() - applied.len();
        let conflicts: Vec<(usize, apply::Conflict)> = results
            .iter()
            .filter_map(|r| match &r.outcome {
                Outcome::Conflicts(c) => {
                    Some(c.iter().map(|c| (r.index, c.clone())).collect::<Vec<_>>())
                }
                _ => None,
            })
            .flatten()
            .collect();

        if !options.dry_run {
            for result in &applied {
                let Some(doc) = docs.iter().find(|d| d.index == result.index) else {
                    continue;
                };
                let key = ApplyHistory::key(
                    cluster.as_str(),
                    &doc.resource.group,
                    &doc.resource.plural,
                    doc.namespace.as_deref(),
                    &doc.name,
                );
                let source = text
                    .get(doc.span.clone())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                // Never Secrets (their buffer may hold decoded values).
                ApplyHistory::record(cx, key, &doc.gvk.kind, source + "\n");
            }
        }

        match (applied.len(), failed) {
            (n, 0) if n > 0 => {
                let what = if n == 1 {
                    applied[0].label.clone()
                } else {
                    format!("{n} objects")
                };
                toast(cx, Notification::success(format!("{verb}: {what}")));
            }
            (0, _) if conflicts.is_empty() => {
                self.panel_tab = PanelTab::Problems;
                self.panel_open = true;
            }
            _ => {}
        }
        if results.len() > 1 || (failed > 0 && self.server_problems.is_empty()) {
            self.panel_tab = PanelTab::Results;
            self.panel_open = true;
        }
        self.results = results;

        if !options.dry_run && !applied.is_empty() {
            if self.is_new() && docs.len() == 1 && failed == 0 {
                // The tab now edits what it created.
                let doc = &docs[0];
                let target = ResourceRef::object(
                    cluster.clone(),
                    self.discovery(cx)
                        .and_then(|d| d.by_gvk(&doc.gvk).map(|r| r.gvr.clone()))
                        .unwrap_or_else(|| {
                            Gvr::new(
                                doc.gvk.group.clone(),
                                doc.gvk.version.clone(),
                                doc.resource.plural.clone(),
                            )
                        }),
                    doc.namespace.clone(),
                    doc.name.clone(),
                );
                self.object = Some(target);
                self.state = LoadState::Loading;
                self.loaded = false;
                self.store = None;
                self.load(window, cx);
            } else if self.object.is_some() {
                self.refresh_after_apply = true;
                self.load(window, cx);
            }
            self.load_history(cx);
        }

        if !conflicts.is_empty() {
            self.confirm_force(cluster, docs, options, text, conflicts, window, cx);
        }
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn confirm_force(
        &mut self,
        cluster: ClusterId,
        docs: Vec<apply::Prepared>,
        options: apply::Options,
        text: String,
        conflicts: Vec<(usize, apply::Conflict)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut spec = ConfirmSpec::new("Fields are managed by someone else", "Force apply");
        spec.danger = true;
        spec.lines = conflicts
            .iter()
            .map(|(_, c)| format!("{}  {}", c.manager, c.field.trim_start_matches('.')).into())
            .collect();
        spec.note = Some(
            "Forcing makes “kubyl” the owner of these fields. The other managers may change them back."
                .into(),
        );
        if self.caps(cx).production {
            spec.typed = Some("force".into());
        }
        let indexes: HashSet<usize> = conflicts.iter().map(|(i, _)| *i).collect();
        let weak = self.weak.clone();
        dialogs::confirm(
            spec,
            move |_, window, cx| {
                let docs: Vec<_> = docs
                    .iter()
                    .filter(|d| indexes.contains(&d.index))
                    .cloned()
                    .collect();
                let options = apply::Options {
                    force: true,
                    ..options
                };
                let (cluster, text) = (cluster.clone(), text.clone());
                if let Err(err) = weak.update(cx, |this, cx| {
                    this.execute(cluster, docs, options, text, window, cx)
                }) {
                    tracing::warn!("force apply: {err:#}");
                }
            },
            window,
            cx,
        );
    }

    pub(crate) fn weak(&self) -> WeakEntity<Self> {
        self.weak.clone()
    }
}

fn toast(cx: &mut App, notification: Notification) {
    NotificationCenter::push(cx, notification);
}

/// References that the palette's resolver doesn't know (cert-manager's `secretName` and
/// `issuerRef`, which the mockup lists).
fn extra_references(
    object: &Value,
    namespace: Option<&str>,
) -> Vec<kubyl_palette::references::Reference> {
    use kubyl_palette::references::{RefTarget, Reference};
    let mut out = Vec::new();
    let api_version = object["apiVersion"].as_str().unwrap_or_default();
    if !api_version.starts_with("cert-manager.io/") {
        return out;
    }
    if let Some(secret) = object.pointer("/spec/secretName").and_then(Value::as_str) {
        out.push(Reference {
            relation: "Secret",
            target: RefTarget::Object {
                gvk: Gvk::new("", "v1", "Secret"),
                namespace: namespace.map(String::from),
                name: secret.to_string(),
            },
        });
    }
    if let Some(name) = object
        .pointer("/spec/issuerRef/name")
        .and_then(Value::as_str)
    {
        let kind = object
            .pointer("/spec/issuerRef/kind")
            .and_then(Value::as_str)
            .unwrap_or("Issuer");
        let group = object
            .pointer("/spec/issuerRef/group")
            .and_then(Value::as_str)
            .unwrap_or("cert-manager.io");
        out.push(Reference {
            relation: "Issuer",
            target: RefTarget::Object {
                gvk: Gvk::new(group, "v1", kind),
                namespace: (kind == "Issuer")
                    .then(|| namespace.map(String::from))
                    .flatten(),
                name: name.to_string(),
            },
        });
    }
    out
}

/// `apiVersion`/`kind` of each document in `text`.
fn document_gvks(text: &str) -> Vec<Gvk> {
    let mut out = Vec::new();
    let mut offset = 0;
    for part in text.split("\n---") {
        if let Some(gvk) = intel::doc_header(text, offset + part.len() / 2) {
            out.push(gvk);
        }
        offset += part.len() + 4;
    }
    out.dedup();
    out
}

fn analyze(
    generation: u64,
    text: &str,
    live: Option<&str>,
    schemas: &[(Gvk, Arc<Value>)],
) -> Analysis {
    let parsed = parse::parse(text);
    let mut problems = validate::syntax_problems(&parsed);
    for root in parsed.roots() {
        problems.extend(validate::validate_header(root));
        let Some(gvk) = gvk_of(root) else {
            continue;
        };
        if let Some((_, doc)) = schemas.iter().find(|(g, _)| g == &gvk)
            && let Some(schema) = Schema::for_gvk(doc, &gvk)
        {
            problems.extend(validate::validate(root, &schema));
        }
    }
    problems.sort_by_key(|p| (p.range.start, p.severity));
    problems.dedup();
    let first = parsed.roots().next();
    let lenses = Lenses {
        metadata: first
            .and_then(|r| r.entry("metadata"))
            .map(|e| e.key.span.start),
        status: first
            .and_then(|r| r.entry("status"))
            .map(|e| e.key.span.start),
    };
    let status_range = first.and_then(|r| r.entry("status")).map(|e| {
        let start = parse::line_range(text, e.key.span.start).start;
        start..e.value.span.end.max(e.key.span.end)
    });
    Analysis {
        generation,
        problems,
        diff: live
            .map(|live| diff::diff(live, text, 1))
            .unwrap_or_default(),
        lenses,
        status_range,
        first_gvk: first.and_then(gvk_of),
        namespace: first
            .and_then(|r| r.find(&Path::keys(&["metadata", "namespace"])))
            .and_then(|n| n.as_str())
            .filter(|n| !n.is_empty())
            .map(String::from),
    }
}

fn gvk_of(root: &parse::Node) -> Option<Gvk> {
    let api_version = root.get("apiVersion")?.as_str()?;
    let kind = root.get("kind")?.as_str()?;
    let (group, version) = api_version.split_once('/').unwrap_or(("", api_version));
    Some(Gvk::new(group, version, kind))
}

impl Focusable for YamlEditor {
    /// The buffer: opening or activating the tab puts the cursor in it.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.picker_open {
            return self.picker_query.read(cx).focus_handle(cx);
        }
        self.editor.read(cx).focus_handle(cx)
    }
}

impl YamlEditor {
    pub(crate) fn root_focus(&self) -> &FocusHandle {
        &self.focus
    }
}

impl TabView for YamlEditor {
    fn tab_title(&self, _: &App) -> SharedString {
        match &self.object {
            Some(target) => format!("{}.yaml", target.name.clone().unwrap_or_default()).into(),
            None => "New resource".into(),
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(kubyl_ui::IconName::Code.path())
    }

    fn is_dirty(&self, cx: &App) -> bool {
        if self.is_new() {
            return self
                .generated
                .as_ref()
                .is_none_or(|(_, t)| *t != self.text(cx))
                && !self.text(cx).trim().is_empty();
        }
        !self.diff.is_empty()
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        // New resources aren't restored: their buffer may hold Secret values.
        Some(ViewRequest::for_resource(
            ViewKind::Yaml,
            self.object.clone()?,
        ))
    }
}

/// Text for the next new-resource editor of a target (see [`crate::open_draft`]).
pub(crate) struct Draft {
    pub text: String,
    pub note: Option<SharedString>,
}

/// Drafts waiting for their editor, by target (cluster, kind and namespace).
#[derive(Default)]
pub(crate) struct PendingDrafts(pub Vec<(ResourceRef, Draft)>);

impl gpui::Global for PendingDrafts {}

/// Takes the draft queued for a new-resource editor of `target`, if any.
fn target_draft(target: &Option<ResourceRef>, cx: &mut App) -> Option<Draft> {
    let target = target.as_ref()?;
    let drafts = &mut cx.default_global::<PendingDrafts>().0;
    let index = drafts.iter().position(|(t, _)| t == target)?;
    Some(drafts.remove(index).1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queued draft fills the next new-resource editor of its target (and only that one),
    /// with its note.
    #[gpui::test]
    fn drafts_fill_the_next_editor_of_their_target(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            kubyl_ui::init(cx);
            Settings::register::<YamlSettings>(cx);
        });
        let cluster = ClusterId::new("c");
        let target = ResourceRef::list(
            cluster.clone(),
            Gvr::new("cert-manager.io", "v1", "certificates"),
            Some("shop".into()),
        );
        let text =
            "apiVersion: cert-manager.io/v1\nkind: Certificate\nmetadata:\n  name: example\n";
        cx.update(|cx| {
            cx.default_global::<PendingDrafts>().0.push((
                target.clone(),
                Draft {
                    text: text.into(),
                    note: Some("From the operator's examples.".into()),
                },
            ));
        });
        let (editor, cx) =
            cx.add_window_view(|window, cx| YamlEditor::new(Some(target.clone()), window, cx));
        editor.update(cx, |editor, cx| {
            assert_eq!(editor.text(cx), text);
            assert_eq!(
                editor.draft_note.as_deref(),
                Some("From the operator's examples.")
            );
            assert!(editor.is_new());
        });
        cx.update(|_, cx| assert!(cx.global::<PendingDrafts>().0.is_empty()));
        // The next editor of the same kind starts from its template again.
        let (second, cx) =
            cx.add_window_view(|window, cx| YamlEditor::new(Some(target.clone()), window, cx));
        second.update(cx, |editor, cx| {
            assert_ne!(editor.text(cx), text);
            assert!(editor.draft_note.is_none());
        });
    }

    #[test]
    fn finds_every_documents_kind() {
        let text = "apiVersion: v1\nkind: ConfigMap\n---\napiVersion: apps/v1\nkind: Deployment\n";
        assert_eq!(
            document_gvks(text),
            [
                Gvk::new("", "v1", "ConfigMap"),
                Gvk::new("apps", "v1", "Deployment")
            ]
        );
    }

    #[test]
    fn cert_manager_references() {
        let object = serde_json::json!({
            "apiVersion": "cert-manager.io/v1",
            "kind": "Certificate",
            "spec": {"secretName": "api-tls", "issuerRef": {"name": "letsencrypt-prod", "kind": "ClusterIssuer"}}
        });
        let refs = extra_references(&object, Some("payments"));
        let titles: Vec<String> = refs.iter().map(|r| r.title()).collect();
        assert_eq!(titles, ["Secret api-tls", "ClusterIssuer letsencrypt-prod"]);
    }
}
