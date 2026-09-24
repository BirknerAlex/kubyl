//! The resource list (board 1 · Pods): a live, virtualized table of any kind.
//!
//! A list has one or more *sources*, each a shared watch ([`kubyl_resources::ResourceStores`]):
//! - one cluster, one namespace (or all namespaces, or a cluster-scoped kind): one source;
//! - several namespaces: one cluster-wide source filtered client-side when the user may list
//!   across namespaces, else one source per namespace;
//! - the Favorites workspace: one source per favorite, across clusters, with a cluster column.
//!
//! Kinds with a hand-written column set watch full objects. Everything else watches metadata
//! only and gets its columns from the server-side `Table` (CRD printer columns), refetched at
//! most once per second while objects change.

mod rows;
mod table;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement, KeyBinding,
    KeyContext, Pixels, Render, SharedString, Subscription, Task, UniformListScrollHandle, Window,
    actions, prelude::*,
};
use gpui_component::input::{InputEvent, InputState};
use kubyl_core::{
    ActionRegistry, ActionSpec, ActiveContext, CellValue, ClusterId, ColumnDef, ColumnProvider,
    ColumnWidth, Gvr, ResourceColumns, ResourceRef, TabView, ViewKind, ViewRequest,
};
use kubyl_kube::access::AccessQuery;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_resources::table::ServerTable;
use kubyl_resources::{
    Filter, ResourceSelection, ResourceStores, Selected, StoreHandle, StoreKey, StoreMode,
    StoreStatus, format,
};
use kubyl_settings::{State, StateSection};
use kubyl_ui::IconName;
use serde::{Deserialize, Serialize};

use crate::catalog;
use crate::favorites::{self, Favorites};
pub use rows::{Row, RowId, SortKey, natural_cmp};

actions!(
    resource_list,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        SelectPageDown,
        SelectPageUp,
        ExtendNext,
        ExtendPrevious,
        ToggleMark,
        SelectAll,
        OpenSelected,
        FocusFilter,
        FocusTable,
        ToggleWide,
    ]
);

/// Key context of the table; resource actions bind in it (plus `kind == Pod` etc.).
pub const CONTEXT: &str = "ResourceList";
const FILTER_CONTEXT: &str = "ResourceFilter";
const PAGE: usize = 20;
/// Minimum time between two server-side table fetches of one source.
const TABLE_REFRESH: Duration = Duration::from_secs(1);

pub(crate) fn init(cx: &mut App) {
    let list = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, list),
        KeyBinding::new("j", SelectNext, list),
        KeyBinding::new("up", SelectPrevious, list),
        KeyBinding::new("k", SelectPrevious, list),
        KeyBinding::new("home", SelectFirst, list),
        KeyBinding::new("g g", SelectFirst, list),
        KeyBinding::new("end", SelectLast, list),
        KeyBinding::new("shift-g", SelectLast, list),
        KeyBinding::new("pagedown", SelectPageDown, list),
        KeyBinding::new("ctrl-f", SelectPageDown, list),
        KeyBinding::new("pageup", SelectPageUp, list),
        KeyBinding::new("ctrl-b", SelectPageUp, list),
        KeyBinding::new("shift-down", ExtendNext, list),
        KeyBinding::new("shift-j", ExtendNext, list),
        KeyBinding::new("shift-up", ExtendPrevious, list),
        KeyBinding::new("shift-k", ExtendPrevious, list),
        KeyBinding::new("enter", OpenSelected, list),
        KeyBinding::new("escape", FocusTable, Some(FILTER_CONTEXT)),
        KeyBinding::new("down", FocusTable, Some(FILTER_CONTEXT)),
    ]);
    // In the registry so the palette's `>` mode finds them (the registry also binds the keys).
    for (spec, keys) in [
        (
            ActionSpec::new("List: Filter", FocusFilter).hint("Filter"),
            "/",
        ),
        (ActionSpec::new("List: Mark Row", ToggleMark), "space"),
        (
            ActionSpec::new("List: Select All", SelectAll),
            "secondary-a",
        ),
        (
            ActionSpec::new("List: Toggle Wide Columns", ToggleWide),
            "ctrl-w",
        ),
    ] {
        ActionRegistry::register(cx, spec.bind(keys, list));
    }
}

/// Replaces the filter of the focused list (the palette's `/` mode).
#[derive(Clone, Debug, PartialEq, Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = resource_list)]
pub struct SetFilter {
    pub query: String,
}

/// Hidden columns and the wide toggle, per kind (`state.json` → `lists`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ListPrefs {
    hidden: Vec<String>,
    wide: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct ListsState {
    kinds: std::collections::BTreeMap<String, ListPrefs>,
}

impl StateSection for ListsState {
    const KEY: &'static str = "lists";
}

/// What one source watches.
#[derive(Clone, Debug, PartialEq)]
struct ScopeSpec {
    cluster: ClusterId,
    namespace: Option<String>,
    labels: Option<String>,
    /// Client-side namespace filter for a cluster-wide watch of several namespaces.
    only: Option<Vec<String>>,
    /// Favorite label shown in the cluster column.
    label: Option<SharedString>,
    mode: StoreMode,
}

#[derive(Default)]
struct TableFetch {
    in_flight: bool,
    dirty: bool,
    last: Option<Instant>,
    task: Option<Task<()>>,
}

struct Source {
    spec: ScopeSpec,
    /// The resolved resource on this cluster (version from discovery).
    gvr: Gvr,
    kind: Option<String>,
    store: StoreHandle,
    table: Option<Arc<ServerTable>>,
    table_error: Option<String>,
    fetch: TableFetch,
    _observe: Subscription,
}

/// How the list's columns are produced.
#[derive(Clone)]
enum Columns {
    Provider(Arc<dyn ColumnProvider>),
    Server,
}

/// Single cluster, or the Favorites workspace.
#[derive(Clone, Debug, PartialEq)]
enum Mode {
    Cluster(ClusterId),
    Favorites,
}

struct Resize {
    column: SharedString,
    start_x: Pixels,
    start_width: f32,
}

pub struct ResourceListView {
    mode: Mode,
    /// The requested resource (group + plural; the version is resolved per cluster).
    gvr: Gvr,
    kind: Option<String>,
    namespaced: bool,
    /// Selected namespaces (cluster mode). Empty: all namespaces.
    namespaces: Vec<String>,
    sources: Vec<Source>,
    columns_source: Columns,
    columns: Vec<ColumnDef>,
    rows: Vec<rows::Row>,
    failing: usize,
    sort: Option<(SharedString, bool)>,
    sort_cache: HashMap<RowId, (usize, SortKey)>,
    name_keys: HashMap<RowId, Arc<rows::NameKey>>,
    health: HashMap<RowId, (usize, bool)>,
    filter_input: Entity<InputState>,
    filter: Filter,
    filter_error: Option<String>,
    selected: Option<RowId>,
    selected_index: Option<usize>,
    marked: HashSet<RowId>,
    anchor: Option<usize>,
    prefs: ListPrefs,
    widths: HashMap<SharedString, f32>,
    resize: Option<Resize>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    rbac_requested: HashSet<AccessQuery>,
    /// The first row was selected automatically once.
    auto_selected: bool,
    _ticker: Task<()>,
    _subscriptions: Vec<Subscription>,
}

/// Opens the list of `gvr` in `cluster` (in `namespace`, else the active one) with `filter`
/// applied, e.g. a workload's label selector to show its pods.
pub fn open_filtered(
    cluster: ClusterId,
    gvr: Gvr,
    namespace: Option<String>,
    filter: String,
    window: &mut Window,
    cx: &mut App,
) {
    cx.set_global(PendingFilter(Some((cluster.clone(), gvr.clone(), filter))));
    window.dispatch_action(
        Box::new(kubyl_core::actions::OpenView(ViewRequest::for_resource(
            ViewKind::Table,
            ResourceRef::list(cluster, gvr, namespace),
        ))),
        cx,
    );
}

/// A filter to apply to the next list that opens for `(cluster, gvr)` (favorites with a label
/// selector).
#[derive(Default)]
pub(crate) struct PendingFilter(pub Option<(ClusterId, Gvr, String)>);

impl gpui::Global for PendingFilter {}

impl ResourceListView {
    /// A list of `target.gvr` in `target.cluster`. `target.namespace` preselects a namespace;
    /// otherwise the active namespace is used when this is the active cluster.
    pub fn new(target: ResourceRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let namespaces = match &target.namespace {
            Some(ns) => vec![ns.clone()],
            None => {
                let active = ActiveContext::global(cx);
                if active.cluster.as_ref().map(|c| &c.id) == Some(&target.cluster) {
                    active.namespace.iter().map(|n| n.to_string()).collect()
                } else {
                    Vec::new()
                }
            }
        };
        let mut this = Self::build(
            Mode::Cluster(target.cluster.clone()),
            target.gvr,
            window,
            cx,
        );
        this.namespaces = namespaces;
        let pending = cx
            .try_global::<PendingFilter>()
            .and_then(|p| p.0.clone())
            .filter(|(c, g, _)| c == &target.cluster && g.resource == this.gvr.resource);
        if let Some((_, _, filter)) = pending {
            cx.set_global(PendingFilter(None));
            this.filter_input
                .update(cx, |input, cx| input.set_value(filter.clone(), window, cx));
            this.set_filter(&filter);
        }
        this.resolve_kind(cx);
        this.sync_sources(window, cx);
        this
    }

    /// The Favorites workspace: `gvr` in every favorite, merged, with a cluster column.
    pub fn favorites(gvr: Gvr, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self::build(Mode::Favorites, gvr, window, cx);
        let favorites = Favorites::global(cx);
        this._subscriptions
            .push(cx.observe_in(&favorites, window, |this, _, window, cx| {
                this.sync_sources(window, cx)
            }));
        this.resolve_kind(cx);
        this.sync_sources(window, cx);
        this
    }

    fn build(mode: Mode, gvr: Gvr, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter"));
        let mut subscriptions = vec![
            cx.subscribe_in(
                &filter_input,
                window,
                |this, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        this.set_filter(&text);
                        this.refresh_rows(window, cx);
                    }
                    InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                    _ => {}
                },
            ),
            cx.observe_global_in::<ActiveContext>(window, |this, window, cx| {
                this.active_context_changed(window, cx)
            }),
        ];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(cx.subscribe_in(
                &manager,
                window,
                |this, _, event: &ConnectionEvent, window, cx| {
                    this.connection_event(event, window, cx)
                },
            ));
        }
        let focus = cx.focus_handle();
        subscriptions
            .push(cx.on_focus_in(&focus, window, |this, _, cx| this.publish_selection(cx)));

        let ticker = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        let prefs = State::get::<ListsState>(cx)
            .kinds
            .get(&prefs_key(&gvr))
            .cloned()
            .unwrap_or_default();
        let default_sort = if gvr.resource == "events" {
            Some(("last_seen".into(), true))
        } else {
            None
        };
        Self {
            mode,
            gvr,
            kind: None,
            namespaced: true,
            namespaces: Vec::new(),
            sources: Vec::new(),
            columns_source: Columns::Server,
            columns: Vec::new(),
            rows: Vec::new(),
            failing: 0,
            sort: default_sort,
            sort_cache: HashMap::new(),
            name_keys: HashMap::new(),
            health: HashMap::new(),
            filter_input,
            filter: Filter::default(),
            filter_error: None,
            selected: None,
            selected_index: None,
            marked: HashSet::new(),
            anchor: None,
            prefs,
            widths: HashMap::new(),
            resize: None,
            scroll: UniformListScrollHandle::new(),
            focus,
            rbac_requested: HashSet::new(),
            auto_selected: false,
            _ticker: ticker,
            _subscriptions: subscriptions,
        }
    }

    fn cluster(&self) -> Option<&ClusterId> {
        match &self.mode {
            Mode::Cluster(id) => Some(id),
            Mode::Favorites => None,
        }
    }

    /// Looks the kind up in discovery (any source cluster) and picks the column provider.
    fn resolve_kind(&mut self, cx: &mut Context<Self>) {
        let clusters: Vec<ClusterId> = match &self.mode {
            Mode::Cluster(id) => vec![id.clone()],
            Mode::Favorites => self
                .favorite_scopes(cx)
                .into_iter()
                .map(|s| s.cluster)
                .collect(),
        };
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let info = clusters.iter().find_map(|id| {
            let discovery = manager.read(cx).discovery(id)?;
            catalog::find(&discovery, &self.gvr.group, &self.gvr.resource).cloned()
        });
        if let Some(info) = info {
            self.kind = Some(info.gvk.kind.clone());
            self.namespaced = info.namespaced;
            self.columns_source = match ResourceColumns::get(cx, &info.gvk) {
                Some(provider) => Columns::Provider(provider),
                None => Columns::Server,
            };
            self.update_columns();
        }
    }

    fn update_columns(&mut self) {
        let base = match &self.columns_source {
            Columns::Provider(provider) => provider.columns(),
            Columns::Server => self
                .sources
                .iter()
                .find_map(|s| s.table.as_ref())
                .map(|t| t.column_defs())
                .unwrap_or_else(|| {
                    vec![
                        ColumnDef::new(
                            "name",
                            "Name",
                            ColumnWidth::Flex {
                                weight: 1.0,
                                min: 180.0,
                            },
                        )
                        .mono(),
                        ColumnDef::new("age", "Age", ColumnWidth::Fixed(60.0)).mono(),
                    ]
                }),
        };
        let mut columns = Vec::with_capacity(base.len() + 2);
        let name_at = base.iter().position(|c| c.id.as_ref() == "name");
        let mut injected = Vec::new();
        if self.mode == Mode::Favorites {
            injected.push(ColumnDef::new(
                "cluster",
                "Cluster",
                ColumnWidth::Fixed(150.0),
            ));
        }
        if self.namespaced && (self.mode == Mode::Favorites || self.namespaces.len() != 1) {
            injected
                .push(ColumnDef::new("namespace", "Namespace", ColumnWidth::Fixed(130.0)).mono());
        }
        let insert_at = match name_at {
            Some(ix) if base[ix].wide => 0,
            Some(ix) => ix + 1,
            None => 0,
        };
        for (ix, column) in base.into_iter().enumerate() {
            if ix == insert_at {
                columns.append(&mut injected);
            }
            columns.push(column);
        }
        columns.append(&mut injected);
        self.columns = columns;
    }

    fn favorite_scopes(&self, cx: &App) -> Vec<ScopeSpec> {
        let favorites = Favorites::global(cx);
        let favorites = favorites.read(cx);
        favorites
            .items()
            .iter()
            .enumerate()
            .filter(|(ix, _)| !favorites.is_excluded_from_workspace(*ix))
            .filter_map(|(_, favorite)| {
                Some(ScopeSpec {
                    cluster: favorites::cluster_of(favorite, cx)?,
                    namespace: Some(favorite.namespace.clone()),
                    labels: favorite.selector.clone(),
                    only: None,
                    label: favorite.alias.clone().map(SharedString::from),
                    mode: StoreMode::Full,
                })
            })
            .collect()
    }

    fn desired_scopes(&mut self, cx: &mut Context<Self>) -> Vec<ScopeSpec> {
        let mode = match self.columns_source {
            Columns::Provider(_) => StoreMode::Full,
            Columns::Server => StoreMode::Metadata,
        };
        let cluster = match &self.mode {
            Mode::Favorites => {
                return self
                    .favorite_scopes(cx)
                    .into_iter()
                    .map(|s| ScopeSpec { mode, ..s })
                    .collect();
            }
            Mode::Cluster(id) => id.clone(),
        };
        let spec = |namespace: Option<String>, only: Option<Vec<String>>| ScopeSpec {
            cluster: cluster.clone(),
            namespace,
            labels: None,
            only,
            label: None,
            mode,
        };
        if !self.namespaced || self.namespaces.is_empty() {
            return vec![spec(None, None)];
        }
        if let [namespace] = self.namespaces.as_slice() {
            return vec![spec(Some(namespace.clone()), None)];
        }
        // Several namespaces: one cluster-wide watch if allowed, else one per namespace.
        let query = AccessQuery::new("list", &self.gvr, None);
        let allowed = ConnectionManager::try_global(cx)
            .and_then(|m| m.read(cx).cached_can_i(&cluster, &query));
        if allowed.is_none() {
            self.request_access(cluster.clone(), query, cx);
        }
        if allowed == Some(true) {
            vec![spec(None, Some(self.namespaces.clone()))]
        } else {
            self.namespaces
                .iter()
                .map(|ns| spec(Some(ns.clone()), None))
                .collect()
        }
    }

    /// Asks the API server (once) whether `query` is allowed, then re-syncs.
    fn request_access(&mut self, cluster: ClusterId, query: AccessQuery, cx: &mut Context<Self>) {
        if !self.rbac_requested.insert(query.clone()) {
            return;
        }
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let task = manager.update(cx, |m, cx| m.can_i(&cluster, query, cx));
        cx.spawn(async move |this, cx| {
            if task.await.is_some() {
                this.update(cx, |_, cx| cx.notify()).ok();
            }
        })
        .detach();
    }

    /// Makes the sources match the current scope (namespaces, favorites, discovery).
    fn sync_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let desired = self.desired_scopes(cx);
        let current: Vec<&ScopeSpec> = self.sources.iter().map(|s| &s.spec).collect();
        if current.len() == desired.len() && current.iter().zip(&desired).all(|(a, b)| *a == b) {
            return;
        }
        let manager = ConnectionManager::try_global(cx);
        let mut sources = Vec::with_capacity(desired.len());
        for (ix, spec) in desired.into_iter().enumerate() {
            let info = manager.as_ref().and_then(|m| {
                let discovery = m.read(cx).discovery(&spec.cluster)?;
                catalog::find(&discovery, &self.gvr.group, &self.gvr.resource).cloned()
            });
            let gvr = info
                .as_ref()
                .map(|i| i.gvr.clone())
                .unwrap_or(self.gvr.clone());
            let mut key = StoreKey::new(spec.cluster.clone(), gvr.clone(), spec.namespace.clone());
            key.mode = spec.mode;
            if let Some(labels) = &spec.labels {
                key = key.labels(labels.clone());
            }
            let store = ResourceStores::acquire(cx, key);
            let observe = cx.observe_in(store.entity(), window, move |this, _, window, cx| {
                this.source_changed(ix, window, cx)
            });
            sources.push(Source {
                spec,
                gvr,
                kind: info.map(|i| i.gvk.kind),
                store,
                table: None,
                table_error: None,
                fetch: TableFetch::default(),
                _observe: observe,
            });
        }
        self.sources = sources;
        self.sort_cache.clear();
        self.update_columns();
        for ix in 0..self.sources.len() {
            self.source_changed(ix, window, cx);
        }
        self.refresh_rows(window, cx);
    }

    fn source_changed(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .sources
            .get(ix)
            .is_some_and(|s| s.spec.mode == StoreMode::Metadata)
        {
            self.schedule_table_fetch(ix, cx);
        }
        self.refresh_rows(window, cx);
    }

    /// Refetches the server-side table of a metadata source, at most once per second.
    fn schedule_table_fetch(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(source) = self.sources.get_mut(ix) else {
            return;
        };
        if !source.store.read(cx).status().is_ready() {
            return;
        }
        if source.fetch.in_flight {
            source.fetch.dirty = true;
            return;
        }
        let wait = source
            .fetch
            .last
            .map(|last| TABLE_REFRESH.saturating_sub(last.elapsed()))
            .unwrap_or_default();
        let Some(client) =
            ConnectionManager::try_global(cx).and_then(|m| m.read(cx).client(&source.spec.cluster))
        else {
            return;
        };
        let gvr = source.gvr.clone();
        let namespace = source.spec.namespace.clone();
        let labels = source.spec.labels.clone();
        let generation = source.store.read(cx).generation();
        source.fetch.in_flight = true;
        source.fetch.dirty = false;
        let executor = cx.background_executor().clone();
        let fetch = kubyl_core::spawn_kube(cx, async move {
            kubyl_resources::table::fetch_table(client, gvr, namespace, labels, None).await
        });
        source.fetch.task = Some(cx.spawn(async move |this, cx| {
            if !wait.is_zero() {
                executor.timer(wait).await;
            }
            let result = fetch.await;
            this.update(cx, |this, cx| {
                let Some(source) = this.sources.get_mut(ix) else {
                    return;
                };
                source.fetch.in_flight = false;
                source.fetch.last = Some(Instant::now());
                match result {
                    Ok(table) => {
                        tracing::debug!(
                            resource = %source.gvr,
                            rows = table.rows.len(),
                            columns = ?table.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
                            "server-side table fetched"
                        );
                        source.table = Some(table);
                        source.table_error = None;
                    }
                    Err(err) => source.table_error = Some(err.to_string()),
                }
                let again = source.fetch.dirty || source.store.read(cx).generation() != generation;
                this.sort_cache.clear();
                this.update_columns();
                if again {
                    this.schedule_table_fetch(ix, cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Shows `text` in the filter input and applies it.
    fn apply_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_input.update(cx, |input, cx| {
            input.set_value(text.to_string(), window, cx)
        });
        self.set_filter(text);
        self.refresh_rows(window, cx);
    }

    fn set_filter(&mut self, text: &str) {
        match Filter::parse(text) {
            Ok(filter) => {
                self.filter = filter;
                self.filter_error = None;
            }
            Err(err) => self.filter_error = Some(err),
        }
    }

    /// Recomputes the visible rows (filter, namespace restriction, sort) and keeps the
    /// selection on the same object.
    fn refresh_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let started = Instant::now();
        let mut rows = Vec::new();
        for (ix, source) in self.sources.iter().enumerate() {
            let only = source.spec.only.as_ref();
            for (key, object) in source.store.read(cx).objects() {
                if let Some(only) = only
                    && !format::namespace(object).is_some_and(|ns| only.iter().any(|o| o == ns))
                {
                    continue;
                }
                if !self.filter.is_empty() && !self.filter.matches(object) {
                    continue;
                }
                rows.push(rows::Row {
                    source: ix,
                    key: key.clone(),
                    object: object.clone(),
                });
            }
        }
        self.sort_rows(&mut rows, cx);
        let group = self.gvr.group.clone();
        // Health is cached per object version (the Arc changes when the object does).
        let mut health = HashMap::with_capacity(rows.len());
        self.failing = 0;
        if let Some(kind) = &self.kind {
            for row in &rows {
                let id = row.id();
                let pointer = Arc::as_ptr(&row.object) as usize;
                let failing = match self.health.get(&id) {
                    Some((cached, failing)) if *cached == pointer => *failing,
                    _ => kubyl_resources::columns::is_failing(&group, kind, &row.object),
                };
                self.failing += failing as usize;
                health.insert(id, (pointer, failing));
            }
        }
        self.health = health;
        let mut index = rows::keep_selection(&rows, self.selected.as_ref(), self.selected_index);
        // Like k9s, the cursor starts on the first row.
        if index.is_none() && !self.auto_selected && !rows.is_empty() {
            self.auto_selected = true;
            index = Some(0);
        }
        if !self.marked.is_empty() {
            let present: HashSet<RowId> = rows.iter().map(|r| r.id()).collect();
            self.marked.retain(|id| present.contains(id));
        }
        tracing::debug!(
            rows = rows.len(),
            elapsed_us = started.elapsed().as_micros() as u64,
            "list rows refreshed"
        );
        self.rows = rows;
        let previous = self.selected.clone();
        self.selected_index = index;
        self.selected = index.map(|i| self.rows[i].id());
        if self.selected != previous && self.focus.contains_focused(window, cx) {
            self.publish_selection(cx);
        }
        cx.notify();
    }

    fn sort_rows(&mut self, rows: &mut [rows::Row], cx: &App) {
        let (column, ascending) = self.sort.clone().unwrap_or_else(|| ("name".into(), true));
        // Name keys never change for a row, so they're computed once per object.
        let mut name_keys = HashMap::with_capacity(rows.len());
        let names: Vec<Arc<rows::NameKey>> = rows
            .iter()
            .map(|row| {
                let id = row.id();
                let key = self
                    .name_keys
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(rows::name_key(&row.object)));
                name_keys.insert(id, key.clone());
                key
            })
            .collect();
        self.name_keys = name_keys;
        let keys: Option<Vec<SortKey>> = (column.as_ref() != "name")
            .then(|| rows.iter().map(|r| self.sort_key(r, &column, cx)).collect());
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            let ordering = match &keys {
                Some(keys) => keys[a].compare(&keys[b]),
                None => names[a].cmp(&names[b]),
            };
            let ordering = if ascending {
                ordering
            } else {
                ordering.reverse()
            };
            ordering.then_with(|| names[a].cmp(&names[b]))
        });
        let sorted: Vec<rows::Row> = order.into_iter().map(|i| rows[i].clone()).collect();
        rows.clone_from_slice(&sorted);
    }

    fn sort_key(&mut self, row: &rows::Row, column: &str, cx: &App) -> SortKey {
        match column {
            "namespace" => {
                return SortKey::Text(format::namespace(&row.object).unwrap_or_default().into());
            }
            // Ascending age = youngest first.
            "age" => {
                return format::creation(&row.object)
                    .map(|t| SortKey::Number(-(t.as_second() as f64)))
                    .unwrap_or(SortKey::Missing);
            }
            "last_seen" => {
                return kubyl_resources::columns::event_time(&row.object)
                    .map(|t| SortKey::Number(-(t.as_second() as f64)))
                    .unwrap_or(SortKey::Missing);
            }
            _ => {}
        }
        let pointer = Arc::as_ptr(&row.object) as usize;
        let id = row.id();
        if let Some((cached, key)) = self.sort_cache.get(&id)
            && *cached == pointer
        {
            return key.clone();
        }
        let key = self
            .columns
            .iter()
            .find(|c| c.id.as_ref() == column)
            .map(|def| SortKey::of_cell(&self.cell(row, def, cx)))
            .unwrap_or(SortKey::Missing);
        self.sort_cache.insert(id, (pointer, key.clone()));
        key
    }

    /// The value of one cell.
    pub(crate) fn cell(&self, row: &rows::Row, column: &ColumnDef, cx: &App) -> CellValue {
        let source = &self.sources[row.source];
        let object = &row.object;
        match column.id.as_ref() {
            "cluster" => {
                let name = ConnectionManager::try_global(cx)
                    .map(|m| m.read(cx).display_name(&source.spec.cluster).to_string())
                    .unwrap_or_else(|| source.spec.cluster.to_string());
                return CellValue::Text(match &source.spec.label {
                    Some(label) => format!("{name} · {label}").into(),
                    None => name.into(),
                });
            }
            "namespace" => {
                return CellValue::Tinted {
                    label: format::namespace(object)
                        .unwrap_or_default()
                        .to_string()
                        .into(),
                    tone: kubyl_core::Tone::Neutral,
                };
            }
            "cpu" | "memory" => {
                if let Some(kind @ ("Pod" | "Node")) =
                    source.kind.as_deref().or(self.kind.as_deref())
                {
                    return kubyl_resources::metrics::usage_cell(
                        cx,
                        &source.spec.cluster,
                        kind,
                        object,
                        &column.id,
                    );
                }
            }
            _ => {}
        }
        match &self.columns_source {
            Columns::Provider(provider) => provider.cell(object, &column.id),
            Columns::Server => match column.id.as_ref() {
                "name" => CellValue::Text(format::name(object).to_string().into()),
                "age" => CellValue::Tinted {
                    label: format::object_age(object, jiff::Timestamp::now()).into(),
                    tone: kubyl_core::Tone::Neutral,
                },
                id => source
                    .table
                    .as_ref()
                    .map(|t| t.cell(&row.key, id))
                    .unwrap_or(CellValue::Empty),
            },
        }
    }

    /// Columns shown right now (wide and hidden columns applied).
    pub(crate) fn visible_columns(&self) -> Vec<ColumnDef> {
        self.columns
            .iter()
            .filter(|c| self.prefs.wide || !c.wide)
            .filter(|c| {
                c.id.as_ref() == "name" || !self.prefs.hidden.iter().any(|h| h == c.id.as_ref())
            })
            .map(|c| {
                let mut c = c.clone();
                if let Some(width) = self.widths.get(&c.id) {
                    c.width = ColumnWidth::Fixed(*width);
                }
                c
            })
            .collect()
    }

    fn save_prefs(&self, cx: &mut App) {
        let key = prefs_key(&self.gvr);
        let prefs = self.prefs.clone();
        State::update::<ListsState>(cx, |state| {
            state.kinds.insert(key, prefs);
        });
    }

    fn toggle_column(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.prefs.hidden.iter().position(|h| h == id) {
            self.prefs.hidden.remove(ix);
        } else {
            self.prefs.hidden.push(id.to_string());
        }
        self.save_prefs(cx);
        cx.notify();
    }

    fn toggle_wide(&mut self, cx: &mut Context<Self>) {
        self.prefs.wide = !self.prefs.wide;
        self.save_prefs(cx);
        cx.notify();
    }

    fn set_sort(&mut self, column: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.sort = match &self.sort {
            Some((current, ascending)) if *current == column => Some((column, !ascending)),
            _ => Some((column, true)),
        };
        self.refresh_rows(window, cx);
    }

    // ----- Namespaces -----

    /// Replaces the namespace selection (empty = all namespaces).
    fn set_namespaces(
        &mut self,
        namespaces: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if namespaces == self.namespaces {
            return;
        }
        self.namespaces = namespaces;
        // Keep the title bar in sync when this is the active cluster and one (or all) is selected.
        if let Some(cluster) = self.cluster().cloned() {
            let active = ActiveContext::global(cx).clone();
            if active.cluster.as_ref().map(|c| &c.id) == Some(&cluster)
                && self.namespaces.len() <= 1
            {
                let namespace = self.namespaces.first().cloned().map(SharedString::from);
                if active.namespace != namespace {
                    ActiveContext::set(
                        cx,
                        ActiveContext {
                            namespace,
                            ..active
                        },
                    );
                }
            }
        }
        self.update_columns();
        self.sync_sources(window, cx);
    }

    fn active_context_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cluster) = self.cluster() else {
            return;
        };
        let active = ActiveContext::global(cx);
        if active.cluster.as_ref().map(|c| &c.id) != Some(cluster) {
            return;
        }
        let wanted: Vec<String> = active.namespace.iter().map(|n| n.to_string()).collect();
        // A multi-namespace selection made here stays until the title bar picks one.
        if self.namespaces.len() > 1 && wanted.is_empty() {
            return;
        }
        if wanted != self.namespaces {
            self.set_namespaces(wanted, window, cx);
        }
    }

    fn connection_event(
        &mut self,
        event: &ConnectionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cluster = match event {
            ConnectionEvent::DiscoveryChanged(id)
            | ConnectionEvent::StateChanged(id)
            | ConnectionEvent::NamespacesChanged(id) => id,
            ConnectionEvent::ContextsChanged => {
                if self.mode == Mode::Favorites {
                    self.sync_sources(window, cx);
                }
                return;
            }
            _ => return,
        };
        let relevant = match &self.mode {
            Mode::Cluster(id) => id == cluster,
            Mode::Favorites => self.sources.iter().any(|s| &s.spec.cluster == cluster),
        };
        if !relevant {
            return;
        }
        if matches!(event, ConnectionEvent::DiscoveryChanged(_)) {
            let before = self.kind.clone();
            self.resolve_kind(cx);
            if before != self.kind {
                self.sources.clear();
            }
        }
        self.sync_sources(window, cx);
        cx.notify();
    }

    // ----- Selection -----

    fn select_index(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        let index = index
            .filter(|_| !self.rows.is_empty())
            .map(|i| i.min(self.rows.len() - 1));
        self.selected_index = index;
        self.selected = index.map(|i| self.rows[i].id());
        if let Some(index) = index {
            self.scroll
                .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        }
        self.publish_selection(cx);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let next = match self.selected_index {
            Some(ix) => ix.saturating_add_signed(delta),
            None => 0,
        };
        if extend {
            let anchor = self.anchor.or(self.selected_index).unwrap_or(0);
            self.anchor = Some(anchor);
            self.mark_range(anchor, next.min(self.rows.len().saturating_sub(1)));
        } else {
            self.anchor = None;
            self.marked.clear();
        }
        self.select_index(Some(next), cx);
    }

    fn mark_range(&mut self, from: usize, to: usize) {
        self.marked.clear();
        let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
        for row in self.rows.iter().take(hi + 1).skip(lo) {
            self.marked.insert(row.id());
        }
    }

    /// Row click: plain selects, shift extends, secondary (⌘/Ctrl) toggles.
    pub(crate) fn click_row(
        &mut self,
        index: usize,
        modifiers: gpui::Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        if modifiers.shift {
            let anchor = self.anchor.or(self.selected_index).unwrap_or(index);
            self.anchor = Some(anchor);
            self.mark_range(anchor, index);
        } else if modifiers.secondary() {
            if let Some(row) = self.rows.get(index) {
                let id = row.id();
                if self.marked.is_empty()
                    && let Some(current) = self.selected.clone()
                {
                    self.marked.insert(current);
                }
                if !self.marked.remove(&id) {
                    self.marked.insert(id);
                }
            }
            self.anchor = Some(index);
        } else {
            self.marked.clear();
            self.anchor = Some(index);
        }
        self.select_index(Some(index), cx);
    }

    pub(crate) fn is_marked(&self, row: &rows::Row) -> bool {
        self.marked.contains(&row.id())
    }

    /// The resource ref of a row.
    fn row_ref(&self, row: &rows::Row) -> ResourceRef {
        let source = &self.sources[row.source];
        ResourceRef::object(
            source.spec.cluster.clone(),
            source.gvr.clone(),
            format::namespace(&row.object).map(String::from),
            format::name(&row.object).to_string(),
        )
    }

    /// Publishes the selection (marked rows, else the cursor row) for actions and the dock.
    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let mut rows: Vec<&rows::Row> = Vec::new();
        if let Some(index) = self.selected_index
            && let Some(row) = self.rows.get(index)
        {
            rows.push(row);
        }
        if !self.marked.is_empty() {
            rows.extend(
                self.rows
                    .iter()
                    .filter(|r| self.marked.contains(&r.id()) && Some(r.id()) != self.selected),
            );
            if self
                .selected
                .as_ref()
                .is_some_and(|s| !self.marked.contains(s))
            {
                rows.remove(0);
            }
        }
        let items: Vec<Selected> = rows
            .iter()
            .map(|row| {
                let source = &self.sources[row.source];
                Selected {
                    target: self.row_ref(row),
                    kind: source
                        .kind
                        .clone()
                        .or(self.kind.clone())
                        .unwrap_or_default(),
                    object: Some(row.object.clone()),
                    store: Some(source.store.entity().clone()),
                }
            })
            .collect();
        let caps = items
            .first()
            .and_then(|s| {
                ConnectionManager::try_global(cx).map(|m| m.read(cx).caps(&s.target.cluster))
            })
            .unwrap_or_default();
        ResourceSelection::set(cx, ResourceSelection { items, caps });
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_index.and_then(|i| self.rows.get(i)) else {
            return;
        };
        let target = self.row_ref(row);
        window.dispatch_action(
            Box::new(kubyl_core::actions::OpenView(ViewRequest::for_resource(
                ViewKind::Details,
                target,
            ))),
            cx,
        );
    }

    // ----- Rendering helpers -----

    /// Label for the kind (`Pods`).
    fn kind_label(&self) -> String {
        catalog::label_for(&self.gvr.group, &self.gvr.resource, self.kind.as_deref())
    }

    fn icon(&self) -> IconName {
        catalog::icon_for(&self.gvr.group, &self.gvr.resource)
    }

    /// The key context of the table: `ResourceList kind=Pod`.
    fn key_context(&self) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add(CONTEXT);
        if let Some(kind) = &self.kind {
            context.set("kind", kind.clone());
        }
        context
    }

    /// Key hints for the selected object (or the list), from the action registry.
    fn hints(&mut self, cx: &mut Context<Self>) -> Vec<(SharedString, SharedString)> {
        let target = self
            .selected_index
            .and_then(|i| self.rows.get(i))
            .map(|row| self.row_ref(row))
            .or_else(|| {
                let cluster = self.cluster().cloned()?;
                Some(ResourceRef::list(
                    cluster,
                    self.gvr.clone(),
                    self.namespaces.first().cloned(),
                ))
            });
        let Some(target) = target else {
            return Vec::new();
        };
        let manager = ConnectionManager::try_global(cx);
        let caps = manager
            .as_ref()
            .map(|m| m.read(cx).caps(&target.cluster))
            .unwrap_or_default();
        let context = self.key_context();
        let mut hints: Vec<(SharedString, SharedString, Option<AccessQuery>)> =
            ActionRegistry::global(cx)
                .all()
                .iter()
                .filter(|spec| {
                    spec.context
                        .as_deref()
                        .is_some_and(|c| c.starts_with(CONTEXT))
                })
                .filter(|spec| spec_matches_context(spec, &context))
                .filter(|spec| spec.is_available(&target, &caps))
                .filter_map(|spec| {
                    let query = crate::actions::access_for(&spec.name, &target);
                    Some((spec.keystrokes.clone()?, spec.hint.clone()?, query))
                })
                .collect();
        let mut missing = Vec::new();
        if let Some(manager) = &manager {
            hints.retain(|(_, _, query)| match query {
                Some(query) => match manager.read(cx).cached_can_i(&target.cluster, query) {
                    Some(allowed) => allowed,
                    None => {
                        missing.push(query.clone());
                        true
                    }
                },
                None => true,
            });
        }
        for query in missing {
            self.request_access(target.cluster.clone(), query, cx);
        }
        let mut hints: Vec<(SharedString, SharedString)> =
            hints.into_iter().map(|(k, h, _)| (k, h)).collect();
        sort_hints(&mut hints);
        hints
    }

    fn status_summary(&self, cx: &App) -> (SharedString, kubyl_core::Tone) {
        let statuses: Vec<StoreStatus> = self
            .sources
            .iter()
            .map(|s| s.store.read(cx).status().clone())
            .collect();
        if statuses.is_empty() {
            return ("idle".into(), kubyl_core::Tone::Muted);
        }
        if statuses.contains(&StoreStatus::Forbidden) {
            return ("forbidden".into(), kubyl_core::Tone::Bad);
        }
        if statuses.iter().any(|s| matches!(s, StoreStatus::Error(_))) {
            return ("retrying".into(), kubyl_core::Tone::Warning);
        }
        if statuses.contains(&StoreStatus::Waiting) {
            return ("offline".into(), kubyl_core::Tone::Muted);
        }
        if statuses.contains(&StoreStatus::Loading) {
            return ("syncing".into(), kubyl_core::Tone::Info);
        }
        ("live".into(), kubyl_core::Tone::Good)
    }

    fn namespace_label(&self) -> String {
        if self.mode == Mode::Favorites {
            let clusters: HashSet<&ClusterId> =
                self.sources.iter().map(|s| &s.spec.cluster).collect();
            return format!(
                "{} favorites on {} clusters",
                self.sources.len(),
                clusters.len()
            );
        }
        if !self.namespaced {
            return "cluster-wide".into();
        }
        match self.namespaces.as_slice() {
            [] => "in all namespaces".into(),
            [one] => format!("in {one}"),
            many => format!("in {} namespaces", many.len()),
        }
    }

    /// The empty-state message, if there are no rows.
    fn empty_message(&self, cx: &App) -> Option<String> {
        if !self.rows.is_empty() {
            return None;
        }
        let label = self.kind_label().to_lowercase();
        let status = self
            .sources
            .first()
            .map(|s| s.store.read(cx).status().clone());
        Some(match status {
            None => "Nothing to show.".into(),
            Some(StoreStatus::Waiting) => "Waiting for the cluster connection…".into(),
            Some(StoreStatus::Loading) => format!("Loading {label}…"),
            Some(StoreStatus::Forbidden) => {
                if self.namespaced && self.namespaces.is_empty() {
                    format!("You may not list {label} across all namespaces. Pick a namespace.")
                } else {
                    format!("You may not list {label} here.")
                }
            }
            Some(StoreStatus::Unsupported) => format!("This cluster doesn't serve {label}."),
            Some(StoreStatus::Error(err)) => format!("Watch failed, retrying: {err}"),
            Some(StoreStatus::Ready) if !self.filter.is_empty() => {
                format!("No {label} match the filter.")
            }
            Some(StoreStatus::Ready) => format!("No {label} {}.", self.namespace_label()),
        })
    }

    /// Adds the namespace of the selection (or the list's) to the favorites.
    pub(crate) fn favorite_namespace(&self, cx: &mut App) {
        let namespace = self
            .selected_index
            .and_then(|i| self.rows.get(i))
            .and_then(|r| format::namespace(&r.object).map(String::from))
            .or_else(|| self.namespaces.first().cloned());
        let (Some(cluster), Some(namespace)) = (self.cluster().cloned(), namespace) else {
            return;
        };
        crate::actions::add_favorite(&cluster, &namespace, cx);
    }
}

/// Whether a spec's context predicate could apply to this list's key context. Only simple
/// `ResourceList && kind == X` predicates are checked; anything else counts as a match.
fn spec_matches_context(spec: &kubyl_core::ActionSpec, context: &KeyContext) -> bool {
    let Some(predicate) = spec.context.as_deref() else {
        return true;
    };
    match gpui::KeyBindingContextPredicate::parse(predicate) {
        Ok(predicate) => predicate.eval(std::slice::from_ref(context)),
        Err(_) => true,
    }
}

/// Orders hints like k9s (`l s d e ⇧f ⌃k ⌃d`), then everything else in registration order.
/// k9s order: `l s d e ⇧f ⌃k ⌃d` first, `:` (palette) and `/` (filter) last, the rest in
/// registration order.
fn sort_hints(hints: &mut [(SharedString, SharedString)]) {
    const ORDER: &[&str] = &["l", "s", "d", "e", "shift-f", "ctrl-k", "ctrl-d"];
    const LAST: &[&str] = &[":", "/"];
    hints.sort_by_key(|(key, _)| {
        let key = key.as_ref();
        match (
            ORDER.iter().position(|o| *o == key),
            LAST.iter().position(|o| *o == key),
        ) {
            (Some(ix), _) => ix,
            (None, Some(ix)) => ORDER.len() + 1 + ix,
            (None, None) => ORDER.len(),
        }
    });
}

fn prefs_key(gvr: &Gvr) -> String {
    if gvr.group.is_empty() {
        gvr.resource.clone()
    } else {
        format!("{}.{}", gvr.resource, gvr.group)
    }
}

impl Focusable for ResourceListView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TabView for ResourceListView {
    fn tab_title(&self, _: &App) -> SharedString {
        match &self.mode {
            Mode::Cluster(_) => self.kind_label().into(),
            Mode::Favorites => format!("Favorites · {}", self.kind_label()).into(),
        }
    }

    fn tab_icon(&self, _: &App) -> Option<SharedString> {
        Some(match self.mode {
            Mode::Cluster(_) => self.icon().path(),
            Mode::Favorites => IconName::StarFilled.path(),
        })
    }

    fn view_request(&self, _: &App) -> Option<ViewRequest> {
        match &self.mode {
            Mode::Cluster(cluster) => Some(ViewRequest::for_resource(
                ViewKind::Table,
                ResourceRef::list(cluster.clone(), self.gvr.clone(), None),
            )),
            Mode::Favorites => Some(ViewRequest::for_resource(
                crate::favorites_view_kind(),
                ResourceRef::list(ClusterId::new(""), self.gvr.clone(), None),
            )),
        }
    }
}

impl Render for ResourceListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        table::render(self, window, cx)
    }
}
