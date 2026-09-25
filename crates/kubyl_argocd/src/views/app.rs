//! An Application's tab (boards 13 and 14): header with status and actions, and the sub-tabs
//! Summary, Resources (tree or list), Diff, History, Events and Controller logs.
//!
//! Kubernetes mode builds the tree from Kubyl's watches; API mode (when connected) uses Argo
//! CD's full tree and shows desired-vs-live diffs. Selecting a node makes the details dock show
//! it; Enter opens it in Kubyl, `l` its logs, `e` its YAML, `d` its diff.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, Global,
    IntoElement, KeyBinding, Render, ScrollStrategy, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, actions, div, prelude::*, uniform_list,
};
use gpui_component::button::{Button as MenuButton, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use kubyl_core::actions::OpenView;
use kubyl_core::{
    ActionRegistry, ClusterId, ColumnDef, ColumnWidth, Gvr, ResourceRef, TabView, ViewKind,
    ViewRegistry, ViewRequest, spawn_kube,
};
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::{
    ResourceSelection, ResourceStores, Selected, StoreHandle, StoreKey, object_key,
};
use kubyl_ui::{
    ActiveColors, Button, Chip, Icon, IconButton, IconName, KeyHints, ProdBadge, fonts, h_flex,
    sizes, u, v_flex,
};
use kubyl_yaml::diff::LineDiff;
use serde_json::Value;

use crate::actions::{self, APP_CONTEXT};
use crate::api::{ApiError, ResourceDiff, ResourceTree};
use crate::dialogs;
use crate::links;
use crate::model::{Application, HistoryEntry, OperationPhase, SyncStatus, short_revision};
use crate::nav::{self, Move, TABLE};
use crate::ops::PolicyChange;
use crate::run::{self, Op};
use crate::state::ArgoCd;
use crate::tree::{self, Live, Node, Show};
use crate::widgets;

/// `ViewKind::Custom` of an application's tab.
pub const VIEW_KIND: &str = "argocd_app";
const ROW_HEIGHT: f32 = 30.0;
/// How often API-mode data is fetched again while shown.
const API_REFRESH: Duration = Duration::from_secs(10);

actions!(
    argocd_app,
    [
        /// Opens the logs of the selected tree node (pods and workloads).
        NodeLogs,
        /// Opens the selected tree node in the YAML editor.
        NodeYaml,
        /// Shows the diff of the selected tree node (API mode).
        NodeDiff,
        /// Switches between the tree and the flat list.
        ToggleTreeMode,
    ]
);

const TREE_CONTEXT: &str = "ArgoTree";

pub(crate) fn init(cx: &mut App) {
    let tree = Some(TREE_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("l", NodeLogs, tree),
        KeyBinding::new("e", NodeYaml, tree),
        KeyBinding::new("d", NodeDiff, tree),
        KeyBinding::new("t", ToggleTreeMode, tree),
    ]);
    cx.default_global::<PendingTab>();
}

/// The sub-tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Summary,
    Resources,
    Diff,
    History,
    Events,
    Logs,
}

impl Tab {
    const ALL: [Tab; 6] = [
        Tab::Summary,
        Tab::Resources,
        Tab::Diff,
        Tab::History,
        Tab::Events,
        Tab::Logs,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Summary => "Summary",
            Tab::Resources => "Resources",
            Tab::Diff => "Diff",
            Tab::History => "History",
            Tab::Events => "Events",
            Tab::Logs => "Controller logs",
        }
    }
}

/// A sub-tab for the next tab that opens for a target (History from `h`).
#[derive(Default)]
pub struct PendingTab(Option<(ResourceRef, Tab)>);

impl Global for PendingTab {}

impl PendingTab {
    pub fn set(target: &ResourceRef, tab: Tab, cx: &mut App) {
        cx.set_global(PendingTab(Some((target.clone(), tab))));
    }

    fn take(target: &ResourceRef, cx: &mut App) -> Option<Tab> {
        let pending = cx.try_global::<PendingTab>()?.0.clone()?;
        (pending.0 == *target).then(|| {
            cx.set_global(PendingTab(None));
            pending.1
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TreeMode {
    Tree,
    List,
}

type Diffs = Result<Vec<(ResourceDiff, LineDiff)>, String>;

pub struct AppView {
    target: ResourceRef,
    own: StoreHandle,
    object: Option<Arc<Value>>,
    app: Option<Arc<Application>>,
    gone: bool,
    tab: Tab,
    focus: FocusHandle,
    // Resources.
    tree_mode: TreeMode,
    show: Show,
    collapsed: HashSet<String>,
    tree_filter: Entity<InputState>,
    nodes: Vec<Node>,
    rows: Vec<Node>,
    selected_node: Option<String>,
    tree_scroll: UniformListScrollHandle,
    live: HashMap<StoreKey, (String, StoreHandle)>,
    _live_observers: Vec<Subscription>,
    api_tree: Option<Arc<ResourceTree>>,
    api_error: Option<String>,
    _api_task: Option<Task<()>>,
    last_version: String,
    // Diff.
    diffs: Option<Diffs>,
    diff_selected: usize,
    side_by_side: bool,
    _diff_task: Option<Task<()>>,
    // History.
    history_selected: Option<i64>,
    history_scroll: UniformListScrollHandle,
    // Events.
    events: Option<StoreHandle>,
    _events_observer: Option<Subscription>,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = StoreKey::new(
            target.cluster.clone(),
            target.gvr.clone(),
            target.namespace.clone(),
        )
        .fields(format!(
            "metadata.name={}",
            target.name.as_deref().unwrap_or_default()
        ));
        let own = ResourceStores::acquire(cx, key);
        let tree_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let mut subscriptions = vec![
            cx.observe(own.entity(), |this, _, cx| this.object_changed(cx)),
            cx.subscribe_in(
                &tree_filter,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.rebuild_rows(cx);
                    }
                },
            ),
        ];
        if let Some(argo) = ArgoCd::try_global(cx) {
            let mut connected = false;
            subscriptions.push(cx.observe(&argo, move |this, argo, cx| {
                let now = argo.read(cx).api(&this.target.cluster).is_some();
                if now != connected {
                    connected = now;
                    this.api_tree = None;
                    this.diffs = None;
                    this.sync_live_stores(cx);
                    this.fetch_api(cx);
                }
                cx.notify();
            }));
        }
        if let Some(manager) = ConnectionManager::try_global(cx) {
            let cluster = target.cluster.clone();
            subscriptions.push(cx.subscribe(
                &manager,
                move |this, _, event: &ConnectionEvent, cx| {
                    if matches!(event, ConnectionEvent::DiscoveryChanged(id) if *id == cluster) {
                        this.sync_live_stores(cx);
                    }
                },
            ));
        }
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(API_REFRESH).await;
                if this.update(cx, |this, cx| this.fetch_api(cx)).is_err() {
                    break;
                }
            }
        });
        // `h` on an app whose tab is open already switches that tab.
        subscriptions.push(cx.observe_global::<PendingTab>(|this, cx| {
            if let Some(tab) = PendingTab::take(&this.target, cx) {
                this.set_tab(tab, cx);
            }
        }));
        let tab = PendingTab::take(&target, cx).unwrap_or(Tab::Resources);
        let mut this = Self {
            target,
            own,
            object: None,
            app: None,
            gone: false,
            tab,
            focus: cx.focus_handle(),
            tree_mode: TreeMode::Tree,
            show: Show::All,
            collapsed: HashSet::new(),
            tree_filter,
            nodes: Vec::new(),
            rows: Vec::new(),
            selected_node: None,
            tree_scroll: UniformListScrollHandle::new(),
            live: HashMap::new(),
            _live_observers: Vec::new(),
            api_tree: None,
            api_error: None,
            _api_task: None,
            last_version: String::new(),
            diffs: None,
            diff_selected: 0,
            side_by_side: false,
            _diff_task: None,
            history_selected: None,
            history_scroll: UniformListScrollHandle::new(),
            events: None,
            _events_observer: None,
            _ticker: ticker,
            _subscriptions: subscriptions,
        };
        this.object_changed(cx);
        this
    }

    fn cluster(&self) -> &ClusterId {
        &self.target.cluster
    }

    fn name(&self) -> &str {
        self.target.name.as_deref().unwrap_or_default()
    }

    fn api_connected(&self, cx: &App) -> bool {
        ArgoCd::try_global(cx).is_some_and(|a| a.read(cx).api(self.cluster()).is_some())
    }

    fn object_changed(&mut self, cx: &mut Context<Self>) {
        let key = object_key(self.target.namespace.as_deref(), self.name());
        let store = self.own.read(cx);
        let object = store.get(&key).cloned();
        let ready = store.status().is_ready();
        self.gone = object.is_none() && ready;
        let version = object
            .as_ref()
            .and_then(|o| o.pointer("/metadata/resourceVersion"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(object) = &object
            && self.object.is_none()
        {
            // First load: the dock follows the app.
            self.publish_app_selection(object.clone(), cx);
        }
        self.app = object
            .as_ref()
            .and_then(|o| Application::parse(o))
            .map(Arc::new);
        self.object = object;
        if version != self.last_version {
            self.last_version = version;
            self.sync_live_stores(cx);
            self.ensure_events(cx);
            // The app changed (a sync finished, a refresh compared): fetch the API data again.
            self.fetch_api(cx);
        }
        self.rebuild(cx);
    }

    fn publish_app_selection(&self, object: Arc<Value>, cx: &mut Context<Self>) {
        let caps = ConnectionManager::try_global(cx)
            .map(|m| m.read(cx).caps(self.cluster()))
            .unwrap_or_default();
        ResourceSelection::set(
            cx,
            ResourceSelection {
                items: vec![Selected {
                    target: self.target.clone(),
                    kind: "Application".into(),
                    object: Some(object),
                    store: Some(self.own.entity().clone()),
                }],
                caps,
            },
        );
    }

    // ----- Live objects (Kubernetes mode) -----

    /// Watches the app's managed kinds and their children's kinds, per namespace.
    fn sync_live_stores(&mut self, cx: &mut Context<Self>) {
        let (Some(app), false) = (self.app.clone(), self.api_connected(cx)) else {
            if !self.live.is_empty() {
                self.live.clear();
                self._live_observers.clear();
            }
            return;
        };
        let Some(discovery) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(self.cluster()))
        else {
            return;
        };
        let mut wanted: HashMap<StoreKey, String> = HashMap::new();
        let mut add = |gvr: &Gvr, kind: &str, namespace: &str, namespaced: bool| {
            let ns = (namespaced && !namespace.is_empty()).then(|| namespace.to_string());
            wanted.insert(
                StoreKey::new(self.target.cluster.clone(), gvr.clone(), ns),
                kind.to_string(),
            );
        };
        for resource in &app.status.resources {
            let info = discovery.preferred().find(|r| {
                r.gvk.group == resource.group && r.gvk.kind == resource.kind && r.is_listable()
            });
            let Some(info) = info else { continue };
            add(
                &info.gvr,
                &info.gvk.kind,
                &resource.namespace,
                info.namespaced,
            );
            for (group, plural) in tree::child_kinds(&resource.group, &resource.kind) {
                if let Some(child) = discovery
                    .preferred()
                    .find(|r| r.gvr.group == *group && r.gvr.resource == *plural && r.is_listable())
                {
                    add(
                        &child.gvr,
                        &child.gvk.kind,
                        &resource.namespace,
                        child.namespaced,
                    );
                }
            }
        }
        let current: HashSet<&StoreKey> = self.live.keys().collect();
        let next: HashSet<&StoreKey> = wanted.keys().collect();
        if current == next {
            return;
        }
        self.live.retain(|key, _| wanted.contains_key(key));
        for (key, kind) in wanted {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.live.entry(key) {
                let handle = ResourceStores::acquire(cx, entry.key().clone());
                entry.insert((kind, handle));
            }
        }
        self._live_observers = self
            .live
            .values()
            .map(|(_, handle)| cx.observe(handle.entity(), |this, _, cx| this.rebuild(cx)))
            .collect();
    }

    fn live_objects(&self, cx: &App) -> Live {
        let mut live = Live::default();
        for (key, (kind, handle)) in &self.live {
            let store = handle.read(cx);
            for object in store.objects().values() {
                live.insert(&key.gvr.group, kind, object.clone());
            }
            if store.status().is_ready() {
                // Every namespace this watch covers.
                let namespaces: Vec<String> = match &key.namespace {
                    Some(ns) => vec![ns.clone()],
                    None => vec![String::new()],
                };
                for ns in namespaces {
                    live.mark_ready(&key.gvr.group, kind, &ns);
                }
            }
        }
        live
    }

    // ----- API mode -----

    fn fetch_api(&mut self, cx: &mut Context<Self>) {
        let Some(api) = ArgoCd::try_global(cx).and_then(|a| a.read(cx).api(self.cluster())) else {
            return;
        };
        let Some(app) = run::app_target(&self.target) else {
            return;
        };
        let fetch_diffs = self.tab == Tab::Diff || self.diffs.is_some();
        let tree_api = api.clone();
        let tree_app = app.clone();
        let tree_task = spawn_kube(cx, async move { tree_api.resource_tree(&tree_app).await });
        self._api_task = Some(cx.spawn(async move |this, cx| {
            let result = tree_task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(tree) => {
                        this.api_tree = Some(Arc::new(tree));
                        this.api_error = None;
                    }
                    Err(err) => this.api_failed(err, cx),
                }
                this.rebuild(cx);
            })
            .ok();
        }));
        if fetch_diffs {
            let task = spawn_kube(cx, async move {
                let items = api.managed_resources(&app).await?;
                // Diffs are computed off the UI thread.
                Ok::<_, ApiError>(
                    items
                        .into_iter()
                        .map(|item| {
                            let diff = crate::diff::resource_diff(&item);
                            (item, diff)
                        })
                        .filter(|(_, diff)| !diff.is_empty())
                        .collect::<Vec<_>>(),
                )
            });
            self._diff_task = Some(cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(diffs) => {
                            this.diff_selected =
                                this.diff_selected.min(diffs.len().saturating_sub(1));
                            this.diffs = Some(Ok(diffs));
                        }
                        Err(err) => {
                            this.diffs = Some(Err(err.to_string()));
                            this.api_failed(err, cx);
                        }
                    }
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    fn api_failed(&mut self, err: ApiError, cx: &mut Context<Self>) {
        if err == ApiError::Unauthorized
            && let Some(argo) = ArgoCd::try_global(cx)
        {
            let cluster = self.cluster().clone();
            argo.update(cx, |argo, cx| argo.unauthorized(&cluster, cx));
        }
        self.api_error = Some(err.to_string());
    }

    // ----- Tree -----

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let Some(app) = self.app.clone() else {
            self.nodes.clear();
            self.rows.clear();
            cx.notify();
            return;
        };
        self.nodes = match (&self.api_tree, self.api_connected(cx)) {
            (Some(tree), true) => tree::build_api(&app, tree),
            _ => tree::build_kubernetes(&app, &self.live_objects(cx)),
        };
        self.rebuild_rows(cx);
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let query = self.tree_filter.read(cx).value().to_string();
        self.rows = match self.tree_mode {
            TreeMode::Tree => tree::visible(&self.nodes, &self.collapsed, self.show, &query),
            TreeMode::List => {
                tree::visible(&tree::flat(&self.nodes), &HashSet::new(), self.show, &query)
            }
        };
        cx.notify();
    }

    fn selected_row(&self) -> Option<(usize, &Node)> {
        let id = self.selected_node.as_ref()?;
        self.rows.iter().enumerate().find(|(_, n)| &n.id == id)
    }

    fn select_node(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.selected_node = index.and_then(|i| self.rows.get(i)).map(|n| n.id.clone());
        if let Some(index) = index {
            self.tree_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        // The dock follows the selected resource.
        if let Some(node) = index.and_then(|i| self.rows.get(i)).cloned() {
            if node.is_app() {
                if let Some(object) = self.object.clone() {
                    self.publish_app_selection(object, cx);
                }
            } else if let Some(target) = self.node_ref(&node, cx) {
                let caps = ConnectionManager::try_global(cx)
                    .map(|m| m.read(cx).caps(self.cluster()))
                    .unwrap_or_default();
                ResourceSelection::set(
                    cx,
                    ResourceSelection {
                        items: vec![Selected {
                            target,
                            kind: node.kind.clone(),
                            object: None,
                            store: None,
                        }],
                        caps,
                    },
                );
            }
        }
        cx.notify();
    }

    /// The Kubyl reference of a tree node (its preferred version from discovery).
    fn node_ref(&self, node: &Node, cx: &App) -> Option<ResourceRef> {
        let discovery = ConnectionManager::try_global(cx)?
            .read(cx)
            .discovery(self.cluster())?;
        let info = discovery
            .preferred()
            .find(|r| r.gvk.group == node.group && r.gvk.kind == node.kind)?;
        Some(ResourceRef::object(
            self.cluster().clone(),
            info.gvr.clone(),
            info.namespaced.then(|| node.namespace.clone()),
            node.name.clone(),
        ))
    }

    fn open_node(&mut self, kind: Option<ViewKind>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, node)) = self.selected_row() else {
            return;
        };
        if node.is_app() {
            self.tab = Tab::Summary;
            cx.notify();
            return;
        }
        let node = node.clone();
        let Some(target) = self.node_ref(&node, cx) else {
            return;
        };
        let kind = kind.unwrap_or_else(|| ViewRegistry::object_view(cx, &target.gvr));
        if kind == ViewKind::Logs && !kubyl_logs::logs_applicable(&target.gvr.resource) {
            return;
        }
        window.dispatch_action(
            Box::new(OpenView(ViewRequest::for_resource(kind, target))),
            cx,
        );
    }

    fn node_diff(&mut self, cx: &mut Context<Self>) {
        let Some((_, node)) = self.selected_row() else {
            return;
        };
        let id = node.id.clone();
        self.tab = Tab::Diff;
        if let Some(Ok(diffs)) = &self.diffs
            && let Some(index) = diffs.iter().position(|(item, _)| {
                tree::node_id(&item.group, &item.kind, &item.namespace, &item.name) == id
            })
        {
            self.diff_selected = index;
        }
        self.fetch_api(cx);
        cx.notify();
    }

    fn toggle_node(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_string());
        }
        self.rebuild_rows(cx);
    }

    fn move_selection(&mut self, movement: Move, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Resources => {
                let current = self.selected_row().map(|(i, _)| i);
                let next = nav::step(current, self.rows.len(), movement);
                self.select_node(next, cx);
            }
            Tab::History => {
                let history = self.history();
                let current = self
                    .history_selected
                    .and_then(|id| history.iter().position(|h| h.id == id));
                if let Some(next) = nav::step(current, history.len(), movement) {
                    self.history_selected = Some(history[next].id);
                    self.history_scroll
                        .scroll_to_item(next, ScrollStrategy::Nearest);
                }
                cx.notify();
            }
            Tab::Diff => {
                if let Some(Ok(diffs)) = &self.diffs
                    && let Some(next) = nav::step(Some(self.diff_selected), diffs.len(), movement)
                {
                    self.diff_selected = next;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab {
            Tab::Resources => self.open_node(None, window, cx),
            Tab::History => {
                if let Some(url) = self.history_selected.and_then(|id| self.commit_url(id)) {
                    cx.open_url(&url);
                }
            }
            _ => {}
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        if tab == Tab::Diff && self.diffs.is_none() {
            self.fetch_api(cx);
        }
        cx.notify();
    }

    // ----- History -----

    fn history(&self) -> Vec<HistoryEntry> {
        self.app
            .as_ref()
            .map(|a| a.history_newest_first())
            .unwrap_or_default()
    }

    fn commit_url(&self, id: i64) -> Option<String> {
        let entry = self.history().into_iter().find(|h| h.id == id)?;
        let source = entry.all_sources().into_iter().next()?;
        let revision = entry.all_revisions().into_iter().next()?;
        links::commit_url(&source.repo_url, &revision)
    }

    // ----- Events -----

    fn ensure_events(&mut self, cx: &mut Context<Self>) {
        if self.events.is_some() {
            return;
        }
        let Some(uid) = self
            .object
            .as_ref()
            .and_then(|o| o.pointer("/metadata/uid"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let key = StoreKey::new(
            self.cluster().clone(),
            Gvr::new("", "v1", "events"),
            self.target.namespace.clone(),
        )
        .fields(format!("involvedObject.uid={uid}"));
        let handle = ResourceStores::acquire(cx, key);
        self._events_observer = Some(cx.observe(handle.entity(), |_, _, cx| cx.notify()));
        self.events = Some(handle);
    }

    // ----- Actions on the app -----

    fn writable(&self, cx: &App) -> bool {
        !run::read_only(self.cluster(), cx) && !self.gone
    }

    fn run(&self, op: Op, cx: &mut App) {
        run::run(self.target.clone(), op, cx).detach();
    }

    fn open_controller_logs(&self, window: &mut Window, cx: &mut App) {
        let Some(app) = &self.app else {
            return;
        };
        let install = ArgoCd::try_global(cx).and_then(|a| {
            a.read(cx).install_for(
                self.cluster(),
                app.status
                    .controller_namespace
                    .as_deref()
                    .or(Some(app.namespace())),
            )
        });
        let Some((install, controller)) = install.and_then(|i| {
            let controller = i.controller.clone()?;
            Some((i, controller))
        }) else {
            kubyl_core::NotificationCenter::push(
                cx,
                kubyl_core::Notification::error("The Argo CD application controller wasn't found."),
            );
            return;
        };
        let target = ResourceRef::object(
            self.cluster().clone(),
            Gvr::new("apps", "v1", controller.resource),
            Some(install.namespace.clone()),
            controller.name,
        );
        kubyl_logs::open_filtered(target, controller_query(app), true, window, cx);
    }
}

/// Lines of the controller's logs about `app`: JSON (`"application":"guestbook"`, 3.x
/// default) or text (`application=guestbook`), in its namespace.
pub fn controller_query(app: &Application) -> String {
    let name = regex_escape(app.name());
    format!(
        r#""application":"{name}"|application={name}(\s|$)|app-qualified-name={ns}/{name}"#,
        ns = regex_escape(app.namespace())
    )
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// ----- Rendering -----

impl AppView {
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(app) = self.app.clone() else {
            return div().into_any_element();
        };
        let api_state = ArgoCd::try_global(cx)
            .map(|a| a.read(cx).api_state(self.cluster()))
            .unwrap_or_default();
        let policy = app.policy();
        let production = ConnectionManager::try_global(cx)
            .is_some_and(|m| m.read(cx).caps(self.cluster()).production);
        let writable = self.writable(cx);
        let auto = if policy.auto_sync() {
            let mut label = "auto-sync".to_string();
            if policy.prune() {
                label.push_str(" · prune");
            }
            if policy.self_heal() {
                label.push_str(" · self-heal");
            }
            h_flex()
                .gap(u(4.0))
                .text_color(colors.green)
                .child(
                    Icon::new(IconName::RefreshCw)
                        .size(12.0)
                        .color(colors.green),
                )
                .child(label)
                .into_any_element()
        } else {
            div()
                .text_color(colors.text_dim)
                .child("manual sync")
                .into_any_element()
        };
        let weak = cx.entity().downgrade();
        let refresh_menu = {
            let weak = weak.clone();
            MenuButton::new("argo-app-refresh")
                .compact()
                .child(
                    h_flex()
                        .gap(u(5.0))
                        .text_size(u(12.5))
                        .child(Icon::new(IconName::RefreshCw).size(12.0))
                        .child("Refresh")
                        .child(Icon::new(IconName::ChevronDown).size(11.0)),
                )
                .dropdown_menu(move |menu, _, _| {
                    let normal = weak.clone();
                    let hard = weak.clone();
                    menu.item(PopupMenuItem::new("Refresh").on_click(move |_, _, cx| {
                        normal
                            .update(cx, |this, cx| this.run(Op::Refresh { hard: false }, cx))
                            .ok();
                    }))
                    .item(
                        PopupMenuItem::new("Hard refresh (regenerate manifests)").on_click(
                            move |_, _, cx| {
                                hard.update(cx, |this, cx| {
                                    this.run(Op::Refresh { hard: true }, cx)
                                })
                                .ok();
                            },
                        ),
                    )
                })
        };
        let target = self.target.clone();
        let sync = Button::new("argo-app-sync")
            .primary()
            .icon(IconName::RefreshCw)
            .label("Sync…")
            .disabled(!writable || app.is_deleting())
            .on_click(move |_, window, cx| dialogs::open_sync(target.clone(), window, cx));
        let ui_cluster = self.cluster().clone();
        let argo_ui = Button::new("argo-app-ui")
            .ghost()
            .icon(IconName::Globe)
            .label("Argo CD UI")
            .on_click(move |_, window, cx| actions::open_argo_ui(&ui_cluster, window, cx));
        let more = {
            let weak = weak.clone();
            let target = self.target.clone();
            let running = app.operation_in_progress();
            let auto_on = policy.auto_sync();
            MenuButton::new("argo-app-more")
                .ghost()
                .compact()
                .child(Icon::new(IconName::Ellipsis).size(14.0))
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    if writable {
                        let rollback = target.clone();
                        let delete = target.clone();
                        let toggle = weak.clone();
                        let terminate = weak.clone();
                        menu = menu
                            .item(
                                PopupMenuItem::new("Rollback…").on_click(move |_, window, cx| {
                                    dialogs::open_rollback(rollback.clone(), None, window, cx)
                                }),
                            )
                            .item(
                                PopupMenuItem::new("Terminate Operation")
                                    .disabled(!running)
                                    .on_click(move |_, _, cx| {
                                        terminate
                                            .update(cx, |this, cx| this.run(Op::Terminate, cx))
                                            .ok();
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(if auto_on {
                                    "Disable Auto-Sync"
                                } else {
                                    "Enable Auto-Sync"
                                })
                                .on_click(move |_, _, cx| {
                                    toggle
                                        .update(cx, |this, cx| {
                                            this.run(
                                                Op::Policy(PolicyChange::AutoSync(!auto_on)),
                                                cx,
                                            )
                                        })
                                        .ok();
                                }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("Delete…").on_click(move |_, window, cx| {
                                    dialogs::open_delete(delete.clone(), window, cx)
                                }),
                            )
                            .separator();
                    }
                    let yaml = target.clone();
                    let name = target.name.clone().unwrap_or_default();
                    menu.item(
                        PopupMenuItem::new("Edit YAML").on_click(move |_, window, cx| {
                            window.dispatch_action(
                                Box::new(OpenView(ViewRequest::for_resource(
                                    ViewKind::Yaml,
                                    yaml.clone(),
                                ))),
                                cx,
                            )
                        }),
                    )
                    .item(PopupMenuItem::new("Copy Name").on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(name.clone()));
                    }))
                })
        };
        let tile = div()
            .flex_none()
            .size(u(30.0))
            .rounded(u(7.0))
            .bg(colors.orange.opacity(0.13))
            .border_1()
            .border_color(colors.orange.opacity(0.33))
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(IconName::Layers).size(16.0).color(colors.orange));
        h_flex()
            .flex_none()
            .gap(u(10.0))
            .px(u(16.0))
            .pt(u(12.0))
            .pb(u(10.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(tile)
            .child(
                v_flex()
                    .min_w_0()
                    .gap(u(3.0))
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(
                                div()
                                    .font_family(fonts::MONO)
                                    .text_size(u(15.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(app.name().to_string()),
                            )
                            .child(Chip::new(app.spec.project.clone()))
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(colors.text_dim)
                                    .child(format!("{} namespace", app.namespace())),
                            )
                            .when(production, |this| this.child(ProdBadge)),
                    )
                    .child(
                        h_flex()
                            .gap(u(12.0))
                            .text_size(u(12.0))
                            .child(widgets::sync_pill(app.sync(), &colors))
                            .child(widgets::health_pill(app.health(), &colors))
                            .child(auto)
                            .when_some(app.activity(), |this, activity| {
                                this.child(widgets::activity_chip(&activity, &colors))
                            }),
                    ),
            )
            .child(div().flex_1())
            .child(super::mode_button(
                "argo-app-mode",
                self.cluster(),
                &api_state,
                cx,
            ))
            .when(writable, |this| this.child(refresh_menu).child(sync))
            .child(argo_ui)
            .child(more)
            .into_any_element()
    }

    fn render_subtabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let colors = cx.colors().clone();
        let app = self.app.clone();
        let count = |tab: Tab| -> Option<String> {
            let app = app.as_ref()?;
            match tab {
                Tab::Resources => Some(app.status.resources.len().to_string()),
                Tab::History => Some(app.status.history.len().to_string()),
                Tab::Diff => match &self.diffs {
                    Some(Ok(diffs)) => Some(diffs.len().to_string()),
                    _ => {
                        (!app.out_of_sync().is_empty()).then(|| app.out_of_sync().len().to_string())
                    }
                },
                _ => None,
            }
        };
        h_flex()
            .flex_none()
            .h(u(36.0))
            .px(u(12.0))
            .gap(u(2.0))
            .items_stretch()
            .border_b_1()
            .border_color(colors.border_variant)
            .children(Tab::ALL.into_iter().map(|tab| {
                let active = self.tab == tab;
                h_flex()
                    .id(SharedString::from(format!("argo-tab-{}", tab.label())))
                    .px(u(10.0))
                    .gap(u(6.0))
                    .cursor_pointer()
                    .text_color(if active { colors.text } else { colors.text_dim })
                    .when(active, |this| this.border_b_2().border_color(colors.accent))
                    .hover(|s| s.text_color(colors.text))
                    .child(tab.label())
                    .when_some(count(tab), |this, count| this.child(Chip::new(count)))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
            }))
    }

    fn render_summary_column(&self, compact: bool, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(app) = self.app.clone() else {
            return div().into_any_element();
        };
        let writable = self.writable(cx);
        let policy = app.policy();
        let mut column = v_flex().min_w_0();

        // Sources.
        let mut sources = widgets::section(
            if app.spec.is_multi_source() {
                "Sources"
            } else {
                "Source"
            },
            &colors,
        );
        let synced = app.synced_revisions();
        for (index, source) in app.spec.all_sources().into_iter().enumerate() {
            let repo = source.repo_short();
            let repo_link =
                links::tree_url(&source.repo_url, source.target(), source.path.as_deref());
            let revision = synced.get(index).cloned();
            sources = sources.child(
                h_flex()
                    .items_start()
                    .gap(u(8.0))
                    .child(
                        Icon::new(IconName::GitBranch)
                            .size(13.0)
                            .color(colors.text_dim),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .text_size(u(12.0))
                            .child(match repo_link {
                                Some(url) => {
                                    widgets::url_link(("argo-src", index), repo, url, &colors)
                                        .into_any_element()
                                }
                                None => div().truncate().child(repo).into_any_element(),
                            })
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .text_color(colors.text_muted)
                                    .child(format!("{} @ {}", source.what(), source.target())),
                            )
                            .when_some(revision, |this, revision| {
                                let link = links::commit_url(&source.repo_url, &revision);
                                let short = short_revision(&revision);
                                this.child(
                                    h_flex()
                                        .gap(u(4.0))
                                        .text_color(colors.text_dim)
                                        .child("synced")
                                        .child(match link {
                                            Some(url) => widgets::url_link(
                                                ("argo-rev", index),
                                                short,
                                                url,
                                                &colors,
                                            )
                                            .into_any_element(),
                                            None => widgets::mono(short),
                                        }),
                                )
                            }),
                    ),
            );
        }
        let destination = app.spec.destination.clone();
        let dest_cluster = links::destination_cluster(
            &destination,
            self.cluster(),
            &crate::apps::known_clusters(cx),
        );
        let dest_label = destination.label();
        let dest = match dest_cluster {
            Some(cluster) => {
                let ns = destination.namespace.clone();
                widgets::link(
                    "argo-app-dest",
                    dest_label,
                    &colors,
                    move |_, window, cx| super::open_destination(&cluster, ns.clone(), window, cx),
                )
                .into_any_element()
            }
            None => widgets::text(dest_label),
        };
        let mut facts = vec![("Destination", dest)];
        if let Some(kind) = app
            .status
            .source_type
            .clone()
            .or_else(|| app.status.source_types.first().cloned())
        {
            facts.push(("Type", widgets::text(kind)));
        }
        if let Some(set) = app.application_set() {
            facts.push(("From", widgets::text(format!("ApplicationSet {set}"))));
        }
        sources = sources.child(widgets::kv(facts, &colors));
        column = column.child(sources);

        // Sync policy.
        let weak = cx.entity().downgrade();
        let toggle_row = |id: &'static str,
                          label: &'static str,
                          on: bool,
                          enabled: bool,
                          change: fn(bool) -> PolicyChange| {
            let weak = weak.clone();
            h_flex()
                .gap(u(8.0))
                .text_size(u(12.0))
                .py(u(2.0))
                .child(div().flex_1().text_color(colors.text_muted).child(label))
                .child(widgets::toggle(
                    id,
                    on,
                    enabled,
                    &colors,
                    move |_, _, cx| {
                        weak.update(cx, |this, cx| this.run(Op::Policy(change(!on)), cx))
                            .ok();
                    },
                ))
        };
        let mut policy_section = widgets::section("Sync policy", &colors)
            .child(toggle_row(
                "argo-auto",
                "Auto-sync",
                policy.auto_sync(),
                writable,
                PolicyChange::AutoSync,
            ))
            .child(toggle_row(
                "argo-prune",
                "Prune",
                policy.prune(),
                writable && policy.auto_sync(),
                PolicyChange::Prune,
            ))
            .child(toggle_row(
                "argo-heal",
                "Self-heal",
                policy.self_heal(),
                writable && policy.auto_sync(),
                PolicyChange::SelfHeal,
            ));
        let mut options: Vec<String> = policy.sync_options.clone();
        if let Some(retry) = policy.retry_label() {
            options.push(retry);
        }
        if !options.is_empty() {
            policy_section = policy_section.child(
                h_flex()
                    .flex_wrap()
                    .gap(u(4.0))
                    .children(options.into_iter().map(|o| Chip::new(o).mono())),
            );
        }
        column = column.child(policy_section);

        // Conditions.
        if !app.status.conditions.is_empty() {
            let mut conditions = widgets::section("Conditions", &colors);
            for condition in &app.status.conditions {
                let (icon, color) = if condition.is_error() {
                    (IconName::CircleX, colors.red)
                } else {
                    (IconName::TriangleAlert, colors.yellow)
                };
                conditions = conditions.child(
                    h_flex()
                        .items_start()
                        .gap(u(8.0))
                        .text_size(u(12.0))
                        .child(Icon::new(icon).size(13.0).color(color))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    div()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(condition.kind.clone()),
                                )
                                .child(
                                    div()
                                        .text_color(colors.text_dim)
                                        .child(condition.message.clone()),
                                ),
                        ),
                );
            }
            column = column.child(conditions);
        }

        // Operation.
        column = column.child(self.render_operation(&app, compact, cx));
        if !compact && !app.status.summary.images.is_empty() {
            column = column.child(widgets::section("Images", &colors).children(
                app.status.summary.images.iter().map(|image| {
                    div()
                        .truncate()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .child(image.clone())
                }),
            ));
        }
        if !compact && !app.status.summary.external_urls.is_empty() {
            column = column.child(
                widgets::section("External URLs", &colors).children(
                    app.status
                        .summary
                        .external_urls
                        .iter()
                        .enumerate()
                        .map(|(i, url)| {
                            widgets::url_link(
                                ("argo-ext-url", i),
                                url.clone(),
                                url.clone(),
                                &colors,
                            )
                        }),
                ),
            );
        }
        column.into_any_element()
    }

    fn render_operation(
        &self,
        app: &Application,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.colors().clone();
        let running = app.operation_in_progress();
        let title = if running {
            "Operation"
        } else {
            "Last operation"
        };
        let mut section = widgets::section(title, &colors);
        let Some(state) = app.operation_state().cloned() else {
            let text = if app.operation.is_some() {
                "Sync requested; waiting for the controller."
            } else {
                "No operation yet."
            };
            return section
                .child(
                    div()
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .child(text),
                )
                .into_any_element();
        };
        let phase = state.phase();
        let by = state
            .initiator()
            .map(|i| i.label())
            .unwrap_or_else(|| "unknown".into());
        let when = if running {
            format!("started {} ago", widgets::age_of(state.started()))
        } else {
            format!("{} ago", widgets::age_of(state.finished()))
        };
        let icon = match phase {
            Some(phase) => widgets::result_icon(phase, &colors),
            None => Icon::new(IconName::Info).color(colors.text_dim),
        };
        let label = match phase {
            Some(OperationPhase::Running) => "Running".to_string(),
            Some(phase) => format!("Sync {}", phase.label().to_lowercase()),
            None => state.phase.clone(),
        };
        section = section.child(
            h_flex()
                .items_start()
                .gap(u(8.0))
                .text_size(u(12.0))
                .child(icon.size(13.0))
                .child(
                    v_flex()
                        .min_w_0()
                        .flex_1()
                        .child(format!("{label} · {when} · by {by}"))
                        .when_some(state.revision(), |this, revision| {
                            this.child(
                                div()
                                    .text_color(colors.text_dim)
                                    .child(format!("revision {}", short_revision(&revision))),
                            )
                        })
                        .when_some(
                            state.message.clone().filter(|m| !m.is_empty()),
                            |this, message| {
                                this.child(div().text_color(colors.text_dim).child(message))
                            },
                        ),
                ),
        );
        if running && self.writable(cx) {
            section = section.child(
                h_flex().child(
                    Button::new("argo-terminate")
                        .danger()
                        .icon(IconName::Square)
                        .label("Terminate")
                        .on_click(cx.listener(|this, _, _, cx| this.run(Op::Terminate, cx))),
                ),
            );
        }
        let results = state.sync_result.map(|r| r.resources).unwrap_or_default();
        let limit = if compact { 5 } else { usize::MAX };
        for result in results.iter().take(limit) {
            let failed = result.failed();
            // Argo CD sets `hookPhase: Running` on plain resources too; it means something
            // only for hooks.
            let status = if result.hook_type.is_some() {
                result.hook_phase.clone()
            } else {
                result.status.clone()
            }
            .unwrap_or_default()
            .to_lowercase();
            section = section.child(
                h_flex()
                    .gap(u(8.0))
                    .text_size(u(12.0))
                    .child(
                        Icon::new(if failed {
                            IconName::CircleX
                        } else {
                            IconName::CircleCheck
                        })
                        .size(12.0)
                        .color(if failed {
                            colors.red
                        } else {
                            colors.green
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .w(u(86.0))
                            .truncate()
                            .text_color(colors.text_dim)
                            .child(result.kind.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(11.5))
                            .child(result.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(if failed { colors.red } else { colors.text_dim })
                            .child(status),
                    ),
            );
            if failed
                && !compact
                && let Some(message) = result.message.clone().filter(|m| !m.is_empty())
            {
                section = section.child(
                    div()
                        .pl(u(20.0))
                        .text_size(u(11.5))
                        .text_color(colors.red)
                        .child(message),
                );
            }
        }
        if results.len() > limit {
            section = section.child(
                div()
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .child(format!("and {} more", results.len() - limit)),
            );
        }
        section.into_any_element()
    }

    fn tree_columns() -> Vec<ColumnDef> {
        vec![
            ColumnDef::new(
                "resource",
                "Resource",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 260.0,
                },
            ),
            ColumnDef::new("sync", "Sync", ColumnWidth::Fixed(104.0)),
            ColumnDef::new("health", "Health", ColumnWidth::Fixed(112.0)),
            ColumnDef::new(
                "info",
                "Info",
                ColumnWidth::Flex {
                    weight: 0.55,
                    min: 120.0,
                },
            ),
            ColumnDef::new("age", "Age", ColumnWidth::Fixed(56.0)),
        ]
    }

    fn render_resources(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let api = self.api_connected(cx);
        let segment = |id: &'static str,
                       icon: IconName,
                       label: &'static str,
                       on: bool,
                       mode: TreeMode,
                       cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .h(u(22.0))
                .px(u(8.0))
                .gap(u(5.0))
                .text_size(u(12.0))
                .cursor_pointer()
                .when(on, |this| {
                    this.bg(colors.chip_selected_background)
                        .text_color(colors.chip_selected_text)
                })
                .when(!on, |this| this.text_color(colors.text_dim))
                .child(Icon::new(icon).size(12.0))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tree_mode = mode;
                    this.rebuild_rows(cx);
                }))
        };
        let modes = h_flex()
            .flex_none()
            .rounded(u(5.0))
            .border_1()
            .border_color(colors.border)
            .overflow_hidden()
            .child(segment(
                "argo-tree-mode",
                IconName::ListTree,
                "Tree",
                self.tree_mode == TreeMode::Tree,
                TreeMode::Tree,
                cx,
            ))
            .child(segment(
                "argo-list-mode",
                IconName::List,
                "List",
                self.tree_mode == TreeMode::List,
                TreeMode::List,
                cx,
            ));
        let out_of_sync = self
            .nodes
            .iter()
            .filter(|n| n.sync == Some(SyncStatus::OutOfSync) || n.requires_pruning)
            .count();
        let unhealthy = self.nodes.iter().filter(|n| n.unhealthy()).count();
        let total = self.nodes.len().saturating_sub(1);
        let show_chip = |id: &'static str,
                         label: String,
                         dot: Option<gpui::Hsla>,
                         show: Show,
                         cx: &mut Context<Self>| {
            let mut chip = Chip::new(label).selected(self.show == show);
            if let Some(dot) = dot {
                chip = chip.dot(dot);
            }
            div()
                .id(id)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show = show;
                    this.rebuild_rows(cx);
                }))
                .child(chip)
        };
        let filter_focused = self
            .tree_filter
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let note = if api {
            "full tree from the Argo CD API".to_string()
        } else {
            "children from Kubyl's watches".to_string()
        };
        let toolbar = h_flex()
            .flex_none()
            .h(u(38.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(modes)
            .child(show_chip(
                "argo-show-all",
                format!("All {total}"),
                None,
                Show::All,
                cx,
            ))
            .child(show_chip(
                "argo-show-oos",
                format!("Out of sync {out_of_sync}"),
                Some(colors.yellow),
                Show::OutOfSync,
                cx,
            ))
            .child(show_chip(
                "argo-show-bad",
                format!("Unhealthy {unhealthy}"),
                Some(colors.red),
                Show::Unhealthy,
                cx,
            ))
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap(u(4.0))
                    .text_size(u(11.5))
                    .text_color(colors.text_dim)
                    .whitespace_nowrap()
                    .when(!api, |this| this.child(Chip::new("live")))
                    .child(note),
            )
            .child(
                div()
                    .flex_none()
                    .w(u(160.0))
                    .h(u(24.0))
                    .px(u(8.0))
                    .flex()
                    .items_center()
                    .gap(u(6.0))
                    .rounded(u(5.0))
                    .bg(colors.input_background)
                    .border_1()
                    .border_color(if filter_focused {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .text_size(u(12.0))
                    .child(Icon::new(IconName::Funnel).size(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.tree_filter).appearance(false)),
                    ),
            );
        let columns = Self::tree_columns();
        let body = if self.rows.is_empty() {
            let message = if self.app.is_none() {
                "Loading…".to_string()
            } else if self.nodes.len() <= 1 {
                "The application has no resources yet.".to_string()
            } else {
                "No resources match.".to_string()
            };
            widgets::empty(message, &colors)
        } else {
            uniform_list(
                "argo-tree-rows",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_tree_rows(range, cx)),
            )
            .flex_1()
            .track_scroll(&self.tree_scroll)
            .into_any_element()
        };
        let hints: Vec<(SharedString, SharedString)> = vec![
            ("enter".into(), "Open in Kubyl".into()),
            ("l".into(), "Logs".into()),
            ("e".into(), "Edit YAML".into()),
            ("d".into(), "Diff".into()),
            ("t".into(), "Tree / list".into()),
            ("s".into(), "Sync…".into()),
            ("r".into(), "Refresh".into()),
        ];
        let table = v_flex()
            .id("argo-tree")
            .key_context(TABLE)
            .key_context(TREE_CONTEXT)
            .track_focus(&self.focus)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .on_action(cx.listener(|this, _: &nav::ExpandNode, _, cx| {
                if let Some((_, node)) = this.selected_row() {
                    let id = node.id.clone();
                    if this.collapsed.remove(&id) {
                        this.rebuild_rows(cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &nav::CollapseNode, _, cx| {
                if let Some((index, node)) = this.selected_row() {
                    let (id, depth, has_children) =
                        (node.id.clone(), node.depth, node.has_children);
                    if has_children && !this.collapsed.contains(&id) {
                        this.collapsed.insert(id);
                        this.rebuild_rows(cx);
                    } else if let Some(parent) =
                        (0..index).rev().find(|i| this.rows[*i].depth < depth)
                    {
                        this.select_node(Some(parent), cx);
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &NodeLogs, window, cx| {
                this.open_node(Some(ViewKind::Logs), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NodeYaml, window, cx| {
                this.open_node(Some(ViewKind::Yaml), window, cx)
            }))
            .on_action(cx.listener(|this, _: &NodeDiff, _, cx| this.node_diff(cx)))
            .on_action(cx.listener(|this, _: &ToggleTreeMode, _, cx| {
                this.tree_mode = match this.tree_mode {
                    TreeMode::Tree => TreeMode::List,
                    TreeMode::List => TreeMode::Tree,
                };
                this.rebuild_rows(cx);
            }))
            .child(widgets::header(&columns, &colors))
            .child(body)
            .child(KeyHints::new(hints));
        v_flex()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(table)
                    .child(
                        div()
                            .id("argo-summary-column")
                            .flex_none()
                            .w(u(330.0))
                            .h_full()
                            .overflow_y_scroll()
                            .border_l_1()
                            .border_color(colors.border)
                            .bg(colors.panel)
                            .child(self.render_summary_column(true, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_tree_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let columns = Self::tree_columns();
        let selected = self.selected_row().map(|(i, _)| i);
        let tree = self.tree_mode == TreeMode::Tree;
        let discovery =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).discovery(self.cluster()));
        range
            .filter_map(|index| {
                let node = self.rows.get(index)?.clone();
                let collapsed = self.collapsed.contains(&node.id);
                let plural = discovery
                    .as_ref()
                    .and_then(|d| {
                        d.preferred()
                            .find(|r| r.gvk.group == node.group && r.gvk.kind == node.kind)
                    })
                    .map(|r| r.gvr.resource.clone())
                    .unwrap_or_default();
                let icon = if node.is_app() {
                    IconName::Layers
                } else {
                    kubyl_explorer::catalog::icon_for(&node.group, &plural)
                };
                let id = node.id.clone();
                let resource = h_flex()
                    .h_full()
                    .min_w_0()
                    .gap(u(6.0))
                    .when(tree, |this| {
                        this.children((0..node.depth).map(|_| {
                            div()
                                .flex_none()
                                .w(u(10.0))
                                .ml(u(6.0))
                                .h_full()
                                .border_l_1()
                                .border_color(colors.border_variant)
                        }))
                    })
                    .child(if tree && node.has_children {
                        div()
                            .id(SharedString::from(format!("argo-node-toggle-{index}")))
                            .flex_none()
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_node(&id, cx)
                            }))
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .size(11.0)
                                .color(colors.text_dim),
                            )
                            .into_any_element()
                    } else {
                        div().flex_none().w(u(11.0)).into_any_element()
                    })
                    .child(Icon::new(icon).size(13.0).color(colors.text_dim))
                    .child(
                        div()
                            .flex_none()
                            .text_size(u(12.0))
                            .text_color(colors.text_dim)
                            .child(node.kind.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(fonts::MONO)
                            .text_size(u(12.0))
                            .when(!node.exists && !node.is_app(), |this| {
                                this.text_color(colors.text_dim).line_through()
                            })
                            .child(node.name.clone()),
                    )
                    .when(node.live, |this| this.child(Chip::new("live")))
                    .when(node.requires_pruning, |this| {
                        this.child(Chip::new("prune").text_color(colors.yellow))
                    })
                    .when(node.hook, |this| this.child(Chip::new("hook")));
                let sync = match node.sync {
                    Some(sync) if node.requires_pruning => {
                        widgets::pill("Prune", widgets::sync_color(sync, &colors))
                            .into_any_element()
                    }
                    Some(sync) => widgets::sync_pill(sync, &colors).into_any_element(),
                    None => div()
                        .text_color(colors.text_faint)
                        .child("—")
                        .into_any_element(),
                };
                let health = match &node.health {
                    Some((health, message)) => {
                        let message = message.clone();
                        div()
                            .id(SharedString::from(format!("argo-node-health-{index}")))
                            .when_some(message.filter(|m| !m.is_empty()), |this, message| {
                                this.tooltip(move |window, cx| {
                                    gpui_component::tooltip::Tooltip::new(message.clone())
                                        .build(window, cx)
                                })
                            })
                            .child(widgets::health_pill(*health, &colors))
                            .into_any_element()
                    }
                    None => div()
                        .text_color(colors.text_faint)
                        .child("—")
                        .into_any_element(),
                };
                let cells: Vec<AnyElement> = vec![
                    resource.into_any_element(),
                    sync,
                    health,
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(colors.text_muted)
                        .child(node.info.clone())
                        .into_any_element(),
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(widgets::age(node.created.as_deref()))
                        .into_any_element(),
                ];
                let row = widgets::row(
                    ("argo-node", index),
                    selected == Some(index),
                    ROW_HEIGHT,
                    &colors,
                )
                .children(
                    columns
                        .iter()
                        .zip(cells)
                        .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                )
                .on_click(cx.listener(
                    move |this, event: &gpui::ClickEvent, window, cx| {
                        this.focus.focus(window, cx);
                        this.select_node(Some(index), cx);
                        if event.click_count() == 2 {
                            this.open_node(None, window, cx);
                        }
                    },
                ));
                Some(row.into_any_element())
            })
            .collect()
    }

    fn render_summary(&mut self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("argo-summary")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(u(760.0))
                    .child(self.render_summary_column(false, cx)),
            )
            .into_any_element()
    }

    fn render_diff(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(app) = self.app.clone() else {
            return widgets::empty("Loading…", &colors);
        };
        if !self.api_connected(cx) {
            let out_of_sync = app.out_of_sync();
            let cluster = self.cluster().clone();
            let mut list = v_flex().gap(u(4.0)).max_w(u(640.0));
            for resource in &out_of_sync {
                list = list.child(
                    h_flex()
                        .gap(u(8.0))
                        .text_size(u(12.0))
                        .child(
                            div()
                                .flex_none()
                                .w(u(130.0))
                                .text_color(colors.text_dim)
                                .child(resource.kind.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .child(resource.name.clone()),
                        )
                        .child(widgets::sync_pill(SyncStatus::OutOfSync, &colors)),
                );
            }
            return v_flex()
                .p(u(18.0))
                .gap(u(12.0))
                .child(
                    div().text_size(u(13.0)).child(match out_of_sync.len() {
                        0 => "Every resource is in sync.".to_string(),
                        1 => "1 resource is out of sync.".to_string(),
                        n => format!("{n} resources are out of sync."),
                    }),
                )
                .child(list)
                .child(
                    div()
                        .max_w(u(640.0))
                        .text_size(u(12.5))
                        .text_color(colors.text_dim)
                        .child("The diff of desired and live state needs API mode: Kubernetes mode only sees the Application object, not the manifests Argo CD renders. Sign in to Argo CD to see it."),
                )
                .child(
                    h_flex().child(
                        Button::new("argo-diff-sign-in")
                            .primary()
                            .icon(IconName::Key)
                            .label("Sign in to Argo CD…")
                            .on_click(move |_, window, cx| dialogs::open_sign_in(cluster.clone(), window, cx)),
                    ),
                )
                .into_any_element();
        }
        let diffs = match &self.diffs {
            None => return widgets::empty("Comparing desired and live state…", &colors),
            Some(Err(err)) => {
                return widgets::empty(format!("Couldn't load the diff: {err}"), &colors);
            }
            Some(Ok(diffs)) => diffs.clone(),
        };
        if diffs.is_empty() {
            return widgets::empty("Desired and live state match.", &colors);
        }
        let selected = self.diff_selected.min(diffs.len() - 1);
        let side_by_side = self.side_by_side;
        let list = v_flex()
            .id("argo-diff-list")
            .flex_none()
            .w(u(300.0))
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(colors.border_variant)
            .children(diffs.iter().enumerate().map(|(index, (item, diff))| {
                widgets::row(("argo-diff-item", index), index == selected, 40.0, &colors)
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .text_size(u(11.5))
                                    .text_color(colors.text_dim)
                                    .child(item.kind.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family(fonts::MONO)
                                    .text_size(u(12.0))
                                    .child(item.name.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap(u(4.0))
                            .font_family(fonts::MONO)
                            .text_size(u(11.0))
                            .child(
                                div()
                                    .text_color(colors.green)
                                    .child(format!("+{}", diff.added)),
                            )
                            .child(
                                div()
                                    .text_color(colors.red)
                                    .child(format!("−{}", diff.removed)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.focus.focus(window, cx);
                        this.diff_selected = index;
                        cx.notify();
                    }))
            }));
        let (item, diff) = &diffs[selected];
        let secret = crate::diff::is_secret(item);
        let toolbar = h_flex()
            .flex_none()
            .h(u(34.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(12.0))
            .child(div().text_color(colors.text_dim).child("live"))
            .child(
                Icon::new(IconName::ArrowRight)
                    .size(11.0)
                    .color(colors.text_dim),
            )
            .child(div().text_color(colors.text_dim).child("desired"))
            .child(
                div()
                    .font_family(fonts::MONO)
                    .text_size(u(11.5))
                    .child(format!("{} {}/{}", item.kind, item.namespace, item.name)),
            )
            .when(secret, |this| this.child(Chip::new("values masked")))
            .child(div().flex_1())
            .child(
                IconButton::new("argo-diff-sbs", IconName::Columns)
                    .icon_size(13.0)
                    .toggled(side_by_side)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.side_by_side = !this.side_by_side;
                        cx.notify();
                    })),
            );
        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(list)
            .child(
                v_flex().flex_1().min_w_0().child(toolbar).child(
                    div()
                        .id("argo-diff-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_scroll()
                        .py(u(6.0))
                        .child(kubyl_yaml::diff_view(diff, side_by_side, &colors)),
                ),
            )
            .into_any_element()
    }

    fn history_columns() -> Vec<ColumnDef> {
        vec![
            ColumnDef::new("id", "ID", ColumnWidth::Fixed(54.0)),
            ColumnDef::new("revision", "Revision", ColumnWidth::Fixed(150.0)),
            ColumnDef::new(
                "source",
                "Source",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 200.0,
                },
            ),
            ColumnDef::new("deployed", "Deployed", ColumnWidth::Fixed(100.0)),
            ColumnDef::new("by", "By", ColumnWidth::Fixed(150.0)),
            ColumnDef::new("action", "", ColumnWidth::Fixed(118.0)),
        ]
    }

    fn render_history(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let history = self.history();
        let limit = self
            .app
            .as_ref()
            .and_then(|a| a.spec.revision_history_limit)
            .unwrap_or(10);
        let info = h_flex()
            .flex_none()
            .h(u(38.0))
            .px(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .text_size(u(12.0))
            .text_color(colors.text_dim)
            .child(Icon::new(IconName::History).size(13.0))
            .child(format!(
                "{} deployment{}, newest first · revisions link to their commit",
                history.len(),
                if history.len() == 1 { "" } else { "s" }
            ))
            .child(div().flex_1())
            .child(format!("history limit {limit} (spec.revisionHistoryLimit)"));
        let body = if history.is_empty() {
            widgets::empty(
                "No deployments yet: the history grows with every sync.",
                &colors,
            )
        } else {
            uniform_list(
                "argo-history-rows",
                history.len(),
                cx.processor(|this, range: Range<usize>, _, cx| {
                    this.render_history_rows(range, cx)
                }),
            )
            .flex_1()
            .track_scroll(&self.history_scroll)
            .into_any_element()
        };
        let mut hints: Vec<(SharedString, SharedString)> = vec![
            ("b".into(), "Rollback…".into()),
            ("enter".into(), "Open commit".into()),
        ];
        hints.push(("s".into(), "Sync…".into()));
        v_flex()
            .flex_1()
            .min_h_0()
            .child(info)
            .child(
                v_flex()
                    .id("argo-history")
                    .key_context(TABLE)
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_h_0()
                    .child(widgets::header(&Self::history_columns(), &colors))
                    .child(body),
            )
            .child(KeyHints::new(hints))
            .into_any_element()
    }

    fn render_history_rows(
        &mut self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = cx.colors().clone();
        let history = self.history();
        let current = self.app.as_ref().and_then(|a| a.current_history_id());
        let writable = self.writable(cx);
        let columns = Self::history_columns();
        range
            .filter_map(|index| {
                let entry = history.get(index)?.clone();
                let is_current = Some(entry.id) == current;
                let sources = entry.all_sources();
                let revisions = entry.all_revisions();
                let first_source = sources.first().cloned();
                let revision_cell = match (revisions.first(), &first_source) {
                    (Some(revision), Some(source)) => {
                        let short = short_revision(revision);
                        let extra =
                            (revisions.len() > 1).then(|| format!("+{}", revisions.len() - 1));
                        let link = links::commit_url(&source.repo_url, revision);
                        h_flex()
                            .gap(u(6.0))
                            .child(
                                Icon::new(IconName::GitCommit)
                                    .size(13.0)
                                    .color(colors.text_dim),
                            )
                            .child(match link {
                                Some(url) => {
                                    widgets::url_link(("argo-hist-rev", index), short, url, &colors)
                                        .into_any_element()
                                }
                                None => widgets::mono(short),
                            })
                            .when_some(extra, |this, extra| {
                                this.child(
                                    div()
                                        .text_color(colors.text_dim)
                                        .text_size(u(11.5))
                                        .child(extra),
                                )
                            })
                            .into_any_element()
                    }
                    (Some(revision), None) => widgets::mono(short_revision(revision)),
                    _ => div()
                        .text_color(colors.text_faint)
                        .child("—")
                        .into_any_element(),
                };
                let source_cell = match &first_source {
                    Some(source) => h_flex()
                        .min_w_0()
                        .gap(u(4.0))
                        .text_size(u(12.0))
                        .child(
                            div()
                                .flex_none()
                                .text_color(colors.text_dim)
                                .child(source.repo_short()),
                        )
                        .child(
                            div()
                                .truncate()
                                .font_family(fonts::MONO)
                                .text_size(u(11.5))
                                .child(source.what()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(colors.text_dim)
                                .child(format!("@ {}", source.target())),
                        )
                        .when(sources.len() > 1, |this| {
                            this.child(Chip::new(format!("{} sources", sources.len())))
                        })
                        .into_any_element(),
                    None => div()
                        .text_color(colors.text_faint)
                        .child("—")
                        .into_any_element(),
                };
                let by = entry
                    .initiated_by
                    .clone()
                    .map(|i| i.label())
                    .unwrap_or_default();
                let automated = entry
                    .initiated_by
                    .as_ref()
                    .is_some_and(|i| i.automated && i.username.is_none());
                let target = self.target.clone();
                let id = entry.id;
                let action = if is_current {
                    Chip::new("current").selected(true).into_any_element()
                } else if writable {
                    Button::new(SharedString::from(format!("argo-rollback-{}", entry.id)))
                        .ghost()
                        .icon(IconName::RotateCcw)
                        .label("Roll back…")
                        .disabled(!entry.can_roll_back())
                        .on_click(move |_, window, cx| {
                            dialogs::open_rollback(target.clone(), Some(id), window, cx)
                        })
                        .into_any_element()
                } else {
                    div().into_any_element()
                };
                let cells: Vec<AnyElement> = vec![
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(12.0))
                        .text_color(if is_current {
                            colors.accent
                        } else {
                            colors.text_dim
                        })
                        .child(format!("#{}", entry.id))
                        .into_any_element(),
                    revision_cell,
                    source_cell,
                    div()
                        .font_family(fonts::MONO)
                        .text_size(u(11.5))
                        .text_color(colors.text_muted)
                        .child(format!("{} ago", widgets::age_of(entry.deployed())))
                        .into_any_element(),
                    div()
                        .truncate()
                        .text_size(u(12.0))
                        .text_color(if automated {
                            colors.text_dim
                        } else {
                            colors.text
                        })
                        .child(by)
                        .into_any_element(),
                    action,
                ];
                let selected = self.history_selected == Some(entry.id);
                Some(
                    widgets::row(("argo-history-row", index), selected, 34.0, &colors)
                        .children(
                            columns
                                .iter()
                                .zip(cells)
                                .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.focus.focus(window, cx);
                            this.history_selected = Some(id);
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_events(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(events) = &self.events else {
            return widgets::empty("Loading events…", &colors);
        };
        let store = events.read(cx);
        let mut rows: Vec<Arc<Value>> = store.objects().values().cloned().collect();
        if rows.is_empty() {
            let message = if store.status().is_settled() {
                "No events for this application (Kubernetes keeps them for an hour)."
            } else {
                "Loading events…"
            };
            return widgets::empty(message, &colors);
        }
        rows.sort_by_key(|e| std::cmp::Reverse(kubyl_resources::columns::event_time(e)));
        let columns = vec![
            ColumnDef::new("type", "Type", ColumnWidth::Fixed(96.0)),
            ColumnDef::new("reason", "Reason", ColumnWidth::Fixed(170.0)),
            ColumnDef::new(
                "message",
                "Message",
                ColumnWidth::Flex {
                    weight: 1.0,
                    min: 240.0,
                },
            ),
            ColumnDef::new("count", "Count", ColumnWidth::Fixed(60.0)),
            ColumnDef::new("age", "Last seen", ColumnWidth::Fixed(80.0)),
        ];
        let now = jiff::Timestamp::now();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(widgets::header(&columns, &colors))
            .child(
                div()
                    .id("argo-events")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows.iter().enumerate().map(|(index, event)| {
                        let warning = event["type"].as_str() == Some("Warning");
                        let count = event["count"]
                            .as_i64()
                            .or_else(|| event.pointer("/series/count").and_then(Value::as_i64))
                            .unwrap_or(1);
                        let age = kubyl_resources::columns::event_time(event)
                            .map(|t| {
                                kubyl_resources::format::human_duration(
                                    kubyl_resources::format::seconds_since(t, now),
                                )
                            })
                            .unwrap_or_default();
                        let cells: Vec<AnyElement> = vec![
                            widgets::pill(
                                if warning { "Warning" } else { "Normal" },
                                if warning {
                                    colors.yellow
                                } else {
                                    colors.text_dim
                                },
                            )
                            .into_any_element(),
                            div()
                                .truncate()
                                .text_color(if warning { colors.yellow } else { colors.text })
                                .child(event["reason"].as_str().unwrap_or_default().to_string())
                                .into_any_element(),
                            div()
                                .truncate()
                                .text_size(u(12.0))
                                .child(kubyl_resources::columns::event_message(event).to_string())
                                .into_any_element(),
                            widgets::mono(count.to_string()),
                            widgets::mono(age),
                        ];
                        widgets::row(("argo-event", index), false, 30.0, &colors).children(
                            columns
                                .iter()
                                .zip(cells)
                                .map(|(def, cell)| widgets::column_cell(def).child(cell)),
                        )
                    })),
            )
            .into_any_element()
    }

    fn render_logs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        let Some(app) = self.app.clone() else {
            return widgets::empty("Loading…", &colors);
        };
        let install = ArgoCd::try_global(cx).and_then(|a| {
            a.read(cx).install_for(
                self.cluster(),
                app.status
                    .controller_namespace
                    .as_deref()
                    .or(Some(app.namespace())),
            )
        });
        let controller = install.as_ref().and_then(|i| {
            i.controller.as_ref().map(|c| {
                format!(
                    "{} {}/{}",
                    c.resource.trim_end_matches('s'),
                    i.namespace,
                    c.name
                )
            })
        });
        let query = controller_query(&app);
        v_flex()
            .p(u(18.0))
            .gap(u(12.0))
            .max_w(u(720.0))
            .child(div().text_size(u(13.0)).child("The application controller's logs, filtered to the lines about this application."))
            .child(widgets::kv(
                vec![
                    ("Controller", widgets::mono(controller.clone().unwrap_or_else(|| "not found".into()))),
                    ("Filter", widgets::mono(query)),
                ],
                &colors,
            ))
            .child(
                h_flex().child(
                    Button::new("argo-open-logs")
                        .primary()
                        .icon(IconName::Terminal)
                        .label("Open controller logs")
                        .disabled(controller.is_none())
                        .on_click(cx.listener(|this, _, window, cx| this.open_controller_logs(window, cx))),
                ),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(colors.text_dim)
                    .child("The logs open in their own tab, with the filter in the search box (lines of every controller replica)."),
            )
            .into_any_element()
    }
}

impl Focusable for AppView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for AppView {
    fn tab_title(&self, _: &App) -> SharedString {
        self.name().to_string().into()
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(IconName::Layers.path())
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        Some(ViewRequest::for_resource(
            ViewKind::Custom(VIEW_KIND.into()),
            self.target.clone(),
        ))
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let body = if self.gone {
            widgets::empty(
                format!("The application {} was deleted.", self.name()),
                &colors,
            )
        } else if self.app.is_none() {
            widgets::empty(
                match self.own.read(cx).status() {
                    kubyl_resources::StoreStatus::Forbidden => {
                        "You may not read this application.".to_string()
                    }
                    _ => "Loading…".to_string(),
                },
                &colors,
            )
        } else {
            match self.tab {
                Tab::Summary => self.render_summary(cx),
                Tab::Resources => self.render_resources(window, cx),
                Tab::Diff => self.render_diff(cx),
                Tab::History => self.render_history(cx),
                Tab::Events => self.render_events(cx),
                Tab::Logs => self.render_logs(cx),
            }
        };
        let header = self.render_header(cx);
        let tabs = self.render_subtabs(cx);
        let target = self.target.clone();
        v_flex()
            .key_context(APP_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .font_family(fonts::UI)
            .text_size(u(sizes::UI_FONT))
            // The app's own actions act on this app, whatever is selected in its tree.
            .on_action({
                let target = target.clone();
                move |_: &actions::Sync, window, cx| dialogs::open_sync(target.clone(), window, cx)
            })
            .on_action(cx.listener(|this, _: &actions::Refresh, _, cx| {
                this.run(Op::Refresh { hard: false }, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::HardRefresh, _, cx| {
                this.run(Op::Refresh { hard: true }, cx)
            }))
            .on_action(
                cx.listener(|this, _: &actions::Terminate, _, cx| this.run(Op::Terminate, cx)),
            )
            .on_action({
                let target = target.clone();
                move |_: &actions::ToggleAutoSync, _, cx| {
                    actions::toggle_auto_sync(target.clone(), cx)
                }
            })
            .on_action(cx.listener(|this, _: &actions::Rollback, window, cx| {
                let id = this.history_selected.filter(|_| this.tab == Tab::History);
                dialogs::open_rollback(this.target.clone(), id, window, cx)
            }))
            .on_action({
                let target = target.clone();
                move |_: &actions::Delete, window, cx| {
                    dialogs::open_delete(target.clone(), window, cx)
                }
            })
            .on_action(
                cx.listener(|this, _: &actions::ShowHistory, _, cx| this.set_tab(Tab::History, cx)),
            )
            .on_action(cx.listener(|this, _: &actions::OpenApplication, _, cx| {
                this.set_tab(Tab::Summary, cx)
            }))
            .on_action({
                let target = target.clone();
                move |_: &actions::EditApplication, window, cx| {
                    window.dispatch_action(
                        Box::new(OpenView(ViewRequest::for_resource(
                            ViewKind::Yaml,
                            target.clone(),
                        ))),
                        cx,
                    )
                }
            })
            .on_action(cx.listener(|this, _: &super::FocusFilter, window, cx| {
                if this.tab == Tab::Resources {
                    let focus = this.tree_filter.read(cx).focus_handle(cx);
                    focus.focus(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &nav::SelectNext, _, cx| this.move_selection(Move::Next, cx)),
            )
            .on_action(cx.listener(|this, _: &nav::SelectPrevious, _, cx| {
                this.move_selection(Move::Previous, cx)
            }))
            .on_action(
                cx.listener(|this, _: &nav::SelectFirst, _, cx| {
                    this.move_selection(Move::First, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &nav::SelectLast, _, cx| this.move_selection(Move::Last, cx)),
            )
            .on_action(cx.listener(|this, _: &nav::SelectPageDown, _, cx| {
                this.move_selection(Move::PageDown, cx)
            }))
            .on_action(cx.listener(|this, _: &nav::SelectPageUp, _, cx| {
                this.move_selection(Move::PageUp, cx)
            }))
            .on_action(cx.listener(|this, _: &nav::Confirm, window, cx| this.confirm(window, cx)))
            .child(header)
            .child(tabs)
            .child(v_flex().flex_1().min_h_0().child(body))
            .when(
                self.tab != Tab::Resources && self.tab != Tab::History,
                |this| {
                    let hints = ActionRegistry::global(cx).hints(APP_CONTEXT);
                    this.child(KeyHints::new(hints))
                },
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_log_query_matches_both_formats() {
        let app = Application::parse(&crate::model::tests::guestbook()).unwrap();
        let query = controller_query(&app);
        let re = regex_lite(&query);
        assert!(re(
            r#"{"app-namespace":"argocd","application":"guestbook","level":"info"}"#
        ));
        assert!(re(
            r#"time="x" level=info msg="y" application=guestbook project=demo"#
        ));
        assert!(!re(r#"{"application":"guestbook-multi"}"#));
        assert!(!re("application=guestbook-multi"));
    }

    /// The query as kubyl_logs runs it.
    fn regex_lite(query: &str) -> impl Fn(&str) -> bool {
        let search = kubyl_logs::search::Search::new(query, true, true).unwrap();
        move |line: &str| search.matches(line)
    }
}
