//! The Clusters section: one root per context, with the kinds the cluster serves grouped like
//! the mockup (Workloads, Network…, Custom Resources by API group).
//!
//! Collapsed roots don't connect. Kinds the user may not `list` are hidden, and so are empty
//! groups. Counts come from metadata-only watches of the kinds in expanded groups.

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Entity, FocusHandle, Focusable, IntoElement,
    KeyBinding, Render, SharedString, Subscription, WeakEntity, Window, actions, div, prelude::*,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::ContextMenuExt as _;
use kubyl_core::actions::OpenView;
use kubyl_core::{ActiveContext, ClusterId, Gvr, ResourceRef, ViewKind, ViewRegistry, ViewRequest};
use kubyl_kube::access::AccessQuery;
use kubyl_kube::{ConnectionEvent, ConnectionManager, ConnectionState};
use kubyl_resources::{ResourceStores, StoreHandle, StoreKey};
use kubyl_settings::{Settings, State};
use kubyl_ui::{
    ActiveColors, Colors, Icon, IconName, ProdBadge, SectionHeader, StatusDot, TreeRow, fonts,
    h_flex, u, v_flex,
};

use crate::catalog::{self, CUSTOM, TreeKind, ViewEntry};
use crate::settings::{ClusterOrder, ExplorerSettings, TreeState};

actions!(
    explorer_tree,
    [
        SelectNext,
        SelectPrevious,
        Expand,
        Collapse,
        Activate,
        ClearFilter
    ]
);

const CONTEXT: &str = "ExplorerTree";

pub(crate) fn bind_keys(cx: &mut App) {
    let tree = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, tree),
        KeyBinding::new("j", SelectNext, tree),
        KeyBinding::new("up", SelectPrevious, tree),
        KeyBinding::new("k", SelectPrevious, tree),
        KeyBinding::new("right", Expand, tree),
        KeyBinding::new("l", Expand, tree),
        KeyBinding::new("left", Collapse, tree),
        KeyBinding::new("h", Collapse, tree),
        KeyBinding::new("enter", Activate, tree),
        KeyBinding::new("space", Activate, tree),
        KeyBinding::new("escape", ClearFilter, Some("ExplorerFilter")),
        KeyBinding::new("down", SelectNext, Some("ExplorerFilter")),
    ]);
}

/// A row of the cluster tree.
#[derive(Clone, Debug)]
enum Item {
    Root(ClusterId),
    Status {
        cluster: ClusterId,
        text: SharedString,
        sign_in: bool,
    },
    Group {
        cluster: ClusterId,
        /// `workloads`, `custom`, `custom:cert-manager.io`, `administration/argocd`.
        id: String,
        label: SharedString,
        depth: usize,
        expanded: bool,
        /// Extra text on the row (a contributed group's version…).
        badge: Option<SharedString>,
    },
    Kind {
        cluster: ClusterId,
        kind: TreeKind,
        depth: usize,
    },
    View {
        cluster: ClusterId,
        entry: ViewEntry,
        depth: usize,
    },
}

impl Item {
    fn id(&self) -> String {
        match self {
            Item::Root(c) => format!("root|{c}"),
            Item::Status { cluster, .. } => format!("status|{cluster}"),
            Item::Group { cluster, id, .. } => format!("group|{cluster}|{id}"),
            Item::Kind { cluster, kind, .. } => match kind.via {
                // The same kind is listed under Custom Resources too.
                Some(via) => format!("kind|{cluster}|{via}|{}", kind.gvr),
                None => format!("kind|{cluster}|{}", kind.gvr),
            },
            Item::View { cluster, entry, .. } => format!("view|{cluster}|{}", entry.id),
        }
    }

    fn cluster(&self) -> &ClusterId {
        match self {
            Item::Root(c) => c,
            Item::Status { cluster, .. }
            | Item::Group { cluster, .. }
            | Item::Kind { cluster, .. }
            | Item::View { cluster, .. } => cluster,
        }
    }
}

/// Open sections per window, so the Explorer header's search button reaches them.
#[derive(Default)]
pub(crate) struct Sections(pub Vec<(AnyWindowHandle, WeakEntity<ClustersSection>)>);

impl gpui::Global for Sections {}

pub struct ClustersSection {
    state: TreeState,
    selected: Option<String>,
    filter: Option<Entity<InputState>>,
    counts: HashMap<StoreKey, StoreHandle>,
    rbac_requested: HashSet<(ClusterId, AccessQuery)>,
    /// Expanded roots already connected at startup (a later manual disconnect sticks).
    started: HashSet<ClusterId>,
    focus: FocusHandle,
    _count_observers: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl ClustersSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![
            cx.observe_global::<ActiveContext>(|this, cx| {
                this.schedule_count_sync(cx);
                cx.notify();
            }),
            Settings::observe::<ExplorerSettings>(cx, |_, _| {}),
            // Groups other crates contribute (their badges change).
            cx.observe_global::<catalog::TreeGroups>(|this, cx| {
                this.schedule_count_sync(cx);
                cx.notify();
            }),
        ];
        if let Some(manager) = ConnectionManager::try_global(cx) {
            subscriptions.push(
                cx.subscribe(&manager, |this, _, event: &ConnectionEvent, cx| {
                    if matches!(
                        event,
                        ConnectionEvent::ContextsChanged | ConnectionEvent::Rekeyed { .. }
                    ) {
                        this.migrate_ids(cx);
                        this.connect_expanded(cx);
                    }
                    if matches!(
                        event,
                        ConnectionEvent::DiscoveryChanged(_)
                            | ConnectionEvent::StateChanged(_)
                            | ConnectionEvent::ContextsChanged
                            | ConnectionEvent::NamespacesChanged(_)
                    ) {
                        this.schedule_count_sync(cx);
                        cx.notify();
                    }
                }),
            );
        }
        let state = State::get::<TreeState>(cx);
        let handle = window.window_handle();
        let weak = cx.weak_entity();
        cx.default_global::<Sections>().0.push((handle, weak));
        let mut this = Self {
            state,
            selected: None,
            filter: None,
            counts: HashMap::new(),
            rbac_requested: HashSet::new(),
            started: HashSet::new(),
            focus: cx.focus_handle(),
            _count_observers: Vec::new(),
            _subscriptions: subscriptions,
        };
        // Expanded roots connect on start (now, or once the kubeconfigs are loaded).
        this.migrate_ids(cx);
        this.connect_expanded(cx);
        this.schedule_count_sync(cx);
        this
    }

    /// Keeps expanded roots and groups under the current cluster ids (contexts were grouped,
    /// an entry was re-keyed).
    fn migrate_ids(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let manager = manager.read(cx);
        if manager.is_loading() {
            return;
        }
        let resolve = |id: &str| manager.resolve(&ClusterId::new(id)).to_string();
        let roots: std::collections::BTreeSet<String> =
            self.state.roots.iter().map(|r| resolve(r)).collect();
        let groups: std::collections::BTreeSet<String> = self
            .state
            .groups
            .iter()
            .map(|key| match key.rsplit_once('|') {
                Some((cluster, group)) => format!("{}|{group}", resolve(cluster)),
                None => key.clone(),
            })
            .collect();
        if roots != self.state.roots || groups != self.state.groups {
            self.state.roots = roots;
            self.state.groups = groups;
            self.save(cx);
        }
    }

    /// Connects expanded roots that weren't connected yet this session.
    fn connect_expanded(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let ids: Vec<ClusterId> = manager
            .read(cx)
            .contexts()
            .filter(|c| self.state.roots.contains(c.id.as_str()) && !self.started.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        self.started.extend(ids.iter().cloned());
        manager.update(cx, |m, cx| {
            for id in &ids {
                m.ensure_connected(id, cx);
            }
        });
    }

    fn save(&self, cx: &mut App) {
        State::set(cx, &self.state);
    }

    fn is_expanded_root(&self, id: &ClusterId) -> bool {
        self.state.roots.contains(id.as_str())
    }

    fn group_key(cluster: &ClusterId, id: &str) -> String {
        format!("{cluster}|{id}")
    }

    fn is_expanded_group(&self, cluster: &ClusterId, id: &str) -> bool {
        self.state.groups.contains(&Self::group_key(cluster, id))
    }

    fn filter_query(&self, cx: &App) -> Option<String> {
        let text = self.filter.as_ref()?.read(cx).value().trim().to_lowercase();
        (!text.is_empty()).then_some(text)
    }

    /// The namespace kinds of `cluster` are counted and checked in.
    fn scope_namespace(cluster: &ClusterId, cx: &App) -> Option<String> {
        let active = ActiveContext::global(cx);
        if active.cluster.as_ref().map(|c| &c.id) == Some(cluster) {
            return active.namespace.as_ref().map(|n| n.to_string());
        }
        None
    }

    /// Whether the user may list `kind` (unknown counts as yes; asks the server once).
    fn allowed(&mut self, cluster: &ClusterId, kind: &TreeKind, cx: &mut Context<Self>) -> bool {
        let namespace = if kind.namespaced {
            Self::scope_namespace(cluster, cx)
        } else {
            None
        };
        let query = AccessQuery::new("list", &kind.gvr, namespace.as_deref());
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return true;
        };
        match manager.read(cx).cached_can_i(cluster, &query) {
            Some(allowed) => allowed,
            None => {
                if self.rbac_requested.insert((cluster.clone(), query.clone())) {
                    let task = manager.update(cx, |m, cx| m.can_i(cluster, query, cx));
                    cx.spawn(async move |this, cx| {
                        if task.await.is_some() {
                            this.update(cx, |_, cx| cx.notify()).ok();
                        }
                    })
                    .detach();
                }
                true
            }
        }
    }

    /// The flattened, visible tree.
    fn items(&mut self, cx: &mut Context<Self>) -> Vec<Item> {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return Vec::new();
        };
        let settings = Settings::get::<ExplorerSettings>(cx).clone();
        let query = self.filter_query(cx);
        let contexts = Self::roots(&settings, self.state.connected_only, cx);
        let mut items = Vec::new();
        for cluster in contexts {
            items.push(Item::Root(cluster.clone()));
            if !self.is_expanded_root(&cluster) {
                continue;
            }
            let (state, discovery, caps) = {
                let m = manager.read(cx);
                (m.state(&cluster), m.discovery(&cluster), m.caps(&cluster))
            };
            let discovery = match (&state, discovery) {
                (ConnectionState::Connected { .. }, Some(discovery)) => discovery,
                (ConnectionState::Connected { .. }, None) => {
                    items.push(Item::Status {
                        cluster: cluster.clone(),
                        text: "Discovering API…".into(),
                        sign_in: false,
                    });
                    continue;
                }
                (state, _) => {
                    let sign_in =
                        matches!(state, ConnectionState::AuthRequired { sign_in: true, .. });
                    let text = match state {
                        ConnectionState::Disconnected => {
                            "Not connected · click to connect".to_string()
                        }
                        ConnectionState::Connecting => "Connecting…".to_string(),
                        ConnectionState::AuthRequired { sign_in: true, .. } => {
                            "Sign-in required · click to sign in".into()
                        }
                        other => {
                            format!("{} · {}", other.label(), other.error().unwrap_or_default())
                        }
                    };
                    items.push(Item::Status {
                        cluster: cluster.clone(),
                        text: text.into(),
                        sign_in,
                    });
                    continue;
                }
            };
            let matches = |label: &str| {
                query
                    .as_ref()
                    .is_none_or(|q| label.to_lowercase().contains(q))
            };
            for group_id in catalog::ordered_groups(&settings.group_order, &settings.hidden_groups)
            {
                if group_id == CUSTOM {
                    let groups = catalog::custom_groups(&discovery);
                    let mut children = Vec::new();
                    for (api_group, kinds) in groups {
                        let id = format!("custom:{api_group}");
                        let visible: Vec<TreeKind> = kinds
                            .into_iter()
                            .filter(|k| matches(&k.label) || matches(&api_group))
                            .filter(|k| self.allowed(&cluster, k, cx))
                            .collect();
                        if visible.is_empty() {
                            continue;
                        }
                        let expanded = self.is_expanded_group(&cluster, &id) || query.is_some();
                        children.push(Item::Group {
                            cluster: cluster.clone(),
                            id: id.clone(),
                            label: api_group.into(),
                            depth: 2,
                            expanded,
                            badge: None,
                        });
                        if expanded {
                            children.extend(visible.into_iter().map(|kind| Item::Kind {
                                cluster: cluster.clone(),
                                kind,
                                depth: 3,
                            }));
                        }
                    }
                    if !children.is_empty() {
                        let expanded = self.is_expanded_group(&cluster, CUSTOM) || query.is_some();
                        items.push(Item::Group {
                            cluster: cluster.clone(),
                            id: CUSTOM.into(),
                            label: "Custom Resources".into(),
                            depth: 1,
                            expanded,
                            badge: None,
                        });
                        if expanded {
                            items.extend(children);
                        }
                    }
                    continue;
                }
                let Some(def) = catalog::group(group_id) else {
                    continue;
                };
                let kinds: Vec<TreeKind> = catalog::group_kinds(def, &discovery)
                    .into_iter()
                    .filter(|k| matches(&k.label))
                    .filter(|k| self.allowed(&cluster, k, cx))
                    .collect();
                let views: Vec<ViewEntry> = (def.views)()
                    .into_iter()
                    .filter(|v| !v.needs_olm || caps.olm)
                    .filter(|v| matches(v.label))
                    .collect();
                // Groups other crates add here (Argo CD under Administration), shown while the
                // cluster serves their kinds.
                let mut contributed = Vec::new();
                for group in catalog::contributed_groups(def.id, cx) {
                    let visible: Vec<TreeKind> = catalog::contributed_kinds(&group, &discovery)
                        .into_iter()
                        .filter(|k| matches(&k.label) || matches(group.label))
                        .filter(|k| self.allowed(&cluster, k, cx))
                        .collect();
                    if visible.is_empty() {
                        continue;
                    }
                    let id = format!("{}/{}", def.id, group.id);
                    let expanded = self.is_expanded_group(&cluster, &id) || query.is_some();
                    let badge = group.badge.as_ref().and_then(|badge| badge(&cluster, cx));
                    contributed.push(Item::Group {
                        cluster: cluster.clone(),
                        id,
                        label: group.label.into(),
                        depth: 2,
                        expanded,
                        badge,
                    });
                    if expanded {
                        contributed.extend(visible.into_iter().map(|kind| Item::Kind {
                            cluster: cluster.clone(),
                            kind,
                            depth: 3,
                        }));
                    }
                }
                if kinds.is_empty() && views.is_empty() && contributed.is_empty() {
                    continue;
                }
                if !def.collapsible {
                    items.extend(views.into_iter().map(|entry| Item::View {
                        cluster: cluster.clone(),
                        entry,
                        depth: 1,
                    }));
                    items.extend(kinds.into_iter().map(|kind| Item::Kind {
                        cluster: cluster.clone(),
                        kind,
                        depth: 1,
                    }));
                    continue;
                }
                let expanded = self.is_expanded_group(&cluster, def.id) || query.is_some();
                items.push(Item::Group {
                    cluster: cluster.clone(),
                    id: def.id.into(),
                    label: def.label.into(),
                    depth: 1,
                    expanded,
                    badge: None,
                });
                if expanded {
                    items.extend(views.into_iter().map(|entry| Item::View {
                        cluster: cluster.clone(),
                        entry,
                        depth: 2,
                    }));
                    items.extend(kinds.into_iter().map(|kind| Item::Kind {
                        cluster: cluster.clone(),
                        kind,
                        depth: 2,
                    }));
                    items.extend(contributed);
                }
            }
        }
        items
    }

    /// The cluster roots in the configured order; with "connected only", the connected and
    /// connecting ones plus the active cluster.
    fn roots(settings: &ExplorerSettings, connected_only: bool, cx: &App) -> Vec<ClusterId> {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return Vec::new();
        };
        let manager = manager.read(cx);
        let roots = manager
            .contexts()
            .map(|c| {
                let state = manager.state(&c.id);
                let live = state.is_connected() || state == ConnectionState::Connecting;
                (c.id.clone(), manager.display_name(&c.id).to_string(), live)
            })
            .collect();
        order_roots(
            roots,
            settings.cluster_order,
            connected_only,
            manager.active(),
        )
    }

    fn schedule_count_sync(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            this.update(cx, |this, cx| this.sync_counts(cx)).ok();
        })
        .detach();
    }

    fn count_key(cluster: &ClusterId, kind: &TreeKind, cx: &App) -> StoreKey {
        let namespace = if kind.namespaced {
            Self::scope_namespace(cluster, cx)
        } else {
            None
        };
        StoreKey::new(cluster.clone(), kind.gvr.clone(), namespace).metadata()
    }

    /// Keeps one metadata watch per visible kind (for counts).
    fn sync_counts(&mut self, cx: &mut Context<Self>) {
        if !Settings::get::<ExplorerSettings>(cx).show_counts {
            self.counts.clear();
            self._count_observers.clear();
            return;
        }
        let items = self.items(cx);
        let wanted: HashSet<StoreKey> = items
            .iter()
            .filter_map(|item| match item {
                Item::Kind { cluster, kind, .. } => Some(Self::count_key(cluster, kind, cx)),
                _ => None,
            })
            .collect();
        let current: HashSet<StoreKey> = self.counts.keys().cloned().collect();
        if wanted == current {
            return;
        }
        self.counts.retain(|key, _| wanted.contains(key));
        for key in wanted {
            if let std::collections::hash_map::Entry::Vacant(entry) = self.counts.entry(key) {
                let handle = ResourceStores::acquire(cx, entry.key().clone());
                entry.insert(handle);
            }
        }
        self._count_observers = self
            .counts
            .values()
            .map(|handle| cx.observe(handle.entity(), |_, _, cx| cx.notify()))
            .collect();
    }

    fn count(&self, cluster: &ClusterId, kind: &TreeKind, cx: &App) -> Option<String> {
        let store = self
            .counts
            .get(&Self::count_key(cluster, kind, cx))?
            .read(cx);
        store
            .status()
            .is_settled()
            .then(|| store.len().to_string())
            .filter(|_| store.status().is_ready())
    }

    // ----- Interaction -----

    fn toggle_root(&mut self, cluster: &ClusterId, cx: &mut Context<Self>) {
        let key = cluster.to_string();
        if !self.state.roots.remove(&key) {
            self.state.roots.insert(key);
            // First expansion opens Workloads, like the mockup.
            let workloads = Self::group_key(cluster, "workloads");
            if !self
                .state
                .groups
                .iter()
                .any(|g| g.starts_with(&format!("{cluster}|")))
            {
                self.state.groups.insert(workloads);
            }
            if let Some(manager) = ConnectionManager::try_global(cx) {
                manager.update(cx, |m, cx| m.ensure_connected(cluster, cx));
            }
        }
        self.save(cx);
        self.schedule_count_sync(cx);
        cx.notify();
    }

    fn toggle_group(&mut self, cluster: &ClusterId, id: &str, cx: &mut Context<Self>) {
        let key = Self::group_key(cluster, id);
        if !self.state.groups.remove(&key) {
            self.state.groups.insert(key);
        }
        self.save(cx);
        self.schedule_count_sync(cx);
        cx.notify();
    }

    /// Makes `cluster` the title-bar cluster unless it already is.
    fn activate_cluster(cluster: &ClusterId, cx: &mut App) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let active = manager.read(cx).active().cloned();
        if active.as_ref() != Some(cluster) {
            manager.update(cx, |m, cx| m.activate(cluster, cx));
        }
    }

    fn activate_item(&mut self, item: &Item, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(item.id());
        match item {
            Item::Root(cluster) => self.toggle_root(cluster, cx),
            Item::Group { cluster, id, .. } => self.toggle_group(cluster, id, cx),
            Item::Status { cluster, .. } => {
                if let Some(manager) = ConnectionManager::try_global(cx) {
                    manager.update(cx, |m, cx| m.activate(cluster, cx));
                }
            }
            Item::Kind { cluster, kind, .. } => {
                Self::activate_cluster(cluster, cx);
                // Events get their own view (live stream, Warning/Normal filters) when a crate
                // provides one; the generic table otherwise.
                let events = kind.gvr.group.is_empty()
                    && kind.gvr.resource == "events"
                    && ViewRegistry::is_registered(cx, &ViewKind::Events);
                // Rows of a contributed group open the kind's own view (Argo CD Applications);
                // the same kind under Custom Resources opens the generic table.
                let view = if events {
                    ViewKind::Events
                } else if kind.via.is_some() {
                    ViewRegistry::list_view(cx, &kind.gvr)
                } else {
                    ViewKind::Table
                };
                window.dispatch_action(
                    Box::new(OpenView(ViewRequest::for_resource(
                        view,
                        ResourceRef::list(cluster.clone(), kind.gvr.clone(), None),
                    ))),
                    cx,
                );
            }
            Item::View { cluster, entry, .. } => {
                Self::activate_cluster(cluster, cx);
                // The cluster's Overview is cluster-wide; a namespace makes it the namespace
                // variant (opened from a favorite).
                let namespace =
                    Self::scope_namespace(cluster, cx).filter(|_| entry.kind != ViewKind::Overview);
                window.dispatch_action(
                    Box::new(OpenView(ViewRequest::for_resource(
                        entry.kind.clone(),
                        ResourceRef::list(cluster.clone(), Gvr::new("", "", ""), namespace),
                    ))),
                    cx,
                );
            }
        }
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let items = self.items(cx);
        if items.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|s| items.iter().position(|i| &i.id() == s));
        let next = match current {
            Some(ix) => (ix as isize + delta).clamp(0, items.len() as isize - 1) as usize,
            None => 0,
        };
        self.selected = Some(items[next].id());
        cx.notify();
    }

    fn selected_item(&mut self, cx: &mut Context<Self>) -> Option<Item> {
        let selected = self.selected.clone()?;
        self.items(cx).into_iter().find(|i| i.id() == selected)
    }

    fn expand_selected(&mut self, expand: bool, cx: &mut Context<Self>) {
        let Some(item) = self.selected_item(cx) else {
            return;
        };
        match &item {
            Item::Root(cluster) if self.is_expanded_root(cluster) != expand => {
                self.toggle_root(cluster, cx)
            }
            Item::Group {
                cluster,
                id,
                expanded,
                ..
            } if *expanded != expand => self.toggle_group(cluster, id, cx),
            // Collapse on a leaf goes to its root.
            _ if !expand => {
                self.selected = Some(Item::Root(item.cluster().clone()).id());
                cx.notify();
            }
            _ => {}
        }
    }

    /// Shows the filter input (the Explorer header's search button).
    pub fn start_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match &self.filter {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter kinds…"));
                let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.schedule_count_sync(cx);
                        cx.notify();
                    }
                });
                self._subscriptions.push(subscription);
                self.filter = Some(input.clone());
                input
            }
        };
        self.state.collapsed_sections.remove("clusters");
        let focus = input.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.notify();
    }

    fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter = None;
        self.focus.focus(window, cx);
        self.schedule_count_sync(cx);
        cx.notify();
    }

    fn render_item(&mut self, item: &Item, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.colors().clone();
        let selected = self.selected.as_deref() == Some(item.id().as_str());
        let id = SharedString::from(item.id());
        let row = match item {
            Item::Root(cluster) => {
                let manager = ConnectionManager::global(cx);
                let (name, color, state, production) = {
                    let m = manager.read(cx);
                    (
                        m.display_name(cluster),
                        m.color(cluster, cx),
                        m.state(cluster),
                        m.context_settings(cluster).production,
                    )
                };
                let mut row = TreeRow::new(id, name)
                    .root(true)
                    .expanded(Some(self.is_expanded_root(cluster)))
                    .icon(IconName::ShipWheel)
                    .icon_color(color)
                    .selected(selected);
                if production {
                    row = row.end_child(ProdBadge);
                }
                let tip = cluster.clone();
                row.end_child(status_slot(cluster, &state, &colors))
                    .tooltip(move |window, cx| {
                        let tip = tip.clone();
                        gpui_component::tooltip::Tooltip::element(move |_, cx| {
                            cluster_tooltip(&tip, cx)
                        })
                        .build(window, cx)
                    })
            }
            Item::Status { text, sign_in, .. } => TreeRow::new(id, text.clone())
                .depth(1)
                .muted_label(true)
                .icon(if *sign_in {
                    IconName::Key
                } else {
                    IconName::Info
                })
                .icon_color(if *sign_in {
                    colors.yellow
                } else {
                    colors.text_dim
                })
                .selected(selected),
            Item::Group {
                label,
                depth,
                expanded,
                badge,
                ..
            } => {
                let row = TreeRow::new(id, label.clone())
                    .depth(*depth)
                    .expanded(Some(*expanded))
                    .selected(selected);
                match badge {
                    Some(badge) => row.end_child(
                        div()
                            .text_size(u(11.0))
                            .text_color(colors.text_dim)
                            .child(badge.clone()),
                    ),
                    None => row,
                }
            }
            Item::Kind {
                cluster,
                kind,
                depth,
            } => {
                let mut row = TreeRow::new(id, kind.label.clone())
                    .depth(*depth)
                    .icon(kind.icon)
                    .selected(selected);
                if kind.gvr.resource == "events" && kind.gvr.group.is_empty() && !selected {
                    row = row.icon_color(colors.yellow);
                }
                if let Some(count) = self.count(cluster, kind, cx) {
                    row = row.count(count);
                }
                row
            }
            Item::View { entry, depth, .. } => TreeRow::new(id, entry.label)
                .depth(*depth)
                .icon(entry.icon)
                .selected(selected),
        };
        let item_for_click = item.clone();
        let root_menu = match item {
            Item::Root(cluster) => Some(cluster.clone()),
            _ => None,
        };
        let row = row.on_click(cx.listener(move |this, _, window, cx| {
            this.focus.focus(window, cx);
            this.activate_item(&item_for_click, window, cx);
        }));
        match root_menu {
            Some(cluster) => div()
                .id(SharedString::from(format!("menu-{cluster}")))
                .child(row)
                .context_menu(move |menu, _, cx| {
                    let connected = ConnectionManager::try_global(cx)
                        .is_some_and(|m| m.read(cx).state(&cluster).is_connected());
                    // A group can show its contexts separately; a context of such a group can
                    // go back to one entry.
                    let grouping = ConnectionManager::try_global(cx).and_then(|m| {
                        let entry = m.read(cx).context(&cluster)?;
                        let group = entry.group.clone()?;
                        Some((group, entry.is_group()))
                    });
                    let switch = cluster.clone();
                    let toggle = cluster.clone();
                    let favorite = cluster.clone();
                    menu.item(
                        gpui_component::menu::PopupMenuItem::new("Switch to Cluster").on_click(
                            move |_, _, cx| {
                                ConnectionManager::global(cx)
                                    .update(cx, |m, cx| m.activate(&switch, cx));
                            },
                        ),
                    )
                    .item(
                        gpui_component::menu::PopupMenuItem::new(if connected {
                            "Disconnect"
                        } else {
                            "Connect"
                        })
                        .on_click(move |_, _, cx| {
                            ConnectionManager::global(cx).update(cx, |m, cx| {
                                if connected {
                                    m.disconnect(&toggle, cx)
                                } else {
                                    m.connect_interactive(&toggle, cx)
                                }
                            });
                        }),
                    )
                    .item(
                        gpui_component::menu::PopupMenuItem::new(
                            "Add Default Namespace to Favorites",
                        )
                        .on_click(move |_, _, cx| {
                            let namespace = ConnectionManager::global(cx)
                                .read(cx)
                                .cluster(&favorite)
                                .and_then(|c| c.default_namespace().map(String::from))
                                .unwrap_or_else(|| "default".into());
                            crate::actions::add_favorite(&favorite, &namespace, cx);
                        }),
                    )
                    .when_some(grouping, |menu, (group, grouped)| {
                        menu.item(
                            gpui_component::menu::PopupMenuItem::new(if grouped {
                                "Show Contexts Separately"
                            } else {
                                "Show as One Cluster"
                            })
                            .on_click(move |_, _, cx| {
                                ConnectionManager::global(cx)
                                    .update(cx, |m, cx| m.set_group_separate(&group, grouped, cx));
                            }),
                        )
                    })
                    .separator()
                    .menu(
                        "Clusters & Kubeconfigs…",
                        Box::new(kubyl_kube::ui::OpenClusters),
                    )
                })
                .into_any_element(),
            None => row.into_any_element(),
        }
    }
}

impl ClustersSection {
    /// `● 4 of 9` in the section header: shows only connected clusters while on.
    fn connected_toggle(&self, colors: &Colors, cx: &mut Context<Self>) -> impl IntoElement {
        let (connected, total) = ConnectionManager::try_global(cx)
            .map(|m| {
                let m = m.read(cx);
                let total = m.contexts().count();
                let connected = m
                    .contexts()
                    .filter(|c| m.state(&c.id).is_connected())
                    .count();
                (connected, total)
            })
            .unwrap_or_default();
        let on = self.state.connected_only;
        let hover = colors.hover;
        h_flex()
            .id("connected-only")
            .gap(u(4.0))
            .px(u(4.0))
            .rounded(u(3.0))
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(if on { colors.accent } else { colors.text_faint })
            .when(on, |this| this.bg(colors.selection))
            .hover(move |s| s.bg(hover))
            .child(StatusDot::new(colors.green))
            .child(format!("{connected} of {total}"))
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(if on {
                    "Showing connected clusters only · click to show all"
                } else {
                    "Show connected clusters only"
                })
                .build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.state.connected_only = !this.state.connected_only;
                this.save(cx);
                this.schedule_count_sync(cx);
                cx.notify();
            }))
    }
}

/// Sorts cluster roots `(id, display name, connected or connecting)` and, with `connected_only`,
/// keeps the live ones and the active cluster.
fn order_roots(
    mut roots: Vec<(ClusterId, String, bool)>,
    order: ClusterOrder,
    connected_only: bool,
    active: Option<&ClusterId>,
) -> Vec<ClusterId> {
    roots.retain(|(id, _, live)| !connected_only || *live || Some(id) == active);
    let key = |name: &str| name.to_lowercase();
    match order {
        ClusterOrder::Name => roots.sort_by_key(|a| key(&a.1)),
        ClusterOrder::ConnectedFirst => {
            roots.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| key(&a.1).cmp(&key(&b.1))))
        }
    }
    roots.into_iter().map(|(id, _, _)| id).collect()
}

/// The connection state at the right end of a cluster row: a green dot when connected, a
/// pulsing dim dot while connecting, the yellow key when a sign-in is needed, a red dot when
/// unreachable or forbidden, nothing when not connected. The error is in the tooltip.
pub(crate) fn status_slot(
    cluster: &ClusterId,
    state: &ConnectionState,
    colors: &Colors,
) -> impl IntoElement {
    let inner = match state {
        ConnectionState::Connected { .. } => Some(StatusDot::new(colors.green).into_any_element()),
        ConnectionState::Connecting => Some(
            StatusDot::new(colors.text_dim)
                .pulsing(SharedString::from(format!("connecting-{cluster}")))
                .into_any_element(),
        ),
        ConnectionState::AuthRequired { .. } => Some(
            Icon::new(IconName::Key)
                .size(12.0)
                .color(colors.yellow)
                .into_any_element(),
        ),
        ConnectionState::Unreachable { .. } | ConnectionState::Forbidden(_) => {
            Some(StatusDot::new(colors.red).into_any_element())
        }
        ConnectionState::Disconnected => None,
    };
    h_flex()
        .flex_none()
        .w(u(12.0))
        .justify_center()
        .children(inner)
}

/// One line about a cluster's connection, for tooltips: `Connected · 38 ms · v1.33.1`, the
/// error with the next retry, `Sign-in required`…
pub(crate) fn state_line(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Connected { latency, version } => {
            format!("Connected · {} ms · {version}", latency.as_millis().max(1))
        }
        ConnectionState::Connecting => "Connecting…".into(),
        ConnectionState::Disconnected => "Not connected · expand to connect".into(),
        ConnectionState::AuthRequired { sign_in: true, .. } => {
            "Sign-in required · click the status row to sign in".into()
        }
        ConnectionState::AuthRequired { message, .. } => {
            format!("Authentication failed: {message}")
        }
        ConnectionState::Unreachable { message, retry_in } => match retry_in {
            Some(delay) => format!(
                "Unreachable: {message} · retrying within {}s",
                delay.as_secs().max(1)
            ),
            None => format!("Unreachable: {message}"),
        },
        ConnectionState::Forbidden(message) => format!("Forbidden: {message}"),
    }
}

/// The tooltip of a cluster row: its name, the connection state and where it comes from.
fn cluster_tooltip(cluster: &ClusterId, cx: &App) -> gpui::AnyElement {
    let colors = cx.colors().clone();
    let Some(manager) = ConnectionManager::try_global(cx) else {
        return div().into_any_element();
    };
    let manager = manager.read(cx);
    let state = manager.state(cluster);
    let dot = match &state {
        ConnectionState::Connected { .. } => Some(colors.green),
        ConnectionState::Connecting | ConnectionState::Disconnected => Some(colors.text_faint),
        ConnectionState::AuthRequired { .. } => Some(colors.yellow),
        _ => Some(colors.red),
    };
    let info = manager.context(cluster);
    let source = info
        .map(|c| kubyl_kube::settings::display_path(&c.file))
        .unwrap_or_default();
    let mono = |text: String, color| {
        div()
            .font_family(fonts::MONO)
            .text_size(u(11.0))
            .text_color(color)
            .child(text)
    };
    let mut tip = v_flex()
        .py(u(4.0))
        .gap(u(3.0))
        .max_w(u(460.0))
        .text_size(u(12.0))
        .child(
            div()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(manager.display_name(cluster)),
        )
        .child(
            h_flex()
                .gap(u(6.0))
                .text_color(colors.text_muted)
                .children(dot.map(StatusDot::new))
                .child(div().min_w_0().child(state_line(&state))),
        );
    let Some(info) = info.filter(|i| i.is_group()) else {
        return tip.child(mono(source, colors.text_dim)).into_any_element();
    };
    // A group: who it signs in as, and the contexts it stands for.
    tip = tip
        .child(div().text_color(colors.text_dim).child(format!(
            "User {} on {}",
            info.user.clone().unwrap_or_default(),
            info.server.clone().unwrap_or_default()
        )))
        .child(div().pt(u(3.0)).text_color(colors.text_dim).child(format!(
            "{} contexts in {source}, shown as one cluster:",
            info.members.len()
        )));
    const SHOWN: usize = 6;
    for member in info.members.iter().take(SHOWN) {
        tip = tip.child(
            h_flex()
                .gap(u(10.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(mono(member.context.clone(), colors.text_muted)),
                )
                .child(mono(
                    member.namespace.clone().unwrap_or_else(|| "default".into()),
                    colors.text_dim,
                )),
        );
    }
    if info.members.len() > SHOWN {
        tip = tip.child(
            div()
                .text_color(colors.text_dim)
                .child(format!("and {} more", info.members.len() - SHOWN)),
        );
    }
    tip.into_any_element()
}

impl Focusable for ClustersSection {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ClustersSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let collapsed = self.state.collapsed_sections.contains("clusters");
        let items = if collapsed {
            Vec::new()
        } else {
            self.items(cx)
        };
        let rows: Vec<gpui::AnyElement> = items
            .iter()
            .map(|item| self.render_item(item, cx))
            .collect();
        let empty = !collapsed
            && items.is_empty()
            && ConnectionManager::try_global(cx).is_some_and(|m| !m.read(cx).is_loading());
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .w_full()
            .pb(u(8.0))
            .on_action(cx.listener(|this, _: &SelectNext, window, cx| {
                if this
                    .filter
                    .as_ref()
                    .is_some_and(|f| f.read(cx).focus_handle(cx).is_focused(window))
                {
                    this.focus.focus(window, cx);
                }
                this.move_selection(1, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &Expand, _, cx| this.expand_selected(true, cx)))
            .on_action(cx.listener(|this, _: &Collapse, _, cx| this.expand_selected(false, cx)))
            .on_action(cx.listener(|this, _: &Activate, window, cx| {
                if let Some(item) = this.selected_item(cx) {
                    this.activate_item(&item, window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &ClearFilter, window, cx| this.clear_filter(window, cx)),
            )
            .child(div().h(u(6.0)))
            .child(
                SectionHeader::new("clusters", "Clusters")
                    .collapsed(collapsed)
                    .end_child(self.connected_toggle(&colors, cx))
                    .on_toggle(cx.listener(|this, _, _, cx| {
                        if !this.state.collapsed_sections.remove("clusters") {
                            this.state.collapsed_sections.insert("clusters".into());
                        }
                        this.save(cx);
                        cx.notify();
                    })),
            )
            .when_some(self.filter.clone(), |this, input| {
                this.child(
                    div()
                        .key_context("ExplorerFilter")
                        .mx(u(8.0))
                        .mb(u(4.0))
                        .h(u(24.0))
                        .px(u(6.0))
                        .flex()
                        .items_center()
                        .rounded(u(4.0))
                        .bg(colors.input_background)
                        .border_1()
                        .border_color(colors.accent)
                        .text_size(u(12.0))
                        .child(
                            Input::new(&input)
                                .appearance(false)
                                .prefix(Icon::new(IconName::Search).size(12.0)),
                        ),
                )
            })
            .children(rows)
            .when(empty, |this| {
                this.child(
                    div()
                        .px(u(12.0))
                        .pt(u(6.0))
                        .text_size(u(12.0))
                        .text_color(colors.text_dim)
                        .font_family(fonts::UI)
                        .child(if self.state.connected_only {
                            "No connected clusters. Turn off \"connected only\" in the header."
                        } else {
                            "No kubeconfig contexts. Add one with + above."
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn roots() -> Vec<(ClusterId, String, bool)> {
        vec![
            (ClusterId::new("s"), "staging".into(), false),
            (ClusterId::new("k"), "Kind-dev".into(), true),
            (ClusterId::new("p"), "prod".into(), true),
            (ClusterId::new("a"), "aks-lab".into(), false),
        ]
    }

    fn names(ids: Vec<ClusterId>) -> Vec<String> {
        ids.into_iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn roots_sort_by_name_or_connected_first() {
        assert_eq!(
            names(order_roots(roots(), ClusterOrder::Name, false, None)),
            ["a", "k", "p", "s"]
        );
        assert_eq!(
            names(order_roots(
                roots(),
                ClusterOrder::ConnectedFirst,
                false,
                None
            )),
            ["k", "p", "a", "s"]
        );
    }

    #[test]
    fn connected_only_keeps_the_active_cluster() {
        let active = ClusterId::new("s");
        assert_eq!(
            names(order_roots(
                roots(),
                ClusterOrder::Name,
                true,
                Some(&active)
            )),
            ["k", "p", "s"]
        );
        assert_eq!(
            names(order_roots(roots(), ClusterOrder::Name, true, None)),
            ["k", "p"]
        );
    }

    #[test]
    fn state_lines_explain_the_dot() {
        let connected = ConnectionState::Connected {
            latency: Duration::from_millis(38),
            version: "v1.33.1".into(),
        };
        assert_eq!(state_line(&connected), "Connected · 38 ms · v1.33.1");
        let unreachable = ConnectionState::Unreachable {
            message: "connection refused".into(),
            retry_in: Some(Duration::from_secs(20)),
        };
        assert_eq!(
            state_line(&unreachable),
            "Unreachable: connection refused · retrying within 20s"
        );
        assert!(state_line(&ConnectionState::Forbidden("no".into())).starts_with("Forbidden"));
        assert!(state_line(&ConnectionState::Disconnected).starts_with("Not connected"));
    }
}
